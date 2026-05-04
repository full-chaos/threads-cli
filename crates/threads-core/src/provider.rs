use async_trait::async_trait;

use crate::{Cursor, Error, Page, Post, PostId, Result, User};

/// The central abstraction for any Threads data source.
///
/// Implementations live in `threads-provider-official` (primary, REST-like
/// `graph.threads.net`) and, feature-gated, `threads-provider-web`
/// (experimental private web GraphQL, disabled by default per PRD).
///
/// Object-safe via `async_trait` so call sites may hold `Box<dyn Provider>`
/// or `Arc<dyn Provider>`.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable identifier, e.g. `"official"` or `"web"`.
    fn name(&self) -> &'static str;

    /// The authenticated user (`/me`).
    async fn fetch_me(&self) -> Result<User>;

    /// One page of the authenticated user's top-level threads.
    async fn fetch_my_threads(&self, cursor: Option<Cursor>) -> Result<Page<Post>>;

    /// One page of the authenticated user's replies (replies made BY the
    /// authenticated user TO other posts).
    ///
    /// Defaults to an empty page so older implementations can opt in
    /// incrementally without breaking the trait's object-safety.
    async fn fetch_my_replies(&self, _cursor: Option<Cursor>) -> Result<Page<Post>> {
        Ok(Page::empty())
    }

    /// One page of replies to a given post.
    async fn fetch_replies(&self, post_id: &PostId, cursor: Option<Cursor>) -> Result<Page<Post>>;

    /// Full conversation (root + descendants) for a thread root id.
    async fn fetch_thread(&self, root_id: &PostId) -> Result<Vec<Post>>;

    /// Delete a post owned by the authenticated user.
    /// Default impl returns `Error::NotSupported`.
    async fn delete_post(&self, _post_id: &PostId) -> Result<()> {
        Err(Error::NotSupported("delete_post".into()))
    }

    /// Delete a reply owned by the authenticated user.
    /// NOTE: undocumented for replies; replies are media objects so the same
    /// DELETE /{id} path is expected to work, but verify on a test reply.
    /// Default impl returns `Error::NotSupported`.
    async fn delete_reply(&self, _reply_id: &PostId) -> Result<()> {
        Err(Error::NotSupported("delete_reply".into()))
    }
}
