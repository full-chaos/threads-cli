# Design: Create/Publish Posts & Replies + Upsert Hardening

Date: 2026-05-20
Status: approved design (pre-plan)
Branch: `feat/publish-and-upsert-hardening`

## TL;DR

Add a `threads-cli post create` command that publishes a **text/image/video/carousel
post or a reply (comment)** to Threads via the official two-step publish flow, and
**harden the local upsert** so re-ingesting a post through a sparse reply edge can no
longer corrupt richer data already stored. After a successful publish the new post is
fetched and upserted into the local SQLite store so it is immediately visible to
`show`/`search`/`export`.

Two features, one plan:
- **Feature A — Upsert hardening** (`threads-core`, `threads-store`): a pure
  `Post::merge` + author-identity resolution. Self-contained, high value, parallelizable.
- **Feature B — Create/publish** (`manifests`, `threads-core`,
  `threads-provider-official`, `threads-cli`): the new write command. Feature B
  exercises Feature A on its after-publish store write.

> Note: the PRD positions publishing as post-v1. This plan deliberately pulls it
> forward as a new capability; treat it as an additive surface, not a v1 contract change.

## Decisions (locked — do not relitigate)

| # | Question | Decision |
|---|----------|----------|
| D1 | Is "upsert" already implemented? | Yes — local upsert + ingest are wired end-to-end. This plan **hardens** it, it does not build it from scratch. |
| D2 | Merge mechanism for re-fetched posts | **Read-merge-write in Rust**: a pure `Post::merge(existing, incoming)` in `threads-core`; `upsert_post_tx` loads, merges, writes the merged row, and reconciles edges/children from merged values. |
| D3 | Author-identity flip (`owner.id` vs synthesized `@handle`) | **Full resolution**: persist `me`'s profile during ingest; when a `(username → real id)` pairing is learned, rewrite `posts.author_id` from `@handle` to the real id and drop the placeholder user. |
| D4 | Media scope for create | **Full**: text, image, video, and carousel (carousel = ≥2 media items). |
| D5 | Publish safety/UX | **Confirm-then-publish**: publish on a normal invocation, but show a composed preview and require `y/N` on a TTY; require `--yes` when not on a TTY. Print the resulting post URL. |
| D6 | Write the published post to the local store? | **Yes** — fetch the canonical post by id, then run it through the hardened upsert. Fall back to a synthesized `Post` from inputs if the fetch fails. |
| D7 | Quota source of truth | **Remote** `GET /me/threads_publishing_limit` (authoritative), not a local audit table. Still handle a mid-flight `429`. |
| D8 | Orchestration location | `crates/threads-cli/src/commands/post.rs` with factored helpers (mirrors `delete.rs`). No new crate. |
| D9 | "Comment" vs "post" | A comment is a create call with `reply_to_id` set. One command, one `--reply-to <ID>` flag — not a separate subcommand. |

## Endpoints (official Threads Graph API, host `https://graph.threads.net`, version `v1.0`)

All facts below were verified against developers.facebook.com (Posts, Publishing
reference, Create Replies, Get Started, Overview, Troubleshooting). Param names and
numbers are quoted from those pages.

### Publish flow (two-step)
- **Create container** — `POST /v1.0/me/threads`
  - `media_type` (required): `TEXT` | `IMAGE` | `VIDEO` | `CAROUSEL`.
  - `text` (optional), `link_attachment` (optional URL), `reply_control` (optional),
    `topic_tag` (optional), `quote_post_id` (optional, **out of scope**).
  - `reply_to_id` (**required if replying**) — makes the container a reply/comment.
  - Media: `image_url` / `video_url` (must be a public HTTPS URL — Meta cURLs it).
  - Carousel: each child created with `is_carousel_item=true`; parent
    `media_type=CAROUSEL` with `children` = comma-separated child container ids; **2–20 items**.
  - Response: `{ "id": "<CONTAINER_ID>" }` (treat as **opaque string**).
- **Publish** — `POST /v1.0/me/threads_publish`
  - `creation_id` (required) = the container id.
  - Response: `{ "id": "<PUBLISHED_POST_ID>" }`.
- **Container status** — `GET /v1.0/{container-id}?fields=status`
  - Values: `EXPIRED | ERROR | FINISHED | IN_PROGRESS | PUBLISHED`.
  - Poll ~once/minute, ≤5 minutes. Needed for `VIDEO`/`CAROUSEL`; for `TEXT` an
    immediate publish usually works (Meta recommends ~30s to fully process any upload).
