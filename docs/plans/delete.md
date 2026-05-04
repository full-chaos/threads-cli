# Plan: `delete` — remote-only, time-windowed

> **Status:** Spec, locked. Source of truth for parallel implementation teams.
> **Scope decisions** are user-confirmed (see "Decisions" section). Anything
> not listed here is OUT OF SCOPE and must not be silently expanded.

---

## TL;DR

Add two new CLI subcommands that issue *real* `DELETE` requests against
`https://graph.threads.net` for the authenticated user's content within a
`--before` / `--after` time window:

```
threads-cli delete posts   [--before <date>] [--after <date>] [--apply] [--limit N]
threads-cli delete replies [--before <date>] [--after <date>] [--apply] [--limit N]
```

Default behavior is **dry-run** (prints what would be deleted, changes
nothing). `--apply` performs the actual deletion. The local SQLite store is
the source of truth for *what to delete* (you must `ingest me` first); the
remote API is the source of truth for *whether the delete succeeded*.

`archive` is **explicitly NOT being built.** Meta exposes no remote archive
operation for root posts. The user already considers `ingest` to be the local
archive; we are not adding a contradicting archive command.

---

## Decisions (locked, do not relitigate)

| Question | Answer |
|---|---|
| Local vs remote? | **Remote-only.** `ingest` already serves "local archive". |
| Archive included? | **No.** Meta has no archive endpoint for root posts. |
| Time field? | `posts.created_at` (the post's authored timestamp). |
| Confirmation UX? | **Dry-run by default**, `--apply` to actually delete. |
| Replies endpoint? | `DELETE /{reply-id}` (probe + warn — undocumented for replies; replies are media objects so it should work). |
| Reply hide via `manage_reply`? | Out of scope. Different semantic; future `moderate` command. |

---

## Endpoints (Threads Graph API v1.0)

### Delete a post (officially documented)

```
DELETE https://graph.threads.net/v1.0/{threads-media-id}?access_token=...
```

- **Permission:** `threads_basic` + `threads_delete`
- **Rate limit:** **100 deletions per 24h per account** (hard cap)
- **Reference:** https://developers.facebook.com/docs/threads/posts/delete-posts

### Delete a reply (probe — undocumented)

Same path: `DELETE /v1.0/{reply-id}`. Replies are media objects in the Threads
data model; the same DELETE endpoint should work for IDs that happen to be
replies. Meta does **not** explicitly document this. We MUST:

1. Flag the manifest entry as `documented = false`.
2. Surface a one-time warning the first time a user runs `delete replies
   --apply`: "This endpoint is not officially documented for replies. Verify
   on a single test reply before deleting in bulk."
3. Treat `403 Forbidden` and `404 Not Found` from this path as user-fixable,
   not as bugs.

### Time-window filtering (server-side, supplementary)

`/me/threads` accepts `since` / `until` (Unix timestamps). We use these only
when fetching from the API to refresh the store (out of scope for this plan).
For `delete`, we filter the local store on `posts.created_at` — the user
should `ingest me` first to have a fresh local view.

---

## Architecture summary (one diagram)

```
                     CLI (clap)
                         │
            threads-cli delete posts --before X --after Y [--apply]
                         │
                         ▼
          1. Open store, query candidates
             SELECT id, created_at, text
             FROM posts
             WHERE author_id = me
               AND parent_id IS NULL          (posts) | NOT NULL (replies)
               AND created_at >= --after
               AND created_at <  --before
             LIMIT N
                         │
                         ▼
            2. Dry-run: print summary + sample, RETURN.
                         │
                  (--apply only)
                         ▼
          3. Pre-flight rate-limit check
             count(deletions WHERE deleted_at >= now-24h)
             refuse if >= 100 with clear message
                         │
                         ▼
          4. For each id (respecting --limit and 100/24h cap):
             a. provider.delete_post(id)         ← network DELETE
             b. on success: store.delete_post(id) (CASCADE)
                store.record_deletion(id, kind, ok=true)
             c. on failure: store.record_deletion(id, kind, ok=false, err=...)
                continue with next (don't abort the whole batch)
                         │
                         ▼
                Print final summary (deleted N, failed M, ...)
```

---

## File-level changes (must match these exactly)

### `crates/threads-core/src/error.rs`

Add a new variant. **Do not** alter existing variants.

```rust
#[derive(Debug, Error)]
pub enum Error {
    // ...existing variants kept verbatim...

    /// The provider does not support this operation (e.g. `web` provider
    /// asked to delete). Distinct from network/auth/parse errors so callers
    /// can route to a clear "your provider can't do that" message.
    #[error("operation not supported by this provider: {0}")]
    NotSupported(String),
}
```

### `crates/threads-core/src/provider.rs`

Add two trait methods with default impls returning `NotSupported`. Keeps the
trait object-safe and lets `web` provider (experimental) skip implementing
them without breakage.

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    // ...existing methods kept verbatim...

    /// Delete a post owned by the authenticated user.
    /// Default impl returns `Error::NotSupported`.
    async fn delete_post(&self, _post_id: &PostId) -> Result<()> {
        Err(crate::Error::NotSupported("delete_post".into()))
    }

    /// Delete a reply owned by the authenticated user.
    /// NOTE: undocumented for replies; replies are media objects so the same
    /// DELETE /{id} path is expected to work, but verify on a test reply.
    /// Default impl returns `Error::NotSupported`.
    async fn delete_reply(&self, _reply_id: &PostId) -> Result<()> {
        Err(crate::Error::NotSupported("delete_reply".into()))
    }
}
```

### `manifests/official_v1.toml`

Add a new top-level `[[actions]]` array (not `[[edges]]` — those are
paginated GETs) so the manifest type system distinguishes write ops cleanly.

```toml
# ------------------------------------------------------------------
# Actions (write operations)
# ------------------------------------------------------------------

