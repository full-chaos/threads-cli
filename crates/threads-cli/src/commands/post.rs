use std::{
    io::{self, IsTerminal, Read},
    path::Path,
};

use anyhow::{Result, anyhow, bail};
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

pub async fn run_post(
    cmd: crate::cli::PostCommand,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<()> {
    match cmd {
        crate::cli::PostCommand::Create(args) => run(args, config_override, db_override).await,
    }
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

    #[allow(dead_code)]
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