- **Container expiry**: unpublished containers expire after **24h** (`EXPIRED`).

### Quota
- **Read** — `GET /v1.0/me/threads_publishing_limit` → fields:
  `quota_usage`, `config { quota_total, quota_duration }`,
  `reply_quota_usage`, `reply_config { quota_total, quota_duration }`,
  `delete_quota_usage`, `delete_config`, `location_search_quota_usage`, `location_search_config`.
- **Caps**: **250 posts / 24h** and a **separate 1,000 replies / 24h** (`quota_duration = 86400`).
  Replies do **not** count against the post quota.

### Permissions / scopes
- Publishing requires **`threads_content_publish`** (plus the always-required `threads_basic`).
- Replying to **your own** root post: `threads_content_publish` is sufficient.
- Replying to **others'** posts: additionally requires `threads_keyword_search` **or**
  `threads_manage_mentions`.
- The token-exchange response carries `user_id`; the publish path may also use the literal `me`.

### Gotchas baked into the design
- Text limit **500 characters**; emoji count as UTF-8 bytes server-side. Validate
  conservatively (reject obviously-over content; surface the API error otherwise).
- Container id may render as int or string in docs → **opaque string** in Rust.
- Media URLs must be public + reachable by Meta (no localhost/auth-gated/private).

## Architecture summary (one diagram)

```
post create --text "…" [--reply-to ID] [--image-url U]… [--video-url U] [--yes]
        │
        ▼
commands/post.rs
  1. load token; require scope threads_content_publish
  2. build PublishRequest (infer media_type from inputs; validate text ≤ 500)
  3. preflight: GET /me/threads_publishing_limit
       post  → require quota_usage       < 250
       reply → require reply_quota_usage  < 1000
  4. preview + confirm (y/N on TTY; --yes off-TTY)
  5. provider.publish(req):                       ── threads-provider-official ──
        TEXT:      create_container → publish_container
        IMAGE:     create_container → (status poll) → publish_container
        VIDEO/CAR: create child containers → poll status FINISHED → create parent
                   → poll → publish_container
        reply:     create_container carries reply_to_id
       → returns PostId
  6. fetch canonical post: provider.fetch_post(PostId)   (full fields)
  7. store.upsert_post(post)   ── Feature A hardened merge ──
  8. print published URL (permalink) + local store note
```

```
ingest me / thread / engagement (existing)
        │  normalized Post (sparse for reply edges)
        ▼
threads-store::upsert_post_tx
  @-sentinel author ─► load_post ─► Post::merge ─► write merged row + reconcile children
  real (numeric) author ─► overwrite (trust the full fetch; allows clearing fields)
  + author resolution: rewrite @handle → real id when (username→id) is known
```

## Interfaces / file-level design

> Signatures are design intent; the implementation plan (writing-plans output, to land
> in `docs/plans/`) will specify exact file diffs and tests.

### Feature A — upsert hardening

**`crates/threads-core/src/model.rs`** — add a pure merge:
```rust
impl Post {
    /// Merge a re-fetched `incoming` post onto an `existing` stored post,
    /// never losing data to a sparser fetch. Pure; unit-tested without SQLite.
    pub fn merge(existing: Post, incoming: Post) -> Post { /* see rules table */ }
}
```
Merge rules:
| Field | Rule |
|---|---|
| `text`, `created_at`, `permalink`, `parent_id`, `root_id` | `incoming.or(existing)` (keep known value when incoming is `None`) |
| `author` | prefer a non-`@`-prefixed id over an `@handle`; else `incoming` |
| `is_quote_post` | `existing || incoming` (sticky true) |
| `media`, `urls`, `mentions` | `incoming` unless empty, then `existing` |
| `id`, `raw` | `incoming` |

**`crates/threads-store/src/query.rs`** — `upsert_post_tx`:
- Before writing, **and only when the incoming author is the `@username` sentinel** (i.e. a
  sparse reply-edge fetch), `load_post(&tx, id)`; if `Some`, compute `Post::merge(existing, incoming)`
  and use the merged value for the row write, child-table rebuild, and edge reconciliation.
- For a real (non-`@`) author — a full fetch — skip the merge and keep the prior overwrite, so
  intentional corrections (e.g. clearing `parent_id`) still propagate. This reconciles the
  merge's field-preservation rules with the existing `reupsert_without_parent_drops_stale_edges`
  behavior; the `@` sentinel is the signal that the fetch is structurally incomplete.
