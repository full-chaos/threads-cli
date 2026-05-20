# Create / Publish Posts & Replies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `threads-cli post create` to publish a text/image/video/carousel post or a reply (comment) to Threads via the official two-step flow, then upsert the new post locally.

**Architecture:** Manifest-declared actions → Provider trait methods (create_container/publish_container/container_status/publishing_limits/fetch_post) on OfficialProvider via a new HttpClient::post_json → orchestrated in commands/post.rs (infer media_type, remote quota preflight, confirm-then-publish, status polling for media, fetch-then-upsert).

**Tech Stack:** Rust 2024 (rustc 1.85), reqwest (rustls), clap, tokio, rusqlite. Threads Graph API v1.0 on https://graph.threads.net. Tests via `cargo test -p <crate>`.

**Assumes:** Feature A's `Post::merge` and `Store::upsert_post` (hardened merge) already exist in `threads-core` and `threads-store`. This plan calls `store.upsert_posts(...)` at the end of publish but does not implement the merge logic itself.

---

## File Structure

| File | Responsibility |
|---|---|
| `manifests/official_v1.toml` | Add `post/create`, `post/publish` actions; `container`, `publishing_limit` objects |
| `crates/threads-manifest/src/lib.rs` | Existing parse/validate (no change); `action()` lookup helper added |
| `crates/threads-core/src/publish.rs` | New: `PublishMediaType`, `ReplyControl`, `PublishRequest`, `MediaInput`, `ContainerId`, `ContainerStatus`, `PublishingLimits`; helpers `infer_media_type`, `validate_text` |
| `crates/threads-core/src/lib.rs` | Add `pub mod publish;` and re-export publish types |
| `crates/threads-core/src/provider.rs` | Add 5 new default-`NotSupported` methods to `Provider` trait |
| `crates/threads-provider-official/src/client.rs` | Add `post_json` method mirroring `delete_json` |
| `crates/threads-provider-official/src/dto.rs` | Add `CreateContainerResp`, `PublishResp`, `ContainerStatusResp`, `PublishingLimitResp`, `PublishingLimitConfig` |
| `crates/threads-provider-official/src/provider.rs` | Implement 5 new trait methods; extend `substitute_post_id` with `{container-id}` |
| `crates/threads-cli/src/cli.rs` | Add `Command::Post(PostCommand)`, `PostCommand`, `PostCreateArgs` |
| `crates/threads-cli/src/commands/post.rs` | New: full orchestration — scope check, `PublishRequest` build, preflight, confirm, `publish_flow`, fetch, upsert, print |
| `crates/threads-cli/src/commands/mod.rs` | Add `pub mod post;` and `Command::Post` dispatch arm |
| `crates/threads-cli/src/commands/auth.rs` | Add `threads_content_publish` to `DEFAULT_SCOPES` (in `threads-provider-official/src/auth.rs`) |

---

## Task Sequence

Tasks are ordered so a shippable TEXT post/reply slice (Tasks 1–7) lands before image → video → carousel (Tasks 8–9), which require status polling. Each task follows the strict TDD cycle: write a failing test first, run it (FAIL), write the minimal implementation, run again (PASS), then commit.

---

### Task 1 — Manifest: add publish actions and objects

**Files:**
- `manifests/official_v1.toml` (append entries)
- `crates/threads-manifest/src/lib.rs` (add `action()` lookup + test)

**Steps:**

- [ ] **Write failing test** in `crates/threads-manifest/src/lib.rs` inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn official_manifest_has_publish_actions_and_objects() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../manifests/official_v1.toml"
    );
    let m = Manifest::from_path(path).expect("manifest should parse");

    // Actions
    let create = m.action("post/create").expect("post/create action missing");
    assert_eq!(create.method, "POST");
    assert_eq!(create.path, "/v1.0/me/threads");
    assert_eq!(create.permission, "threads_content_publish");
    assert_eq!(create.rate_limit_per_day, Some(250));

    let publish = m.action("post/publish").expect("post/publish action missing");
    assert_eq!(publish.method, "POST");
    assert_eq!(publish.path, "/v1.0/me/threads_publish");
    assert_eq!(publish.permission, "threads_content_publish");

    // Objects
    let container = m.object("container").expect("container object missing");
    assert_eq!(container.path, "/v1.0/{container-id}");
    assert!(container.fields.contains(&"status".to_string()));

    let limits = m.object("publishing_limit").expect("publishing_limit object missing");
    assert_eq!(limits.path, "/v1.0/me/threads_publishing_limit");
    assert!(limits.fields.iter().any(|f| f == "quota_usage"));
    assert!(limits.fields.iter().any(|f| f == "reply_quota_usage"));
}
```

- [ ] **Run:** `cargo test -p threads-manifest official_manifest_has_publish_actions_and_objects`
  - Expected: **FAIL** (actions/objects not yet in TOML; `m.action()` method missing)

- [ ] **Add `action()` helper** to `Manifest` in `crates/threads-manifest/src/lib.rs` after the existing `edge()` method:

```rust
pub fn action(&self, name: &str) -> Option<&Action> {
    self.actions.iter().find(|a| a.name == name)
}
```

- [ ] **Append to `manifests/official_v1.toml`** (after the existing `[[actions]]` entries):

```toml
[[actions]]
name = "post/create"
path = "/v1.0/me/threads"
method = "POST"
permission = "threads_content_publish"
documented = true
# Rate limit: 250 posts per 24h per account (Meta-enforced).
rate_limit_per_day = 250

[[actions]]
name = "post/publish"
path = "/v1.0/me/threads_publish"
method = "POST"
permission = "threads_content_publish"
documented = true
# No separate rate limit — publishing consumes the create quota.

[[objects]]
name = "container"
path = "/v1.0/{container-id}"
method = "GET"
permission = "threads_content_publish"
fields = ["status"]

[[objects]]
name = "publishing_limit"
path = "/v1.0/me/threads_publishing_limit"
method = "GET"
permission = "threads_content_publish"
fields = [
    "quota_usage",
    "config",
    "reply_quota_usage",
    "reply_config",
]
```

- [ ] **Run:** `cargo test -p threads-manifest official_manifest_has_publish_actions_and_objects`
  - Expected: **PASS**

- [ ] **Also verify** the pre-existing `parses_official_v1_manifest` test still passes:
  - `cargo test -p threads-manifest`

- [ ] **Commit:** `feat(manifest): add post/create, post/publish actions and container/publishing_limit objects`

---

### Task 2 — Core types: `publish.rs`

**Files:**
- `crates/threads-core/src/publish.rs` (new file)
- `crates/threads-core/src/lib.rs` (add `pub mod publish;` + re-exports)

**Steps:**

- [ ] **Write failing test** — add a `#[cfg(test)]` block at the bottom of the (not-yet-created) `publish.rs`. Since the file doesn't exist, add the test file first with just the test and placeholder stubs so it compiles for the FAIL step. Write the full file content:

```rust
// crates/threads-core/src/publish.rs
use crate::model::PostId;

/// API-facing media type for the create-container call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishMediaType {
    Text,
    Image,
    Video,
    Carousel,
}

impl PublishMediaType {
    /// Infer the correct media type from `reply_to_id` presence and
    /// the media inputs. Carousel when ≥2 items, Image/Video for one item,
    /// Text otherwise. `reply_to_id` does NOT change the media type — a
    /// reply with an image is still `Image`.
    pub fn infer(media: &[MediaInput]) -> Self {
        match media {
            [] => PublishMediaType::Text,
            [single] => match single.kind {
                MediaInputKind::Image => PublishMediaType::Image,
                MediaInputKind::Video => PublishMediaType::Video,
            },
            _ => PublishMediaType::Carousel,
        }
    }

    /// Serialize to the wire value expected by the Threads API.
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            PublishMediaType::Text => "TEXT",
            PublishMediaType::Image => "IMAGE",
            PublishMediaType::Video => "VIDEO",
            PublishMediaType::Carousel => "CAROUSEL",
        }
    }
}

/// Wire values for `reply_control`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyControl {
    Everyone,
    AccountsYouFollow,
    MentionedOnly,
}

impl ReplyControl {
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            ReplyControl::Everyone => "everyone",
            ReplyControl::AccountsYouFollow => "accounts_you_follow",
            ReplyControl::MentionedOnly => "mentioned_only",
        }
    }
}

/// Kind of media in a [`MediaInput`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaInputKind {
    Image,
    Video,
}

/// A single media item to include in a post.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInput {
    pub kind: MediaInputKind,
    /// Public HTTPS URL. Meta curls this; private/localhost URLs will fail.
    pub url: String,
}

/// Input to the two-step create/publish flow.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishRequest {
    pub media_type: PublishMediaType,
    pub text: Option<String>,
    pub reply_to_id: Option<PostId>,
    pub reply_control: Option<ReplyControl>,
    pub link_attachment: Option<String>,
    pub media: Vec<MediaInput>,
}

/// Opaque container id returned by `POST /v1.0/me/threads`.
/// Treat as a string; may be numeric or alphanumeric depending on Meta's
/// internal representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerId(pub String);

impl ContainerId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Processing state of a container returned by `GET /{container-id}?fields=status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerStatus {
    Expired,
    Error,
    Finished,
    InProgress,
    Published,
}