[[actions]]
name = "post/delete"
path = "/v1.0/{post-id}"
method = "DELETE"
permission = "threads_delete"
documented = true
# Rate limit: 100 successful deletes per 24h per account (Meta-enforced).
rate_limit_per_day = 100

[[actions]]
name = "reply/delete"
path = "/v1.0/{reply-id}"
method = "DELETE"
permission = "threads_delete"
documented = false  # not officially documented for replies; probe behavior
rate_limit_per_day = 100
```

### `crates/threads-manifest/src/lib.rs`

Add an `Action` struct and an `actions: Vec<Action>` field on `Manifest`,
parallel to `Object`/`Edge`. Mark `documented` with `#[serde(default)]` and
`rate_limit_per_day` as `Option<u32>` for forward compatibility.

### `crates/threads-provider-official/src/client.rs`

Add a `delete` method on `HttpClient`. Reuse the existing 401/403/404/429/5xx
mapping and `x-app-usage` backoff logic.

```rust
impl HttpClient {
    /// DELETE `path` (absolute or relative to `base`). Returns the parsed
    /// JSON response body if Meta returns one, or `()` if empty.
    pub async fn delete_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        // ...mirror get_json_value structure but with .delete()...
    }
}
```

### `crates/threads-provider-official/src/provider.rs`

Implement the two trait methods using the manifest action lookup pattern
that already exists for `endpoint_fields` / `edge_path`:

```rust
impl OfficialProvider {
    fn action_path(&self, key: &str) -> Option<String> {
        self.manifest
            .actions
            .iter()
            .find(|a| a.name == key)
            .map(|a| a.path.clone())
    }
}

#[async_trait]
impl Provider for OfficialProvider {
    // ...existing methods kept verbatim...

    async fn delete_post(&self, post_id: &PostId) -> Result<()> {
        let path = self
            .action_path("post/delete")
            .ok_or_else(|| Error::Manifest("missing action `post/delete`".into()))?;
        let path = Self::substitute_post_id(&path, post_id);
        // Threads API may return {"success": true} or just 200; we don't care.
        let _ = self.http.delete_json(&path, &[]).await?;
        Ok(())
    }

    async fn delete_reply(&self, reply_id: &PostId) -> Result<()> {
        let path = self
            .action_path("reply/delete")
            .ok_or_else(|| Error::Manifest("missing action `reply/delete`".into()))?;
        let path = Self::substitute_post_id(&path, reply_id);
        let _ = self.http.delete_json(&path, &[]).await?;
        Ok(())
    }
}
```

