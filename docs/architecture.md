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
}
```

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
auth login→ OAuth, store token (keyring | file fallback)
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

## SQLite strategy

- Single DB file at `~/.local/share/threads-cli/store.db`.
- Schema managed by `threads-store` via versioned migrations.
- `posts_fts` FTS5 virtual table with triggers mirroring `posts.text`.
- Recursive CTE for thread traversal (`show --thread`).
- `raw_payloads` retains provider JSON for replay / re-normalization.