- Keep the existing delete-then-reinsert of children/edges, but drive it from the resulting post.

**Author resolution** (`threads-provider-official` + `threads-store` + `threads-ingest`):
- *Current state:* the live author synthesis is in `provider::dto_to_post`, which uses a
  **bare `username`** when `owner` is absent (reply edges `me/replies`, `post/replies`,
  `post/conversation` do not request `owner`; `me/threads` and the `post` object do). The
  `OfficialNormalizer` in `threads-ingest/normalizer.rs` (which uses `@username`) is **not
  called by the orchestrator** — it is currently dead code. A bare username is
  indistinguishable from a numeric `owner.id`.
- *Sentinel:* change `dto_to_post` to synthesize the detectable form `@username` so
  `author_id LIKE '@%'` precisely means "synthesized, awaiting resolution". (Updates the
  existing `dto_to_post_synthesizes_author_from_username` test to expect `@alice`. Optionally
  retire the dead `OfficialNormalizer` to remove the divergent second path.)
- `threads-ingest` orchestrator: call `store.upsert_user(&me)` in `ingest_me` and
  `ingest_engagement` (currently `me` is fetched then discarded except `.id`).
- `threads-store`: add `resolve_author(username, real_id)` that, in one txn, upserts the
  real user, runs `UPDATE posts SET author_id = real_id WHERE author_id = '@' || username`,
  then deletes the `@username` placeholder user row (posts are re-keyed before the delete so
  the `ON DELETE CASCADE` is a no-op). `edges` are intentionally NOT touched — `edges.from_id`
  holds post ids, never an author handle, so an author rewrite has nothing to reconcile there.
- Trigger resolution whenever a `(username, real_id)` pair is observed (from `/me`, or any
  post DTO carrying both `owner.id` and `username`).
- The store is pre-1.0 and re-ingestable, so any rows written under the old bare-username
  form before this change are not migrated; a re-ingest re-keys them via the sentinel.

### Feature B — create/publish

**`manifests/official_v1.toml`** — new entries:
- `[[actions]] post/create` → `POST /v1.0/me/threads`, `permission = "threads_content_publish"`.
- `[[actions]] post/publish` → `POST /v1.0/me/threads_publish`, same permission.
- `[[objects]] container` → `GET /v1.0/{container-id}`, fields `["status"]`.
- `[[objects]] publishing_limit` → `GET /v1.0/me/threads_publishing_limit` with the quota fields.
- Record `rate_limit_per_day = 250` (posts) and a reply cap of `1000` as metadata.

**`crates/threads-core/src/model.rs`** (or a new `publish.rs`) — request/response types:
```rust
pub struct PublishRequest {
    pub media_type: PublishMediaType,        // Text | Image | Video | Carousel
    pub text: Option<String>,
    pub reply_to_id: Option<PostId>,
    pub reply_control: Option<ReplyControl>, // Everyone | AccountsYouFollow | MentionedOnly | …
    pub link_attachment: Option<String>,
    pub media: Vec<MediaInput>,              // url + kind; ≥2 ⇒ carousel
}
pub struct MediaInput { pub kind: MediaKind, pub url: String }
pub struct ContainerId(pub String);          // opaque
pub enum ContainerStatus { Expired, Error, Finished, InProgress, Published }
pub struct PublishingLimits {
    pub post_usage: u32, pub post_total: u32,
    pub reply_usage: u32, pub reply_total: u32,
}
```

**`crates/threads-core/src/provider.rs`** — trait additions (default `Err(NotSupported)`):
```rust
async fn create_container(&self, req: &PublishRequest) -> Result<ContainerId>;
async fn publish_container(&self, id: &ContainerId) -> Result<PostId>;
async fn container_status(&self, id: &ContainerId) -> Result<ContainerStatus>;
async fn publishing_limits(&self) -> Result<PublishingLimits>;
async fn fetch_post(&self, id: &PostId) -> Result<Post>;   // for after-publish upsert
```

**`crates/threads-provider-official/src/client.rs`** — `post_json` mirroring `delete_json`
(429/5xx retry, backoff, `x-app-usage`, redaction, error mapping; params on the query string
alongside `access_token`).