`substitute_post_id` already replaces `{post-id}`. We replace `{reply-id}`
the same way for symmetry — Team A may either generalize the helper or add a
sister `substitute_reply_id`. Keep it simple.

### `crates/threads-store/src/migrations.rs`

Add migration **v3** — strictly additive, never drops data.

```rust
fn migration_v3_deletions(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS deletions (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id      TEXT    NOT NULL,
            kind         TEXT    NOT NULL CHECK (kind IN ('post','reply')),
            deleted_at   TEXT    NOT NULL,
            success      INTEGER NOT NULL DEFAULT 1,
            error        TEXT
        );
        CREATE INDEX IF NOT EXISTS deletions_deleted_at_idx ON deletions(deleted_at);
        CREATE INDEX IF NOT EXISTS deletions_post_idx       ON deletions(post_id);
        "
    )
    .map_err(StoreError::Sqlite)
}
```

### `crates/threads-store/src/query.rs`

Add four helpers. **Do not** modify `upsert_post_tx` or any existing function.

```rust
/// Posts in [after, before) authored by `author`, matched on `created_at`.
/// `kind` selects root posts (parent_id IS NULL) vs replies (parent_id NOT NULL).
pub enum PostKind { Post, Reply }

pub fn posts_in_window(
    conn: &Connection,
    author: &UserId,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    kind: PostKind,
    limit: usize,
) -> Result<Vec<Post>>;

/// Hard-delete a post by id.
///
/// `media`, `urls`, `mentions`, and `raw_payloads` are removed by SQLite
/// foreign-key CASCADE (declared in migration v1). The `edges` table has no
/// FK to `posts`, so we DELETE its rows in BOTH directions explicitly
/// (`from_id = id OR to_id = id`), otherwise stale edges would orphan the
/// recursive-CTE thread traversal.
///
/// Wrapped in a single transaction; idempotent (`Ok(false)` if absent).
pub fn delete_post(conn: &mut Connection, id: &PostId) -> Result<bool>;

/// Append a row to `deletions`. NEVER fails the caller if the audit insert
/// fails — log and continue (loss of audit must not abort actual deletion).
pub fn record_deletion(
    conn: &Connection,
    id: &PostId,
    kind: PostKind,
    success: bool,
    error: Option<&str>,
) -> Result<()>;

/// Count rows in `deletions` with deleted_at >= now - 24h AND success = 1.
/// Used for the 100/24h pre-flight rate-limit check.
pub fn deletions_in_last_24h(conn: &Connection) -> Result<u64>;

/// Oldest successful deletion still inside the 24h window. CLI uses this to
/// render `quota resets at <oldest + 24h>` when the cap is hit.
pub fn oldest_deletion_in_last_24h(conn: &Connection) -> Result<Option<DateTime<Utc>>>;
```

Re-export each from `lib.rs` next to the existing `upsert_post`, etc.

### `crates/threads-store/src/store.rs`

Thin wrappers around the four `query` helpers, mirroring the existing
`upsert_post`, `get_post`, etc. style. Lock the connection once per call.

### `crates/threads-provider-official/src/auth.rs`

Update `DEFAULT_SCOPES` to include `threads_delete`:

```rust
pub const DEFAULT_SCOPES: &[&str] = &[
    "threads_basic",
    "threads_read_replies",
    "threads_delete",
];
```

If we want to keep read-only logins lightweight, expose a second constant:

```rust
pub const READ_ONLY_SCOPES: &[&str] = &["threads_basic", "threads_read_replies"];
pub const DELETE_SCOPES: &[&str] = &[
    "threads_basic",
    "threads_read_replies",
    "threads_delete",
];
```

and have `auth login` request `DELETE_SCOPES` by default but accept a
`--scopes minimal|delete` flag. **Recommended:** keep it simple — request
`threads_delete` always; users who don't want it ignore it.

### `crates/threads-provider-official/src/token_store.rs`

Add `granted_scopes: Option<Vec<String>>` to the `Token` struct (
`#[serde(default)]`). On login, capture the scopes that were requested. If
the token lacks `threads_delete` when a delete is attempted, return a
specific `Error::Auth("token missing threads_delete; run auth login")`
variant from the CLI layer (NOT silently ignored).

> Note: Meta's token-exchange endpoint does not return granted scopes in
> the response; we record what we *asked for* at login time. The matching
> `token_has_scope(&Token, scope)` helper is **strict** — `granted_scopes =
> None` (a token saved before scope tracking shipped) reads as missing the
> scope, so the CLI surfaces "run `auth login`" instead of letting Meta
> return an opaque 403.

### `crates/threads-cli/src/cli.rs`

Add the new `Delete` subcommand:

```rust
#[derive(Debug, Subcommand)]
pub enum Command {
    // ...existing...

    /// Delete posts or replies on Threads (remote, irreversible).
    /// Default is DRY-RUN; pass --apply to actually delete.
    #[command(subcommand)]
    Delete(DeleteCommand),
}

#[derive(Debug, Subcommand)]
pub enum DeleteCommand {
    /// Delete top-level posts authored by you.
    Posts(DeleteArgs),
    /// Delete replies authored by you.
    Replies(DeleteArgs),
}

#[derive(Debug, clap::Args)]
pub struct DeleteArgs {
    /// Only consider posts created STRICTLY BEFORE this time (RFC 3339 or YYYY-MM-DD).
    #[arg(long)]
    pub before: Option<String>,

    /// Only consider posts created AT OR AFTER this time (RFC 3339 or YYYY-MM-DD).
    #[arg(long)]
    pub after: Option<String>,

    /// Cap the number of candidates considered. Defaults to no cap, but the
    /// 100/24h rate limit always applies on --apply.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Actually perform the delete. Without this flag, prints what WOULD
    /// be deleted and changes nothing.
    #[arg(long)]
    pub apply: bool,

    /// Skip the "this endpoint is undocumented for replies" warning prompt.
    /// Only relevant for `delete replies`.
    #[arg(long)]
    pub yes_undocumented: bool,
}
```

### `crates/threads-cli/src/commands/delete.rs` (new file)

Single new file; ~150 lines. Implements the Wave-3 workflow precisely as
described in the architecture diagram above. Must:

1. Parse `--before`/`--after` accepting both RFC 3339 (`2025-01-15T00:00:00Z`)
   and bare ISO date (`2025-01-15`, treated as `T00:00:00Z`).
2. If neither flag is given, REFUSE to run (`anyhow::bail!`) — deleting
   "everything" by default would be catastrophic.
3. Resolve `me` once via `provider.fetch_me()` to get the author UserId.
4. Call `store.posts_in_window(...)` with the right `PostKind`.
5. If empty: print "no candidates" and return.
6. Print:
   ```
   DRY RUN — would delete N {posts|replies} authored between A and B:
     <id>  <created_at>  <text snippet 60 chars>
     ... (up to 10 samples)
     ... and (N-10) more
   Run with --apply to actually delete.
   Note: Threads API enforces a hard cap of 100 deletions per 24h.
   ```
7. If `--apply` not set: return.
8. If `--apply` and `delete replies` and not `--yes-undocumented`: print the
   warning and require an interactive `y` confirmation (TTY only — error if
   not on a TTY without `--yes-undocumented`).
9. Pre-flight: call `store.deletions_in_last_24h()`. If `>= 100`, refuse with
   the timestamp of when the oldest counted deletion will fall out of the
   window.