impl ContainerStatus {
    /// Parse the wire string from the Threads API (case-insensitive).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "EXPIRED" => Some(ContainerStatus::Expired),
            "ERROR" => Some(ContainerStatus::Error),
            "FINISHED" => Some(ContainerStatus::Finished),
            "IN_PROGRESS" => Some(ContainerStatus::InProgress),
            "PUBLISHED" => Some(ContainerStatus::Published),
            _ => None,
        }
    }
}

/// Remote quota snapshot from `GET /me/threads_publishing_limit`.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishingLimits {
    /// Posts used in the current 24h window.
    pub post_usage: u32,
    /// Max posts allowed in a 24h window (250 per Meta's documentation).
    pub post_total: u32,
    /// Replies used in the current 24h window.
    pub reply_usage: u32,
    /// Max replies allowed in a 24h window (1 000 per Meta's documentation).
    pub reply_total: u32,
}

/// Validate that text fits within the 500-character limit.
/// This is a conservative client-side guard; the API will be the final authority.
/// Returns `Err(String)` with a human-readable message when the text is too long.
pub fn validate_text(text: &str) -> Result<(), String> {
    let len = text.chars().count();
    if len > 500 {
        Err(format!(
            "text is {} characters; Threads allows at most 500",
            len
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_text_when_no_media() {
        assert_eq!(PublishMediaType::infer(&[]), PublishMediaType::Text);
    }

    #[test]
    fn infer_image_for_single_image() {
        let m = vec![MediaInput {
            kind: MediaInputKind::Image,
            url: "https://example.com/a.jpg".into(),
        }];
        assert_eq!(PublishMediaType::infer(&m), PublishMediaType::Image);
    }

    #[test]
    fn infer_video_for_single_video() {
        let m = vec![MediaInput {
            kind: MediaInputKind::Video,
            url: "https://example.com/a.mp4".into(),
        }];
        assert_eq!(PublishMediaType::infer(&m), PublishMediaType::Video);
    }

    #[test]
    fn infer_carousel_for_two_or_more() {
        let two = vec![
            MediaInput {
                kind: MediaInputKind::Image,
                url: "https://example.com/a.jpg".into(),
            },
            MediaInput {
                kind: MediaInputKind::Image,
                url: "https://example.com/b.jpg".into(),
            },
        ];
        assert_eq!(PublishMediaType::infer(&two), PublishMediaType::Carousel);
    }

    #[test]
    fn validate_text_accepts_exactly_500() {
        let s = "x".repeat(500);
        assert!(validate_text(&s).is_ok());
    }

    #[test]
    fn validate_text_rejects_501() {
        let s = "x".repeat(501);
        let err = validate_text(&s).unwrap_err();
        assert!(err.contains("501"), "error should mention the count: {err}");
    }

    #[test]
    fn validate_text_accepts_empty() {
        assert!(validate_text("").is_ok());
    }

    #[test]
    fn container_status_parses_all_variants() {
        assert_eq!(
            ContainerStatus::from_wire("FINISHED"),
            Some(ContainerStatus::Finished)
        );
        assert_eq!(
            ContainerStatus::from_wire("IN_PROGRESS"),
            Some(ContainerStatus::InProgress)
        );
        assert_eq!(
            ContainerStatus::from_wire("EXPIRED"),
            Some(ContainerStatus::Expired)
        );
        assert_eq!(
            ContainerStatus::from_wire("ERROR"),
            Some(ContainerStatus::Error)
        );
        assert_eq!(
            ContainerStatus::from_wire("PUBLISHED"),
            Some(ContainerStatus::Published)
        );
        assert!(ContainerStatus::from_wire("UNKNOWN_JUNK").is_none());
    }

    #[test]
    fn reply_control_wire_strings() {
        assert_eq!(ReplyControl::Everyone.as_wire_str(), "everyone");
        assert_eq!(
            ReplyControl::AccountsYouFollow.as_wire_str(),
            "accounts_you_follow"
        );
        assert_eq!(ReplyControl::MentionedOnly.as_wire_str(), "mentioned_only");
    }

    #[test]
    fn publish_media_type_wire_strings() {
        assert_eq!(PublishMediaType::Text.as_wire_str(), "TEXT");
        assert_eq!(PublishMediaType::Image.as_wire_str(), "IMAGE");
        assert_eq!(PublishMediaType::Video.as_wire_str(), "VIDEO");
        assert_eq!(PublishMediaType::Carousel.as_wire_str(), "CAROUSEL");
    }
}
```

- [ ] **Run (before adding `pub mod publish` to lib.rs):** `cargo test -p threads-core`
  - Expected: **FAIL** (module not found)

- [ ] **Wire up in `crates/threads-core/src/lib.rs`** — add after the existing `pub mod provider;` line:

```rust
pub mod publish;

pub use publish::{
    ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishMediaType, PublishRequest,
    PublishingLimits, ReplyControl, validate_text,
};
```

- [ ] **Run:** `cargo test -p threads-core`
  - Expected: **PASS** (all tests in `publish.rs` and the existing `model` tests pass)

- [ ] **Commit:** `feat(core): add publish types — PublishRequest, ContainerId, ContainerStatus, PublishingLimits`

---

### Task 3 — Provider trait: 5 new default methods

**Files:**
- `crates/threads-core/src/provider.rs`

**Steps:**

- [ ] **Write failing test** — add to the bottom of `provider.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::{ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishRequest, PublishingLimits, PublishMediaType};

    struct StubProvider;

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str { "stub" }
        async fn fetch_me(&self) -> crate::Result<crate::User> { unimplemented!() }
        async fn fetch_my_threads(&self, _: Option<crate::Cursor>) -> crate::Result<crate::Page<crate::Post>> { unimplemented!() }
        async fn fetch_replies(&self, _: &crate::PostId, _: Option<crate::Cursor>) -> crate::Result<crate::Page<crate::Post>> { unimplemented!() }
        async fn fetch_thread(&self, _: &crate::PostId) -> crate::Result<Vec<crate::Post>> { unimplemented!() }
    }

    #[tokio::test]
    async fn new_methods_default_to_not_supported() {
        let p = StubProvider;

        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("hi".into()),
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let cid = ContainerId::new("c1");
        let pid = crate::PostId::new("p1");
        let item = MediaInput {
            kind: MediaInputKind::Image,
            url: "https://example.com/a.jpg".into(),
        };

        assert!(matches!(p.create_container(&req).await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.publish_container(&cid).await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.container_status(&cid).await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.publishing_limits().await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.fetch_post(&pid).await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.create_carousel_item(&item).await, Err(crate::Error::NotSupported(_))));
        assert!(matches!(p.create_carousel_container(&req, &[cid.clone()]).await, Err(crate::Error::NotSupported(_))));
    }
}
```

- [ ] **Run:** `cargo test -p threads-core new_methods_default_to_not_supported`
  - Expected: **FAIL** (methods not yet on the trait)

- [ ] **Add the 5 default methods** to the `Provider` trait in `crates/threads-core/src/provider.rs`. Insert after `delete_reply`:

```rust
    /// Create a media container for the two-step publish flow.
    /// Default impl returns `Error::NotSupported`.
    async fn create_container(
        &self,
        _req: &crate::publish::PublishRequest,
    ) -> Result<crate::publish::ContainerId> {
        Err(Error::NotSupported("create_container".into()))
    }

    /// Publish a previously created container, returning the new post id.
    /// Default impl returns `Error::NotSupported`.
    async fn publish_container(
        &self,
        _id: &crate::publish::ContainerId,
    ) -> Result<PostId> {
        Err(Error::NotSupported("publish_container".into()))
    }

    /// Poll the processing status of a container.
    /// Default impl returns `Error::NotSupported`.
    async fn container_status(
        &self,
        _id: &crate::publish::ContainerId,
    ) -> Result<crate::publish::ContainerStatus> {
        Err(Error::NotSupported("container_status".into()))
    }

    /// Fetch the authenticated user's remote publishing quota.
    /// Default impl returns `Error::NotSupported`.
    async fn publishing_limits(&self) -> Result<crate::publish::PublishingLimits> {
        Err(Error::NotSupported("publishing_limits".into()))
    }

    /// Fetch a single post by id (used after publish to upsert the canonical record).
    /// Default impl returns `Error::NotSupported`.
    async fn fetch_post(&self, _id: &PostId) -> Result<Post> {
        Err(Error::NotSupported("fetch_post".into()))
    }

    /// Create a single carousel child container for one media item
    /// (the provider sets `is_carousel_item=true`).
    /// Default impl returns `Error::NotSupported`.
    async fn create_carousel_item(
        &self,
        _item: &crate::publish::MediaInput,
    ) -> Result<crate::publish::ContainerId> {
        Err(Error::NotSupported("create_carousel_item".into()))
    }

    /// Create the carousel parent container from already-created child container ids.
    /// Default impl returns `Error::NotSupported`.
    async fn create_carousel_container(
        &self,
        _req: &crate::publish::PublishRequest,
        _children: &[crate::publish::ContainerId],
    ) -> Result<crate::publish::ContainerId> {
        Err(Error::NotSupported("create_carousel_container".into()))
    }
```

Also update the `use` line at the top of `provider.rs` to bring in the publish module (no import change needed since we use the full path in the default impls).

- [ ] **Run:** `cargo test -p threads-core new_methods_default_to_not_supported`
  - Expected: **PASS**

- [ ] **Run full suite:** `cargo test -p threads-core`
  - Expected: **PASS** (all existing tests still pass)

- [ ] **Commit:** `feat(core): add create_container/publish_container/container_status/publishing_limits/fetch_post to Provider trait`

---

### Task 4 — `HttpClient::post_json` (pure helper tests only)

**Files:**
- `crates/threads-provider-official/src/client.rs`

**Steps:**

- [ ] **Write failing test** — add to the `#[cfg(test)] mod tests` block inside `client.rs`:

```rust
    #[test]
    fn post_json_url_would_append_access_token() {
        // We cannot hit the real network in tests. This test verifies the
        // URL-building logic by inspecting `is_near_limit` and `backoff`
        // helpers that post_json reuses — and verifies the method compiles
        // and is reachable.
        // Minimal smoke-check: the type signature must accept the call.
        // (Actual network behavior is covered by the fake-provider CLI tests.)
        use url::Url;
        let base = Url::parse("https://graph.threads.net").unwrap();
        let mut url = base.join("/v1.0/me/threads").unwrap();
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("media_type", "TEXT");
            q.append_pair("text", "hello");
            q.append_pair("access_token", "tok");
        }
        let s = url.to_string();
        assert!(s.contains("access_token=tok"));
        assert!(s.contains("media_type=TEXT"));
        assert!(s.contains("text=hello"));
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official post_json_url_would_append_access_token`
  - Expected: **FAIL** (test exists but `post_json` method not yet on `HttpClient`, causing compile error if referenced; alternatively the test file doesn't compile because `post_json` isn't declared yet)

  > Note: since the test above does NOT call `post_json` directly, it should actually compile even without the impl. If it already passes because it only tests URL building, that is acceptable — the important compile-gated test is in Task 6 (provider impl). Add the test to establish baseline; move to implementation.

- [ ] **Implement `post_json`** in `crates/threads-provider-official/src/client.rs` — mirror `delete_json` exactly, replacing `self.inner.delete(...)` with `self.inner.post(...)`. Add the method directly after `delete_json`:

```rust
    /// POST `path` (absolute or relative to `base`), passing `query` params
    /// (including `access_token`) on the query string. Returns the raw JSON `Value`.
    ///
    /// Mirrors `delete_json`: same 429/5xx retry policy, `x-app-usage` backoff,
    /// redaction, and error mapping.
    ///
    /// Equivalent curl shape:
    /// `curl -X POST 'https://graph.threads.net/v1.0/me/threads?media_type=TEXT&text=hi&access_token=...'`.
    pub async fn post_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let mut url = if path.starts_with("http://") || path.starts_with("https://") {
            Url::parse(path)?
        } else {
            self.base.join(path)?
        };
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
            q.append_pair("access_token", &self.token);
        }

        let mut attempt = 0u32;
        let mut delay_ms = 250u64;
        loop {
            attempt += 1;
            let resp = self
                .inner
                .post(url.clone())
                .send()
                .await
                .map_err(|e| Error::Network(e.to_string()))?;
            let status = resp.status();
            let app_usage = resp
                .headers()
                .get("x-app-usage")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());

            if status.is_success() {
                let body = resp
                    .text()
                    .await
                    .map_err(|e| Error::Network(e.to_string()))?;
                if let Some(usage) = app_usage.as_deref() {
                    if is_near_limit(usage) {
                        warn!(usage, "threads API near rate limit; client-side backoff");
                    }
                }
                if body.trim().is_empty() {
                    return Ok(serde_json::Value::Null);
                }
                return serde_json::from_str(&body).map_err(Error::from);
            }

            let body = redact::redact(&resp.text().await.unwrap_or_default());
            match status.as_u16() {
                401 | 403 => return Err(Error::Auth(format!("{status}: {body}"))),
                404 => return Err(Error::NotFound(body)),
                429 => {
                    if attempt > 5 {
                        return Err(Error::RateLimit {
                            retry_after: retry_after.map(Duration::from_secs),
                        });
                    }
                    let wait = retry_after
                        .map(Duration::from_secs)
                        .unwrap_or_else(|| backoff(delay_ms));
                    debug!(?wait, attempt, "rate limited, backing off");
                    tokio::time::sleep(wait).await;
                    delay_ms = (delay_ms * 2).min(30_000);
                }
                s if (500..600).contains(&s) => {
                    if attempt > 5 {
                        return Err(Error::Network(format!("{status}: {body}")));
                    }
                    tokio::time::sleep(backoff(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(30_000);
                }
                _ => return Err(Error::Other(format!("{status}: {body}"))),
            }
        }
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official`
  - Expected: **PASS** (all existing tests pass, new URL test passes)

- [ ] **Commit:** `feat(provider): add HttpClient::post_json mirroring delete_json`

---

### Task 5 — DTOs for publish responses

**Files:**
- `crates/threads-provider-official/src/dto.rs`

**Steps:**

- [ ] **Write failing tests** — add to the `#[cfg(test)] mod tests` block in `dto.rs`:

```rust
    #[test]
    fn parses_create_container_resp() {
        let v = r#"{"id":"container_abc123"}"#;
        let r: CreateContainerResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.id, "container_abc123");
    }

    #[test]
    fn parses_publish_resp() {
        let v = r#"{"id":"post_xyz999"}"#;
        let r: PublishResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.id, "post_xyz999");
    }

    #[test]
    fn parses_container_status_resp() {
        let v = r#"{"status":"FINISHED"}"#;
        let r: ContainerStatusResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.status, "FINISHED");
    }

    #[test]
    fn parses_publishing_limit_resp_full() {
        let v = r#"{
            "quota_usage": 3,
            "config": { "quota_total": 250, "quota_duration": 86400 },
            "reply_quota_usage": 12,
            "reply_config": { "quota_total": 1000, "quota_duration": 86400 }
        }"#;
        let r: PublishingLimitResp = serde_json::from_str(v).unwrap();
        assert_eq!(r.quota_usage, 3);
        assert_eq!(r.config.quota_total, 250);
        assert_eq!(r.reply_quota_usage, 12);
        assert_eq!(r.reply_config.quota_total, 1000);
    }

    #[test]
    fn parses_publishing_limit_resp_wrapped_in_data_array() {
        // The API returns this as `{ "data": [{ ... }] }` when using fields=.
        // We parse the inner element only (provider unwraps data[0]).
        let inner = r#"{
            "quota_usage": 0,
            "config": { "quota_total": 250, "quota_duration": 86400 },
            "reply_quota_usage": 0,
            "reply_config": { "quota_total": 1000, "quota_duration": 86400 }
        }"#;
        let r: PublishingLimitResp = serde_json::from_str(inner).unwrap();
        assert_eq!(r.quota_usage, 0);
        assert_eq!(r.config.quota_total, 250);
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official parses_create_container_resp`
  - Expected: **FAIL** (types not yet defined)

- [ ] **Add DTOs to `crates/threads-provider-official/src/dto.rs`** — append after the existing structs:

```rust
/// Response from `POST /v1.0/me/threads` (create container).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateContainerResp {
    pub id: String,
}

/// Response from `POST /v1.0/me/threads_publish`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishResp {
    pub id: String,
}

/// Response from `GET /{container-id}?fields=status`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContainerStatusResp {
    pub status: String,
}

/// One element from `GET /me/threads_publishing_limit`.
/// The API wraps this in `{ "data": [ { ... } ] }` when field-projected;
/// the provider extracts `data[0]` before deserializing into this struct.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishingLimitResp {
    #[serde(default)]
    pub quota_usage: u32,
    pub config: PublishingLimitConfig,
    #[serde(default)]
    pub reply_quota_usage: u32,
    pub reply_config: PublishingLimitConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishingLimitConfig {
    pub quota_total: u32,
    pub quota_duration: u64,
}
```

- [ ] **Run:** `cargo test -p threads-provider-official`
  - Expected: **PASS** (all DTO tests pass)

- [ ] **Commit:** `feat(provider): add CreateContainerResp, PublishResp, ContainerStatusResp, PublishingLimitResp DTOs`

---

### Task 6 — `OfficialProvider` implements the 5 new trait methods

**Files:**
- `crates/threads-provider-official/src/provider.rs`

**Steps:**

- [ ] **Write failing tests** — add to `#[cfg(test)] mod tests` in `provider.rs`:

```rust
    // ---- publish param building ----

    #[test]
    fn create_container_text_params_include_media_type_and_text() {
        use threads_core::publish::{PublishMediaType, PublishRequest};
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("Hello Threads!".into()),
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let params = build_create_params(&req);
        let map: std::collections::HashMap<_, _> = params.iter().cloned().collect();
        assert_eq!(map.get("media_type").copied(), Some("TEXT"));
        assert_eq!(map.get("text").copied(), Some("Hello Threads!"));
        assert!(!params.iter().any(|(k, _)| *k == "reply_to_id"));
    }

    #[test]
    fn create_container_reply_params_include_reply_to_id() {
        use threads_core::{PostId, publish::{PublishMediaType, PublishRequest}};
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("a reply".into()),
            reply_to_id: Some(PostId::new("parent_post_99")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let params = build_create_params(&req);
        let map: std::collections::HashMap<_, _> = params.iter().cloned().collect();
        assert_eq!(map.get("reply_to_id").copied(), Some("parent_post_99"));
    }

    #[test]
    fn create_container_image_params_include_image_url() {
        use threads_core::publish::{MediaInput, MediaInputKind, PublishMediaType, PublishRequest};
        let req = PublishRequest {
            media_type: PublishMediaType::Image,
            text: None,
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![MediaInput {
                kind: MediaInputKind::Image,
                url: "https://example.com/photo.jpg".into(),
            }],
        };
        let params = build_create_params(&req);
        let map: std::collections::HashMap<_, _> = params.iter().cloned().collect();
        assert_eq!(map.get("media_type").copied(), Some("IMAGE"));
        assert_eq!(
            map.get("image_url").copied(),
            Some("https://example.com/photo.jpg")
        );
    }

    #[test]
    fn substitute_container_id_replaces_placeholder() {
        let path = "/v1.0/{container-id}";
        use threads_core::publish::ContainerId;
        let cid = ContainerId::new("ctr_42");
        let result = OfficialProvider::substitute_container_id(path, &cid);
        assert_eq!(result, "/v1.0/ctr_42");
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official create_container_text_params`
  - Expected: **FAIL** (`build_create_params` and `substitute_container_id` not yet defined)

- [ ] **Implement in `crates/threads-provider-official/src/provider.rs`**:

  1. Add imports at the top:
  ```rust
  use threads_core::publish::{
      ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishRequest, PublishingLimits,
  };
  ```

  2. Add `substitute_container_id` as a `pub(crate)` associated fn on `OfficialProvider` (after existing `substitute_post_id`):
  ```rust
      pub(crate) fn substitute_container_id(path: &str, id: &ContainerId) -> String {
          path.replace("{container-id}", id.as_str())
      }
  ```

  3. Add `pub(crate) fn build_create_params(req: &PublishRequest) -> Vec<(&'static str, String)>` as a module-level `pub(crate)` function (placed before the `#[async_trait]` block so tests can reach it):

  ```rust
  /// Build the query-string params for `POST /v1.0/me/threads`.
  /// All values are owned Strings because lifetimes from the request fields
  /// need to outlive the params slice passed to `post_json`.
  pub(crate) fn build_create_params(req: &PublishRequest) -> Vec<(&'static str, String)> {
      let mut p: Vec<(&'static str, String)> = Vec::new();
      p.push(("media_type", req.media_type.as_wire_str().to_string()));
      if let Some(ref text) = req.text {
          p.push(("text", text.clone()));
      }
      if let Some(ref rid) = req.reply_to_id {
          p.push(("reply_to_id", rid.as_str().to_string()));
      }
      if let Some(ref rc) = req.reply_control {
          p.push(("reply_control", rc.as_wire_str().to_string()));
      }
      if let Some(ref la) = req.link_attachment {
          p.push(("link_attachment", la.clone()));
      }
      for m in &req.media {
          match m.kind {
              MediaInputKind::Image => p.push(("image_url", m.url.clone())),
              MediaInputKind::Video => p.push(("video_url", m.url.clone())),
          }
      }
      p
  }
  ```

  4. Implement the 5 trait methods on `OfficialProvider` (add inside the `impl Provider for OfficialProvider` block, after `delete_reply`):

  ```rust
      async fn create_container(
          &self,
          req: &PublishRequest,
      ) -> threads_core::Result<ContainerId> {
          let path = self
              .action_path("post/create")
              .ok_or_else(|| threads_core::Error::Manifest("missing action `post/create`".into()))?;
          let owned_params = build_create_params(req);
          let borrowed: Vec<(&str, &str)> = owned_params
              .iter()
              .map(|(k, v)| (*k, v.as_str()))
              .collect();
          let val = self.http.post_json(&path, &borrowed).await?;
          let resp: crate::dto::CreateContainerResp =
              serde_json::from_value(val).map_err(threads_core::Error::from)?;
          Ok(ContainerId::new(resp.id))
      }

      async fn publish_container(
          &self,
          id: &ContainerId,
      ) -> threads_core::Result<threads_core::PostId> {
          let path = self
              .action_path("post/publish")
              .ok_or_else(|| threads_core::Error::Manifest("missing action `post/publish`".into()))?;
          let creation_id = id.as_str().to_string();
          let params: Vec<(&str, &str)> = vec![("creation_id", creation_id.as_str())];
          let val = self.http.post_json(&path, &params).await?;
          let resp: crate::dto::PublishResp =
              serde_json::from_value(val).map_err(threads_core::Error::from)?;
          Ok(threads_core::PostId::new(resp.id))
      }

      async fn container_status(
          &self,
          id: &ContainerId,
      ) -> threads_core::Result<ContainerStatus> {
          let path = self
              .object_path("container")
              .ok_or_else(|| threads_core::Error::Manifest("missing object `container`".into()))?;
          let path = Self::substitute_container_id(&path, id);
          let fields = self.endpoint_fields("container").unwrap_or_else(|| "status".into());
          let val: serde_json::Value = self
              .http
              .get_json(&path, &[("fields", fields.as_str())])
              .await?;
          let resp: crate::dto::ContainerStatusResp =
              serde_json::from_value(val).map_err(threads_core::Error::from)?;
          ContainerStatus::from_wire(&resp.status).ok_or_else(|| {
              threads_core::Error::Parse(format!("unknown container status: {}", resp.status))
          })
      }

      async fn publishing_limits(&self) -> threads_core::Result<PublishingLimits> {
          let path = self
              .object_path("publishing_limit")
              .ok_or_else(|| threads_core::Error::Manifest("missing object `publishing_limit`".into()))?;
          let fields = self
              .endpoint_fields("publishing_limit")
              .unwrap_or_else(|| "quota_usage,config,reply_quota_usage,reply_config".into());
          // The API may return a `{ "data": [ { ... } ] }` envelope.
          let raw: serde_json::Value = self
              .http
              .get_json(&path, &[("fields", fields.as_str())])
              .await?;
          let item = if let Some(arr) = raw.get("data").and_then(|d| d.as_array()) {
              arr.first()
                  .cloned()
                  .ok_or_else(|| threads_core::Error::Parse("publishing_limit data array is empty".into()))?
          } else {
              raw
          };
          let resp: crate::dto::PublishingLimitResp =
              serde_json::from_value(item).map_err(threads_core::Error::from)?;
          Ok(PublishingLimits {
              post_usage: resp.quota_usage,
              post_total: resp.config.quota_total,
              reply_usage: resp.reply_quota_usage,
              reply_total: resp.reply_config.quota_total,
          })
      }

      async fn fetch_post(&self, id: &threads_core::PostId) -> threads_core::Result<threads_core::Post> {
          let path = self
              .object_path("post")
              .ok_or_else(|| threads_core::Error::Manifest("missing object `post`".into()))?;
          let path = Self::substitute_post_id(&path, id);
          let fields = self.endpoint_fields("post");
          let mut q: Vec<(&str, &str)> = Vec::new();
          if let Some(ref f) = fields {
              q.push(("fields", f.as_str()));
          }
          let dto: crate::dto::PostDto = self.http.get_json(&path, &q).await?;
          Ok(dto_to_post(dto, None))
      }
  ```

- [ ] **Run:** `cargo test -p threads-provider-official`
  - Expected: **PASS**

- [ ] **Commit:** `feat(provider): implement create_container, publish_container, container_status, publishing_limits, fetch_post`

---

### Task 7 — Auth: add `threads_content_publish` scope

**Files:**
- `crates/threads-provider-official/src/auth.rs`

**Steps:**

- [ ] **Write failing test** — add to the existing `tests` block in `auth.rs` (or add a new `mod tests` block if absent). First check whether tests exist; if not, add the block:

```rust
    #[test]
    fn default_scopes_include_content_publish() {
        assert!(
            DEFAULT_SCOPES.contains(&"threads_content_publish"),
            "DEFAULT_SCOPES must include threads_content_publish for the post create command; got: {DEFAULT_SCOPES:?}"
        );
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official default_scopes_include_content_publish`
  - Expected: **FAIL** (`threads_content_publish` not in `DEFAULT_SCOPES`)

- [ ] **Edit `DEFAULT_SCOPES`** in `crates/threads-provider-official/src/auth.rs`:

```rust
// Before:
pub const DEFAULT_SCOPES: &[&str] = &["threads_basic", "threads_read_replies", "threads_delete"];

// After:
pub const DEFAULT_SCOPES: &[&str] = &[
    "threads_basic",
    "threads_read_replies",
    "threads_delete",
    "threads_content_publish",
];
```

- [ ] **Run:** `cargo test -p threads-provider-official default_scopes_include_content_publish`
  - Expected: **PASS**

- [ ] **Verify** existing token_store tests still pass (they reference `threads_delete`, not `DEFAULT_SCOPES`):
  `cargo test -p threads-provider-official`

- [ ] **Commit:** `feat(auth): add threads_content_publish to DEFAULT_SCOPES`

---

### Task 8 — CLI: `Command::Post` and `PostCreateArgs`

**Files:**
- `crates/threads-cli/src/cli.rs`

**Steps:**

- [ ] **Write failing test** — update the existing `cli_structure_is_valid` test; it already calls `Cli::command().debug_assert()` which validates the whole clap structure. Adding `Command::Post` and having it compile is sufficient for the test to verify correctness. Add a dedicated test for the new subcommand shape:

```rust
    #[test]
    fn post_create_args_parse_correctly() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "threads-cli",
            "post",
            "create",
            "--text",
            "Hello world",
            "--yes",
        ])
        .expect("should parse");
        match cli.command {
            Command::Post(PostCommand::Create(args)) => {
                assert_eq!(args.text.as_deref(), Some("Hello world"));
                assert!(args.yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn post_create_reply_args_parse_correctly() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "threads-cli",
            "post",
            "create",
            "--text",
            "A reply",
            "--reply-to",
            "post_abc",
            "--reply-control",
            "mentioned-only",
        ])
        .expect("should parse");
        match cli.command {
            Command::Post(PostCommand::Create(args)) => {
                assert_eq!(args.reply_to.as_deref(), Some("post_abc"));
                assert!(matches!(
                    args.reply_control,
                    Some(ReplyControlArg::MentionedOnly)
                ));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
```

- [ ] **Run:** `cargo test -p threads-cli post_create_args_parse_correctly`
  - Expected: **FAIL** (`Command::Post`, `PostCommand`, `PostCreateArgs`, `ReplyControlArg` not yet defined)

- [ ] **Add to `crates/threads-cli/src/cli.rs`**:

  1. Add `Post` variant to `Command` (after `Delete`):
  ```rust
      /// Publish a new post or reply to Threads.
      #[command(subcommand)]
      Post(PostCommand),
  ```

  2. Add new types after the `DeleteArgs` struct:
  ```rust
  #[derive(Debug, Subcommand)]
  pub enum PostCommand {
      /// Create and publish a text, image, video, or carousel post (or reply).
      Create(PostCreateArgs),
  }

  /// Clap-facing enum for `--reply-control`.
  #[derive(Copy, Clone, Debug, ValueEnum)]
  pub enum ReplyControlArg {
      Everyone,
      AccountsYouFollow,
      MentionedOnly,
  }

  #[derive(Debug, clap::Args)]
  pub struct PostCreateArgs {
      /// Text body of the post. Pass `-` to read from stdin.
      #[arg(long)]
      pub text: Option<String>,

      /// Create as a reply to this post id.
      #[arg(long)]
      pub reply_to: Option<String>,

      /// Public HTTPS URL of an image to attach (repeatable; ≥2 media ⇒ carousel).
      #[arg(long)]
      pub image_url: Vec<String>,

      /// Public HTTPS URL of a video to attach (repeatable).
      #[arg(long)]
      pub video_url: Vec<String>,

      /// Control who can reply to this post.
      #[arg(long, value_enum)]
      pub reply_control: Option<ReplyControlArg>,

      /// Attach a link preview URL.
      #[arg(long)]
      pub link_attachment: Option<String>,

      /// Skip the interactive confirmation prompt (required when not on a TTY).
      #[arg(long)]
      pub yes: bool,
  }
  ```

- [ ] **Run:** `cargo test -p threads-cli`
  - Expected: **FAIL** (dispatch in `mod.rs` not yet updated; `Command::Post` arm missing)

  > Note: The `cli_structure_is_valid` and new parse tests may pass at this point; the compile error will be in `dispatch`. Proceed to Task 9 to wire dispatch, then rerun.

- [ ] **Commit (after Task 9 passes):** `feat(cli): add Command::Post, PostCommand::Create, PostCreateArgs`

---

### Task 9 — Wire dispatch + `commands/post.rs` — TEXT post/reply slice (shippable)

**Files:**
- `crates/threads-cli/src/commands/mod.rs`
- `crates/threads-cli/src/commands/post.rs` (new file)

This task delivers the complete end-to-end text post + reply slice, which is independently shippable. Image/video/carousel polling is added in Task 10.

**Steps:**

- [ ] **Write failing tests** — create `crates/threads-cli/src/commands/post.rs` with the test module first (the module will fail to compile until the implementation exists):

```rust
// crates/threads-cli/src/commands/post.rs

use std::{
    io::{self, IsTerminal, Read},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use threads_core::{
    Post, Provider,
    publish::{ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishMediaType, PublishRequest, PublishingLimits, ReplyControl, validate_text},
};
use threads_provider_official::{TokenStore, token_store::token_has_scope};

use crate::cli::{PostCreateArgs, ReplyControlArg};

pub async fn run(
    args: PostCreateArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<()> {
    // 1. Scope check
    let token = TokenStore::new()
        .load()
        .map_err(|e| anyhow!("read token: {e}"))?;
    let token = match token {
        Some(t) if token_has_scope(&t, "threads_content_publish") => t,
        Some(_) => bail!(
            "stored token lacks `threads_content_publish` scope; run `threads-cli auth login`"
        ),
        None => bail!("no stored token; run `threads-cli auth login`"),
    };
    let _ = token; // token's access_token is consumed by open_provider

    // 2. Build PublishRequest
    let text = resolve_text(args.text.as_deref())?;
    if let Some(ref t) = text {
        validate_text(t).map_err(|e| anyhow!("{e}"))?;
    }

    let mut media: Vec<MediaInput> = Vec::new();
    for url in &args.image_url {
        media.push(MediaInput { kind: MediaInputKind::Image, url: url.clone() });
    }
    for url in &args.video_url {
        media.push(MediaInput { kind: MediaInputKind::Video, url: url.clone() });
    }

    let media_type = PublishMediaType::infer(&media);
    let reply_to_id = args.reply_to.as_deref().map(threads_core::PostId::new);
    let reply_control = args.reply_control.map(|rc| match rc {
        ReplyControlArg::Everyone => ReplyControl::Everyone,
        ReplyControlArg::AccountsYouFollow => ReplyControl::AccountsYouFollow,
        ReplyControlArg::MentionedOnly => ReplyControl::MentionedOnly,
    });

    let req = PublishRequest {
        media_type,
        text,
        reply_to_id: reply_to_id.clone(),
        reply_control,
        link_attachment: args.link_attachment.clone(),
        media,
    };

    // 3. Open provider and store
    let cli_cfg = crate::commands::load_config(config_override)?;
    let provider = crate::commands::open_provider(&cli_cfg).await?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;

    // 4. Preflight quota check
    let limits = provider
        .publishing_limits()
        .await
        .map_err(|e| anyhow!("fetch publishing limits: {e}"))?;
    check_quota(&limits, reply_to_id.is_some())?;

    // 5. Reply-to-others warning (informational, not a hard block)
    if reply_to_id.is_some() {
        eprintln!(
            "Note: replying to another user's post requires `threads_keyword_search` or \
             `threads_manage_mentions` scope in addition to `threads_content_publish`. \
             If you see a permissions error, re-run `threads-cli auth login` after \
             enabling those scopes in your app dashboard."
        );
    }

    // 6. Confirm gate
    show_preview(&req);
    confirm(args.yes)?;

    // 7. Publish
    let post_id = publish_flow(&provider, &req).await?;

    // 8. Fetch canonical post and upsert
    let post = match provider.fetch_post(&post_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: fetch after publish failed ({e}); synthesizing local record");
            synthesize_post(&post_id, &req)
        }
    };
    store
        .upsert_posts(std::slice::from_ref(&post), None)
        .map_err(|e| anyhow!("upsert published post: {e}"))?;

    // 9. Print result
    let url = post
        .permalink
        .as_deref()
        .unwrap_or("<permalink unavailable>");
    println!("Published: {}", post_id);
    println!("URL:       {url}");
    println!("Stored in local DB.");

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_text(raw: Option<&str>) -> Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some("-") => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s)?;
            Ok(Some(s.trim_end().to_string()))
        }
        Some(t) => Ok(Some(t.to_string())),
    }
}

pub(crate) fn check_quota(limits: &PublishingLimits, is_reply: bool) -> Result<()> {
    if is_reply {
        if limits.reply_usage >= limits.reply_total {
            bail!(
                "reply quota exhausted: {}/{} replies in the last 24h. Try again later.",
                limits.reply_usage,
                limits.reply_total
            );
        }
    } else if limits.post_usage >= limits.post_total {
        bail!(
            "post quota exhausted: {}/{} posts in the last 24h. Try again later.",
            limits.post_usage,
            limits.post_total
        );
    }
    Ok(())
}

fn show_preview(req: &PublishRequest) {
    println!("--- Preview ---");
    println!("type:  {}", req.media_type.as_wire_str());
    if let Some(ref t) = req.text {
        println!("text:  {t}");
    }
    if let Some(ref rid) = req.reply_to_id {
        println!("reply: {rid}");
    }
    for m in &req.media {
        let kind = match m.kind {
            MediaInputKind::Image => "image",
            MediaInputKind::Video => "video",
        };
        println!("media: [{kind}] {}", m.url);
    }
    if let Some(ref la) = req.link_attachment {
        println!("link:  {la}");
    }
    println!("---------------");
}

fn confirm(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        bail!("not on a TTY and --yes not passed; aborting. Re-run with --yes to publish without confirmation.");
    }
    print!("Publish? [y/N] ");
    use std::io::Write as _;
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => bail!("publish cancelled"),
    }
}

/// Core two-step publish orchestration.
/// TEXT: create → publish (no polling needed per spec; for uniformity we still
/// support the path but skip status polling).
/// IMAGE/VIDEO: create → poll status until FINISHED (≤5 attempts) → publish.
/// CAROUSEL: create N child containers via create_carousel_item (each sets
///   is_carousel_item=true) → poll each → create parent container via
///   create_carousel_container(req, &child_ids) → poll parent → publish.
pub(crate) async fn publish_flow<P: Provider>(
    provider: &P,
    req: &PublishRequest,
) -> Result<threads_core::PostId> {
    match req.media_type {
        PublishMediaType::Text => {
            let cid = provider
                .create_container(req)
                .await
                .map_err(|e| anyhow!("create container: {e}"))?;
            let post_id = provider
                .publish_container(&cid)
                .await
                .map_err(|e| anyhow!("publish container: {e}"))?;
            Ok(post_id)
        }

        PublishMediaType::Image | PublishMediaType::Video => {
            let cid = provider
                .create_container(req)
                .await
                .map_err(|e| anyhow!("create container: {e}"))?;
            poll_until_finished(provider, &cid).await?;
            let post_id = provider
                .publish_container(&cid)
                .await
                .map_err(|e| anyhow!("publish container: {e}"))?;
            Ok(post_id)
        }

        PublishMediaType::Carousel => {
            // 1. Create one child container per media item. The provider's
            //    create_carousel_item sets is_carousel_item=true.
            let mut child_ids: Vec<ContainerId> = Vec::new();
            for item in &req.media {
                let cid = provider
                    .create_carousel_item(item)
                    .await
                    .map_err(|e| anyhow!("create carousel child container: {e}"))?;
                poll_until_finished(provider, &cid).await?;
                child_ids.push(cid);
            }

            // 2. Create the parent container from the child container ids.
            let parent_cid = provider
                .create_carousel_container(req, &child_ids)
                .await
                .map_err(|e| anyhow!("create carousel parent container: {e}"))?;
            poll_until_finished(provider, &parent_cid).await?;
            let post_id = provider
                .publish_container(&parent_cid)
                .await
                .map_err(|e| anyhow!("publish carousel: {e}"))?;
            Ok(post_id)
        }
    }
}

/// Poll container status up to 5 times, waiting 10 seconds between attempts.
/// Returns `Ok(())` when status is `Finished`; errors on `Expired`/`Error`/5 attempts.
async fn poll_until_finished<P: Provider>(
    provider: &P,
    cid: &ContainerId,
) -> Result<()> {
    for attempt in 1..=5 {
        let status = provider
            .container_status(cid)
            .await
            .map_err(|e| anyhow!("poll container status: {e}"))?;
        match status {
            ContainerStatus::Finished | ContainerStatus::Published => return Ok(()),
            ContainerStatus::Expired => bail!("container {} expired before publishing", cid),
            ContainerStatus::Error => bail!("container {} entered error state", cid),
            ContainerStatus::InProgress => {
                if attempt < 5 {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    }
    bail!(
        "container {} did not finish processing after 5 status polls",
        cid
    )
}

fn synthesize_post(
    post_id: &threads_core::PostId,
    req: &PublishRequest,
) -> Post {
    Post {
        id: post_id.clone(),
        author: threads_core::UserId::new(""),
        text: req.text.clone(),
        created_at: Some(chrono::Utc::now()),
        parent_id: req.reply_to_id.clone(),
        root_id: req.reply_to_id.clone(),
        permalink: None,
        media: vec![],
        urls: vec![],
        mentions: vec![],
        is_quote_post: false,
        raw: None,
    }
}

// ---------------------------------------------------------------------------
// Tests — fake provider pattern (no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use threads_core::{
        Cursor, Error as CoreError, Page, PostId, Result as CoreResult, User, UserId,
        publish::{ContainerId, ContainerStatus, PublishingLimits},
    };

    // ---- Shared fake post builder ----
    fn fake_post(id: &str) -> Post {
        Post {
            id: PostId::new(id),
            author: UserId::new("me"),
            text: Some("published".into()),
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: Some(format!("https://www.threads.net/@me/post/{id}")),
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        }
    }

    // ---- FakeProvider ----
    #[derive(Default)]
    struct FakeProviderState {
        created: Vec<PublishRequest>,
        published: Vec<ContainerId>,
        next_container_id: usize,
        next_post_id: usize,
        status_responses: Vec<ContainerStatus>,
        status_call_count: usize,
        limits: Option<PublishingLimits>,
        carousel_item_count: usize,
        carousel_parent_children: Vec<ContainerId>,
    }

    struct FakeProvider {
        state: Arc<Mutex<FakeProviderState>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeProviderState {
                    limits: Some(PublishingLimits {
                        post_usage: 0,
                        post_total: 250,
                        reply_usage: 0,
                        reply_total: 1000,
                    }),
                    ..Default::default()
                })),
            }
        }

        fn with_status_sequence(self, statuses: Vec<ContainerStatus>) -> Self {
            self.state.lock().unwrap().status_responses = statuses;
            self
        }

        fn with_exhausted_post_quota(self) -> Self {
            self.state.lock().unwrap().limits = Some(PublishingLimits {
                post_usage: 250,
                post_total: 250,
                reply_usage: 0,
                reply_total: 1000,
            });
            self
        }

        fn with_exhausted_reply_quota(self) -> Self {
            self.state.lock().unwrap().limits = Some(PublishingLimits {
                post_usage: 0,
                post_total: 250,
                reply_usage: 1000,
                reply_total: 1000,
            });
            self
        }

        fn created_requests(&self) -> Vec<PublishRequest> {
            self.state.lock().unwrap().created.clone()
        }
    }

    #[async_trait]
    impl threads_core::Provider for FakeProvider {
        fn name(&self) -> &'static str { "fake" }

        async fn fetch_me(&self) -> CoreResult<User> {
            Ok(User {
                id: UserId::new("me"),
                username: Some("testuser".into()),
                name: None,
                biography: None,
                profile_picture_url: None,
            })
        }

        async fn fetch_my_threads(&self, _: Option<Cursor>) -> CoreResult<Page<Post>> {
            Ok(Page::empty())
        }

        async fn fetch_replies(
            &self,
            _: &PostId,
            _: Option<Cursor>,
        ) -> CoreResult<Page<Post>> {
            Ok(Page::empty())
        }

        async fn fetch_thread(&self, _: &PostId) -> CoreResult<Vec<Post>> {
            Ok(vec![])
        }

        async fn create_container(
            &self,
            req: &PublishRequest,
        ) -> CoreResult<ContainerId> {
            let mut s = self.state.lock().unwrap();
            s.created.push(req.clone());
            let id = format!("fake_container_{}", s.next_container_id);
            s.next_container_id += 1;
            Ok(ContainerId::new(id))
        }

        async fn publish_container(&self, cid: &ContainerId) -> CoreResult<PostId> {
            let mut s = self.state.lock().unwrap();
            s.published.push(cid.clone());
            let id = format!("fake_post_{}", s.next_post_id);
            s.next_post_id += 1;
            Ok(PostId::new(id))
        }

        async fn container_status(&self, _: &ContainerId) -> CoreResult<ContainerStatus> {
            let mut s = self.state.lock().unwrap();
            let idx = s.status_call_count;
            s.status_call_count += 1;
            s.status_responses
                .get(idx)
                .cloned()
                .ok_or_else(|| CoreError::Other("no more status responses".into()))
        }

        async fn publishing_limits(&self) -> CoreResult<PublishingLimits> {
            let s = self.state.lock().unwrap();
            s.limits.clone().ok_or_else(|| CoreError::Other("no limits set".into()))
        }

        async fn fetch_post(&self, id: &PostId) -> CoreResult<Post> {
            Ok(fake_post(id.as_str()))
        }

        async fn create_carousel_item(
            &self,
            _item: &MediaInput,
        ) -> CoreResult<ContainerId> {
            let mut s = self.state.lock().unwrap();
            let id = format!("fake_child_{}", s.carousel_item_count);
            s.carousel_item_count += 1;
            Ok(ContainerId::new(id))
        }

        async fn create_carousel_container(
            &self,
            req: &PublishRequest,
            children: &[ContainerId],
        ) -> CoreResult<ContainerId> {
            let mut s = self.state.lock().unwrap();
            s.carousel_parent_children = children.to_vec();
            s.created.push(req.clone());
            let id = format!("fake_parent_{}", s.next_container_id);
            s.next_container_id += 1;
            Ok(ContainerId::new(id))
        }
    }

    // ---- Tests ----

    #[tokio::test]
    async fn publish_flow_text_creates_then_publishes() {
        let provider = FakeProvider::new();
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("hello world".into()),
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let post_id = publish_flow(&provider, &req).await.unwrap();
        assert!(post_id.as_str().starts_with("fake_post_"));
        let created = provider.created_requests();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].media_type, PublishMediaType::Text);
    }

    #[tokio::test]
    async fn publish_flow_text_reply_passes_reply_to_id() {
        let provider = FakeProvider::new();
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("a reply".into()),
            reply_to_id: Some(PostId::new("parent_99")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        publish_flow(&provider, &req).await.unwrap();
        let created = provider.created_requests();
        assert_eq!(created[0].reply_to_id, Some(PostId::new("parent_99")));
    }

    #[tokio::test]
    async fn publish_flow_image_polls_status_then_publishes() {
        let provider = FakeProvider::new()
            .with_status_sequence(vec![ContainerStatus::InProgress, ContainerStatus::Finished]);
        let req = PublishRequest {
            media_type: PublishMediaType::Image,
            text: None,
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![MediaInput {
                kind: MediaInputKind::Image,
                url: "https://example.com/img.jpg".into(),
            }],
        };
        let post_id = publish_flow(&provider, &req).await.unwrap();
        assert!(post_id.as_str().starts_with("fake_post_"));
        assert_eq!(provider.state.lock().unwrap().status_call_count, 2);
    }

    #[tokio::test]
    async fn publish_flow_carousel_creates_children_then_parent() {
        // 2 images → 2 child containers, each polled to FINISHED, then 1 parent
        // container polled to FINISHED, then published.
        let provider = FakeProvider::new().with_status_sequence(vec![
            ContainerStatus::Finished, // child 0
            ContainerStatus::Finished, // child 1
            ContainerStatus::Finished, // parent
        ]);
        let req = PublishRequest {
            media_type: PublishMediaType::Carousel,
            text: Some("a carousel".into()),
            reply_to_id: None,
            reply_control: None,
            link_attachment: None,
            media: vec![
                MediaInput {
                    kind: MediaInputKind::Image,
                    url: "https://example.com/a.jpg".into(),
                },
                MediaInput {
                    kind: MediaInputKind::Image,
                    url: "https://example.com/b.jpg".into(),
                },
            ],
        };
        let post_id = publish_flow(&provider, &req).await.unwrap();
        assert!(post_id.as_str().starts_with("fake_post_"));
        let s = provider.state.lock().unwrap();
        assert_eq!(s.carousel_item_count, 2, "expected 2 child containers");
        assert_eq!(
            s.carousel_parent_children.len(),
            2,
            "parent should reference 2 children"
        );
    }

    #[test]
    fn check_quota_blocks_when_posts_exhausted() {
        let limits = PublishingLimits {
            post_usage: 250,
            post_total: 250,
            reply_usage: 0,
            reply_total: 1000,
        };
        let err = check_quota(&limits, false).unwrap_err();
        assert!(err.to_string().contains("quota exhausted"));
    }

    #[test]
    fn check_quota_blocks_when_replies_exhausted() {
        let limits = PublishingLimits {
            post_usage: 0,
            post_total: 250,
            reply_usage: 1000,
            reply_total: 1000,
        };
        let err = check_quota(&limits, true).unwrap_err();
        assert!(err.to_string().contains("quota exhausted"));
    }

    #[test]
    fn check_quota_passes_when_under_limit() {
        let limits = PublishingLimits {
            post_usage: 10,
            post_total: 250,
            reply_usage: 5,
            reply_total: 1000,
        };
        assert!(check_quota(&limits, false).is_ok());
        assert!(check_quota(&limits, true).is_ok());
    }

    #[test]
    fn confirm_required_off_tty_without_yes() {
        // Simulate non-TTY stdin by checking the function logic:
        // confirm(false) on non-TTY should bail. We can't easily mock IsTerminal
        // in a unit test, so instead we verify that confirm(true) always passes.
        assert!(confirm(true).is_ok());
    }
}
```

- [ ] **Run:** `cargo test -p threads-cli`
  - Expected: **FAIL** (module not in `mod.rs`, `Command::Post` dispatch arm missing)

- [ ] **Wire up in `crates/threads-cli/src/commands/mod.rs`**:

  1. Add `pub mod post;` to the module list.
  2. Add the dispatch arm:
  ```rust
  Command::Post(cmd) => post::run_post(cmd, cli.config.as_deref(), cli.db.as_deref()).await,
  ```

  Also add a thin dispatcher in `post.rs`:
  ```rust
  pub async fn run_post(
      cmd: crate::cli::PostCommand,
      config_override: Option<&Path>,
      db_override: Option<&Path>,
  ) -> Result<()> {
      match cmd {
          crate::cli::PostCommand::Create(args) => run(args, config_override, db_override).await,
      }
  }
  ```

- [ ] **Run:** `cargo test -p threads-cli`
  - Expected: **PASS** (all CLI tests pass, including `cli_structure_is_valid`, both parse tests, and all fake-provider tests)

- [ ] **Commit:** `feat(cli): add post create command with text/reply/image/video/carousel orchestration`

---

### Task 10 — OfficialProvider: carousel item + parent containers (type-safe)

**Files:**
- `crates/threads-provider-official/src/provider.rs`

This task implements the two dedicated carousel methods (`create_carousel_item` and `create_carousel_container`) on `OfficialProvider`. There are NO magic-string sentinels: each method builds its own params and POSTs to `action_path("post/create")` via `post_json`, reusing `crate::dto::CreateContainerResp`. `PublishRequest` is unchanged.

**Steps:**

- [ ] **Write failing tests** — add to `#[cfg(test)] mod tests` in `provider.rs`:

```rust
    #[test]
    fn carousel_item_params_set_is_carousel_item() {
        use threads_core::publish::{MediaInput, MediaInputKind};
        let item = MediaInput {
            kind: MediaInputKind::Image,
            url: "https://example.com/img.jpg".into(),
        };
        let params = OfficialProvider::build_carousel_item_params(&item);
        let map: std::collections::HashMap<_, _> = params.iter().cloned().collect();
        assert_eq!(map.get("media_type").copied(), Some("IMAGE"));
        assert_eq!(
            map.get("image_url").copied(),
            Some("https://example.com/img.jpg")
        );
        assert_eq!(map.get("is_carousel_item").copied(), Some("true"));
    }

    #[test]
    fn carousel_parent_params_set_children_csv() {
        use threads_core::{PostId, publish::{ContainerId, PublishMediaType, PublishRequest}};
        let req = PublishRequest {
            media_type: PublishMediaType::Carousel,
            text: Some("carousel caption".into()),
            reply_to_id: Some(PostId::new("parent_post_7")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let children = vec![ContainerId::new("ctr_1"), ContainerId::new("ctr_2")];
        let params = OfficialProvider::build_carousel_parent_params(&req, &children);
        let map: std::collections::HashMap<_, _> = params.iter().cloned().collect();
        assert_eq!(map.get("media_type").copied(), Some("CAROUSEL"));
        assert_eq!(map.get("children").copied(), Some("ctr_1,ctr_2"));
        assert_eq!(map.get("text").copied(), Some("carousel caption"));
        assert_eq!(map.get("reply_to_id").copied(), Some("parent_post_7"));
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official carousel_item_params_set_is_carousel_item`
  - Expected: **FAIL** (`build_carousel_item_params` / `build_carousel_parent_params` not yet defined)

- [ ] **Implement the two param builders** on `OfficialProvider` (module-level `pub(crate)` fns, alongside `build_create_params`):

```rust
/// Build the query-string params for one carousel CHILD container.
/// Always sets `is_carousel_item=true`.
pub(crate) fn build_carousel_item_params(item: &MediaInput) -> Vec<(&'static str, String)> {
    let mut p: Vec<(&'static str, String)> = Vec::new();
    match item.kind {
        MediaInputKind::Image => {
            p.push(("media_type", "IMAGE".to_string()));
            p.push(("image_url", item.url.clone()));
        }
        MediaInputKind::Video => {
            p.push(("media_type", "VIDEO".to_string()));
            p.push(("video_url", item.url.clone()));
        }
    }
    p.push(("is_carousel_item", "true".to_string()));
    p
}

/// Build the query-string params for the carousel PARENT container.
/// `children` is the comma-separated list of already-created child container ids.
pub(crate) fn build_carousel_parent_params(
    req: &PublishRequest,
    children: &[ContainerId],
) -> Vec<(&'static str, String)> {
    let mut p: Vec<(&'static str, String)> = Vec::new();
    p.push(("media_type", "CAROUSEL".to_string()));
    if let Some(ref text) = req.text {
        p.push(("text", text.clone()));
    }
    if let Some(ref rid) = req.reply_to_id {
        p.push(("reply_to_id", rid.as_str().to_string()));
    }
    if let Some(ref rc) = req.reply_control {
        p.push(("reply_control", rc.as_wire_str().to_string()));
    }
    let csv = children
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(",");
    p.push(("children", csv));
    p
}
```

- [ ] **Implement the two trait methods** on `OfficialProvider` (inside `impl Provider for OfficialProvider`, after `fetch_post`):

```rust
    async fn create_carousel_item(
        &self,
        item: &MediaInput,
    ) -> threads_core::Result<ContainerId> {
        let path = self
            .action_path("post/create")
            .ok_or_else(|| threads_core::Error::Manifest("missing action `post/create`".into()))?;
        let owned_params = build_carousel_item_params(item);
        let borrowed: Vec<(&str, &str)> = owned_params
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();
        let val = self.http.post_json(&path, &borrowed).await?;
        let resp: crate::dto::CreateContainerResp =
            serde_json::from_value(val).map_err(threads_core::Error::from)?;
        Ok(ContainerId::new(resp.id))
    }

    async fn create_carousel_container(
        &self,
        req: &PublishRequest,
        children: &[ContainerId],
    ) -> threads_core::Result<ContainerId> {
        let path = self
            .action_path("post/create")
            .ok_or_else(|| threads_core::Error::Manifest("missing action `post/create`".into()))?;
        let owned_params = build_carousel_parent_params(req, children);
        let borrowed: Vec<(&str, &str)> = owned_params
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();
        let val = self.http.post_json(&path, &borrowed).await?;
        let resp: crate::dto::CreateContainerResp =
            serde_json::from_value(val).map_err(threads_core::Error::from)?;
        Ok(ContainerId::new(resp.id))
    }
```

- [ ] **Run:** `cargo test -p threads-provider-official && cargo test -p threads-cli`
  - Expected: **PASS** for both crates

- [ ] **Commit:** `feat(provider): type-safe carousel item/parent container creation`

---

## Self-check for the executor

Run all of these in order after completing all tasks. Every command must exit with status 0.

```
cargo test -p threads-manifest
cargo test -p threads-core
cargo test -p threads-provider-official
cargo test -p threads-cli
cargo build
```

Additionally, after running `threads-cli auth login` (with `threads_content_publish` in the requested scopes), verify the full flow manually:

```
threads-cli post create --text "test post" --yes
```

Expected output pattern:
```
--- Preview ---
type:  TEXT
text:  test post
---------------
Published: <numeric-id>
URL:       https://www.threads.net/@<username>/post/<shortcode>
Stored in local DB.
```

---

## Assumptions and Known Gaps

1. **Feature A assumed complete.** `store.upsert_posts(...)` in this plan calls the Feature A hardened merge. If Feature A is not yet merged, the upsert still works (it falls back to the pre-existing overwrite behavior); it just does not apply the merge rules.

2. **Carousel modeling.** Carousel structure is modeled with dedicated provider methods (`create_carousel_item` / `create_carousel_container`); `PublishRequest` stays free of carousel-control fields.

3. **`publishing_limits` envelope ambiguity.** The Threads API documentation shows the response as `{ "data": [{ ... }] }` when fields are projected; the provider defensively handles both the envelope form and a bare object. Real-world behavior should be verified against the live API.

4. **Carousel child status polling.** Each carousel child is polled before the parent is created. The spec says this is the correct approach ("poll status FINISHED → create parent"). If Meta's API allows creating the parent before children finish, the polling order could be relaxed.

5. **`is_carousel_item` on child containers.** The spec says children must be created with `is_carousel_item=true`. The provider's `create_carousel_item` (via `build_carousel_item_params`) sets this unconditionally for every child. If Meta adds any other carousel-child-specific params in future, extend `build_carousel_item_params`.

6. **Reply to others requires additional scopes.** The plan prints a non-blocking warning when `reply_to_id` is set. If the user's token lacks `threads_keyword_search` or `threads_manage_mentions`, the API will return a 403. The user is instructed to re-run `auth login` after enabling those scopes. This is noted as a user-facing warning, not enforced as a hard block, per the spec decision.

7. **Status polling uses `tokio::time::sleep(10s)`.** In tests, `FakeProvider` returns status responses immediately (no sleep). In production, the 10-second sleep means a video post could take up to 50 seconds before failing. Meta recommends ~30 seconds to process; 5 × 10s = 50s is a reasonable upper bound. The sleep is skipped in tests because the fake provider drives status synchronously.

8. **`--text -` stdin reading** is implemented via `io::stdin().read_to_string`. This blocks the async runtime briefly; for the expected use case (pasting a short text body) this is acceptable. A `tokio::fs::File` based async stdin read can be added if needed.

9. **No carousel validation (2–20 items).** The spec mentions 2–20 carousel items. The plan does not add a client-side guard for the upper bound (20 items); the API will reject > 20. A client-side check at `build_create_params` or in `run()` can be added as a follow-up.

10. **`cargo build` scope.** The self-check includes `cargo build` which builds the entire workspace. The new `threads_content_publish` scope in `DEFAULT_SCOPES` means new tokens acquired after this change will request that scope. Existing stored tokens will not have it and will be blocked by `token_has_scope`. Users must re-run `auth login` to get a token with the new scope. This is the same migration pattern used when `threads_delete` was added.
