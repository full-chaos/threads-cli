# threads-cli — Architecture

## One-line summary

`https://graph.threads.net` is Meta's **REST-like Graph API**, not GraphQL. We
drive it from a versioned local TOML manifest and normalize every response into
a stable internal graph model before touching SQLite.

## Crate graph

```
                  ┌──────────────┐
                  │ threads-cli  │  (binary: clap subcommands)
                  └──────┬───────┘
                         │
     ┌───────────────────┼────────────────────┐
     ▼                   ▼                    ▼
┌──────────┐     ┌──────────────┐     ┌────────────┐
│ ingest   │◄────┤ provider-*   │     │  store     │
│          │     │ (official,   │     │            │
│          │─────► web [opt])   │     │            │
└────┬─────┘     └──────┬───────┘     └─────┬──────┘
     │                  │                   │
     ▼                  ▼                   ▼
┌─────────────────────────────────────────────────┐
│                  threads-core                   │
│   Provider trait · internal model · errors      │
└─────────────────────────────────────────────────┘
                  ▲
                  │
           ┌──────┴────────┐
           │ threads-      │
           │ manifest      │  (TOML → typed endpoints)
           └───────────────┘
```

## The "normalize, don't DDL" rule (from the PRD)

```
GOOD: Official API response → typed provider DTO → normalizer → internal model → SQLite
BAD:  Official API response → dynamically generated database schema
```

Consequences:

1. **Provider changes are normalizer edits, not migrations.** Re-run
   normalization over retained `raw_payloads` to backfill new fields.
2. **Search indexes remain valid across provider versions.** FTS5 sits on the
   internal `posts.text`, not on provider-shaped rows.
3. **Two providers, one store.** When the optional `threads-provider-web`
   adapter is enabled, its normalizer emits the same `Post`/`Edge`/`Media`
   records; downstream code cannot tell the difference.

## Provider contract

All data sources implement [`threads_core::Provider`]:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch_me(&self) -> Result<User>;
    async fn fetch_my_threads(&self, cursor: Option<Cursor>) -> Result<Page<Post>>;
    async fn fetch_replies(&self, post_id: &PostId, cursor: Option<Cursor>) -> Result<Page<Post>>;
    async fn fetch_thread(&self, root_id: &PostId) -> Result<Vec<Post>>;
    async fn fetch_audience_insight(&self, user_id: &UserId, query: AudienceInsightQuery)
        -> Result<AudienceInsightResult>;
    async fn fetch_mentions(&self, user_id: &UserId, cursor: Option<Cursor>, limit: usize)
        -> Result<Page<Post>>;
}
```

The complete trait also contains post retrieval, publishing, deletion, and
reply methods. There is deliberately no follower-list or follow-mutation
method. Audience insights are aggregate; Mentions are paginated public-media
records.

Pagination is cursor-based; providers translate their native paging (Meta's
`paging.cursors.after`, etc.) into our opaque `Cursor(String)`.

## Provider priority

```
1. official (graph.threads.net)  — always on, v1 target
2. cache    (local SQLite store) — read-through
3. web      (threads.net/api/graphql) — EXPERIMENTAL, feature-gated off
```

The private web provider is **never** auto-enabled. It ships behind the
`enabled` Cargo feature in `threads-provider-web` and requires an explicit
runtime flag to participate in ingests.

## Data flow (ingest)

```
init      → writes ~/.config/threads-cli/config.toml
auth login→ OAuth, atomically mirror token metadata/access token to a private file and save to Keychain best-effort
ingest me → orchestrator {
    1. fetch_me()
    2. loop fetch_my_threads(cursor)
    3. for each post: fetch_replies(post.id, cursor)
    4. normalize each payload → Post/Edge/Media records
    5. tag with fetch_run_id
    6. upsert via threads-store (FTS triggers run here)
    7. retain raw JSON in raw_payloads table
}
```

Token reads are file-first. The private token-file mirror is always written on
save and is read before Keychain; Keychain storage is best-effort rather than a
fallback-only persistence path. On Unix, the mirror is atomically replaced and
accepted only from an owner-controlled, non-group/world-writable directory as
an owner-only regular file.

## Data flow (delete — destructive remote)

`delete` is the destructive remote-delete path, and `post` performs supported
remote publishing. `follow` only opens a user-mediated official intent and
performs no API write. Audience refresh writes account-scoped local snapshots
and observed mention/reply records; `show`, `engaged`, and `purge` are local.
Full delete design: [`docs/plans/delete.md`](plans/delete.md).

```
delete posts --before X --after Y [--apply] [--limit N]
    1. parse_time(--before, --after)              → DateTime<Utc>
    2. validate token has `threads_delete` scope  → token_has_scope (strict)
    3. fetch_me()                                  → me.id
    4. store.posts_in_window(me.id, after, before, kind, limit)
    5. print DRY RUN summary; --apply ? continue : return
    6. (replies) interactive `undocumented endpoint` confirmation
    7. pre-flight rate-limit check
         a. store.deletions_in_last_24h() < 100
         b. else bail with `quota resets at <oldest + 24h>`
    8. for each id in window:
         a. provider.delete_post(id) → DELETE /v1.0/{id}
         b. on Ok:    store.delete_post(id) (tx: edges + posts)
                      store.record_deletion(id, kind, ok=true)
         c. on Err:   store.record_deletion(id, kind, ok=false, err=...)
         d. on Error::RateLimit: stop batch cleanly
    9. print summary: deleted, failed, remaining_quota_24h
```

Key invariants:

- **Dry-run is the default.** `--apply` is required to actually delete.
- **Local store is the candidate source of truth.** `delete` does not
  enumerate from the API; the user runs `ingest me` first to refresh.
- **`deletions` audit table** records every attempt (success or failure)
  so the 100/24h rate limit gate is auditable across processes.
- **`edges` cleanup is manual** — the table has no FK to `posts`, so
  `store.delete_post` opens a transaction that removes both directions of
  edges referencing the deleted id before deleting the post row itself.
- **`archive` is intentionally absent.** Meta exposes no remote archive
  endpoint for root posts; the `ingest` command serves the local-archive
  role.

## Data flow (audience refresh)

```
audience refresh
    → OfficialProvider: typed Insights (count + one demographic breakdown/call)
    → threads-core typed AudienceInsightResult
    → transactional audience snapshot store
    → separate paginated official Mentions phase
    → typed Post + Mention records in the local store
```

Insights persistence is atomic. Once the snapshot is committed, only a Mentions
permission denial is downgraded to a warning; authentication, network, parse,
rate-limit, and store errors fail the command while retaining the committed
snapshot. The resulting trend is local snapshot history, not Meta historical
data. The web provider is read-only and is not part of this flow. Audience/raw
data is account-scoped and excluded from post export.

## Manifest action types

- `[[objects]]` — single-resource GET (e.g. `/me`, `/{post-id}`)
- `[[edges]]` — paginated GET (e.g. `/me/threads`, `/{post-id}/replies`)
- `[[actions]]` — write operations (`DELETE /{post-id}`, etc.)

Adding a new action is a manifest edit + a `Provider` trait method with a
default `Err(Error::NotSupported)` impl + an override on `OfficialProvider`.
The experimental `web` provider stays read-only by inheriting the default.

## SQLite strategy

- Single DB file at `~/.local/share/threads-cli/store.db`.
- Schema managed by `threads-store` via versioned migrations.
- `posts_fts` FTS5 virtual table with triggers mirroring `posts.text`.
- Recursive CTE for thread traversal (`show --thread`).
- `raw_payloads` retains provider JSON for replay / re-normalization.
