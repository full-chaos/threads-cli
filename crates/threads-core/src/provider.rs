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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::{ContainerId, MediaInput, MediaInputKind, PublishRequest, PublishMediaType};

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