**`crates/threads-provider-official/src/provider.rs`** — implement the trait methods via
`action_path`/`object_path` + `HttpClient::post_json`/`get_json`, plus DTOs in `dto.rs`.

**`crates/threads-cli/src/cli.rs`** — new command:
```
Command::Post(PostCommand) → Create(PostCreateArgs)
PostCreateArgs:
  --text <STRING|->            text body (or stdin when "-")
  --reply-to <POST_ID>         create a reply/comment
  --image-url <URL>            repeatable; ≥2 media ⇒ carousel
  --video-url <URL>            repeatable
  --reply-control <ENUM>       who can reply
  --link-attachment <URL>
  --yes                        skip confirmation (required off-TTY)
```

**`crates/threads-cli/src/commands/post.rs`** — orchestration (helpers): scope check,
`PublishRequest` build + `media_type` inference + text validation, remote quota preflight,
confirm gate, `provider.publish(...)`, fetch-then-upsert, print URL.

**`crates/threads-cli/src/commands/auth.rs`** — add `threads_content_publish` to the
requested OAuth scopes (and document that replying to others needs
`threads_keyword_search`/`threads_manage_mentions`).

## Out of scope (do not build)

- Editing posts (no Threads edit API).
- Local-file media upload (API only accepts public URLs).
- Scheduling, drafts, quote-posts (`quote_post_id`).
- Reply moderation (hide/approve), insights, location search.
- A local publish audit table (quota is read from the remote endpoint).

## Test matrix

| Layer | Test | Asserts |
|---|---|---|
| core | `merge_keeps_known_created_at_when_incoming_none` | sparse fetch can't null a known timestamp |
| core | `merge_prefers_real_author_over_handle` | `@handle` never overwrites `owner.id` |
| core | `merge_is_quote_post_sticky_true` | quote flag not downgraded |
| core | `merge_keeps_media_when_incoming_empty` | reply-edge re-fetch can't wipe media |
| store | `reupsert_via_sparse_reply_preserves_rich_root` | end-to-end merge through `upsert_post_tx` |
| store | `resolve_author_rewrites_handle_to_real_id` | `@handle` posts re-keyed; placeholder removed |
| store | `posts_by_author_finds_my_replies_after_resolution` | engagement seed fix |
| provider | `create_container_text_builds_expected_params` | `media_type=TEXT`, `text`, `reply_to_id` mapping |
| provider | `publish_container_returns_post_id` | parses `{ "id": … }` |
| provider | `container_status_parses_enum` | status string → enum |
| provider | `publishing_limits_parses_quota_fields` | post/reply usage + totals |
| cli | `media_type_inference` | 0→TEXT, 1 img→IMAGE, 1 vid→VIDEO, ≥2→CAROUSEL |
| cli | `text_over_500_rejected` | length guard |
| cli | `confirm_required_off_tty_without_yes` | safety gate |
| cli | `preflight_blocks_when_quota_exhausted` | refuses at 250 posts / 1000 replies |
| cli (mock provider) | `publish_then_upsert_makes_post_searchable` | the A↔B join |

## Conventions every team must follow

- Errors: `thiserror` in library crates, `anyhow` (with context) in `threads-cli`.
- Async: `tokio`; provider trait is `async_trait`, object-safe.
- Manifest-driven paths: no hard-coded endpoint strings in the provider; resolve via
  `action_path`/`object_path`/`edge_path`, substituting `{container-id}` like the existing
  `{post-id}`/`{reply-id}` helper.
- Time: RFC 3339; tolerate Meta's colonless `+0000` offset (existing `parse_timestamp`).
- Secrets: never log tokens; reuse `redact` on error bodies.
- Migrations: additive, idempotent, versioned (none required for this plan unless author
  resolution needs an index).
- New columns/tables only from the internal model, never derived from provider DTOs.

## Milestones (writing-plans will expand into waves)

1. **A1–A2** `Post::merge` + `upsert_post_tx` integration (isolated, parallelizable).
2. **B-infra** `post_json`, provider trait + DTOs, core publish types, manifest entries.
3. **B-text** text post + reply slice end-to-end (shippable): build → preflight → confirm →
   create → publish → fetch → upsert → print URL.
4. **B-media** image, then video (status polling), then carousel (multi-container).
5. **A3** author-identity resolution (independent; parallel to B).

Dependencies: B-text depends on B-infra; B-media depends on B-text; A3 depends on A1–A2
only for the merge it reuses during resolution rewrites.
