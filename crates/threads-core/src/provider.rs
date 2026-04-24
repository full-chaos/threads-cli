use async_trait::async_trait;

use crate::{Cursor, Page, Post, PostId, Result, User};

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

    /// One page of replies to a given post.
    async fn fetch_replies(
        &self,
        post_id: &PostId,
        cursor: Option<Cursor>,
    ) -> Result<Page<Post>>;

    /// Full conversation (root + descendants) for a thread root id.
    async fn fetch_thread(&self, root_id: &PostId) -> Result<Vec<Post>>;
}