10. Iterate. On each: provider.delete_{post,reply}, then store.delete_post +
    store.record_deletion. Sleep 100ms between calls (cheap rate-limit
    insurance). Catch per-id errors and continue.
11. Print final summary: `deleted: N, failed: M, remaining_quota_24h: K`.

### `crates/threads-cli/src/commands/mod.rs`

Wire the new `Delete` command. Mirror existing patterns.

---

## Out of scope (do not build)

- Any `archive` subcommand (Meta's API doesn't expose archive for posts).
- `manage_reply` / hide-reply moderation (different semantic; future
  `moderate` command).
- Bulk endpoint (Meta has none — every delete is individual).
- Soft-delete in the local store (we hard-delete on success; the
  `deletions` audit table preserves the *fact* that a delete happened).
- Restore / undo (delete is irreversible by API design).
- Multi-account (deferred per README).

---

## Test matrix (Wave 4 enforces)

| Layer | Test |
|---|---|
| `manifest` | `actions` array parses; `documented=false` round-trips |
| `provider` (default trait) | `delete_post` on a stub returns `NotSupported` |
| `provider-official` | `delete_post` issues a `DELETE` to the right URL with `access_token=` query (via mock HTTP) |
| `provider-official` | 429 retried with backoff, then succeeds |
| `provider-official` | 404 surfaced as `Error::NotFound` |
| `store` | migration v3 creates `deletions` table |
| `store` | `posts_in_window(after=X, before=Y, kind=Post)` excludes replies |
| `store` | `posts_in_window(kind=Reply)` excludes top-level posts |
| `store` | `delete_post` is idempotent (returns Ok(false) on missing id) |
| `store` | `deletions_in_last_24h` correctly slides the window |
| `cli` | `delete posts` without `--before`/`--after` errors |
| `cli` | `delete posts` without `--apply` prints DRY RUN, calls no provider methods |
| `cli` | `delete posts --apply` calls provider.delete_post then store.delete_post |
| `cli` | `delete replies --apply` without `--yes-undocumented` prompts (or errors on non-TTY) |
| `cli` | hitting the 100/24h cap mid-batch stops cleanly with a clear message |

---

## Conventions every team must follow

- **Errors**: use `threads_core::Error` variants, never `anyhow::anyhow!` in
  library crates. CLI is allowed `anyhow`.
- **Async**: `async fn` with `async_trait` for trait methods.
- **Logging**: `tracing::{info, warn, debug}`, never `println!` from
  library crates. CLI may `println!` for user output.
- **Migrations**: idempotent (`CREATE TABLE IF NOT EXISTS`). Add to the
  `migrations()` Vec; do NOT renumber existing migrations.
- **Trait additions**: default-impl form to preserve object-safety and
  not break the `web` provider.
- **Tests**: bottom of each file in `mod tests { ... }`. Use `tempfile`
  for store tests, mock HTTP via `wiremock` if needed.
- **Time parsing**: accept both RFC 3339 and `YYYY-MM-DD`. Parse to
  `DateTime<Utc>`. Reject anything else with a clear message.
- **No silent expansion**: if a team needs something not in this doc,
  STOP and surface the gap, do not invent.

---

## Wave plan

1. **Wave 1** (sync, by Sisyphus): write this doc. ✅ DONE.
2. **Wave 2** (3 parallel agent teams):
   - **Team A**: Foundation — error variant, manifest action type, Provider
     trait, HttpClient delete, OfficialProvider impl. Compiles, has unit
     tests, no CLI changes.
   - **Team B**: Store — migration v3, four query helpers, four store
     wrappers, unit tests for each.
   - **Team C**: OAuth scope wiring — `DEFAULT_SCOPES` includes
     `threads_delete`, `Token.granted_scopes` field, scope-mismatch error.
3. **Wave 3** (sequential): CLI command. Depends on all of Wave 2.
4. **Wave 4** (sequential): Oracle review + workspace tests + README +
   architecture.md update.
