use async_trait::async_trait;
use chrono::{DateTime, Utc};
use threads_core::publish::{
    ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishRequest, PublishingLimits,
};
use threads_core::{
    AudienceInsightQuery, AudienceInsightResult, AudienceSnapshot, Cursor, DemographicBucket,
    DemographicDimension, Error, Media, MediaKind, Page, Post, PostId, Provider, Result, User,
    UserId,
};
use threads_manifest::Manifest;

use crate::{
    client::HttpClient,
    dto::{Envelope, InsightDto, InsightsEnvelope, MeDto, PostDto},
};

pub struct OfficialProvider {
    pub(crate) http: HttpClient,
    pub(crate) manifest: Manifest,
}

impl OfficialProvider {
    pub fn new(http: HttpClient, manifest: Manifest) -> Self {
        Self { http, manifest }
    }

    fn endpoint_fields(&self, key: &str) -> Option<String> {
        let fields = self
            .manifest
            .edges
            .iter()
            .find(|e| e.name == key)
            .map(|e| &e.fields)
            .or_else(|| {
                self.manifest
                    .objects
                    .iter()
                    .find(|o| o.name == key)
                    .map(|o| &o.fields)
            })?;
        if fields.is_empty() {
            None
        } else {
            Some(fields.join(","))
        }
    }

    fn object_path(&self, key: &str) -> Option<String> {
        self.manifest
            .objects
            .iter()
            .find(|o| o.name == key)
            .map(|o| o.path.clone())
    }

    fn edge_path(&self, key: &str) -> Option<String> {
        self.manifest
            .edges
            .iter()
            .find(|e| e.name == key)
            .map(|e| e.path.clone())
    }

    fn action_path(&self, key: &str) -> Option<String> {
        self.manifest
            .actions
            .iter()
            .find(|a| a.name == key)
            .map(|a| a.path.clone())
    }

    fn substitute_post_id(path: &str, post_id: &PostId) -> String {
        // Delete actions use either `{post-id}` or `{reply-id}` for the same
        // media-object id slot, so one helper substitutes both placeholders.
        path.replace("{post-id}", post_id.as_str())
            .replace("{reply-id}", post_id.as_str())
    }

    fn substitute_user_id(path: &str, user_id: &UserId) -> String {
        path.replace("{threads-user-id}", user_id.as_str())
    }

    pub(crate) fn substitute_container_id(path: &str, id: &ContainerId) -> String {
        path.replace("{container-id}", id.as_str())
    }
}

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

#[async_trait]
impl Provider for OfficialProvider {
    fn name(&self) -> &'static str {
        "official"
    }

    async fn fetch_me(&self) -> Result<User> {
        let path = self
            .object_path("me")
            .ok_or_else(|| Error::Manifest("missing object `me`".into()))?;
        let fields = self.endpoint_fields("me");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let dto: MeDto = self.http.get_json(&path, &q).await?;
        Ok(User {
            id: UserId::new(dto.id),
            username: dto.username,
            name: dto.name,
            biography: dto.threads_biography,
            profile_picture_url: dto.threads_profile_picture_url,
        })
    }

    async fn fetch_my_threads(&self, cursor: Option<Cursor>) -> Result<Page<Post>> {
        let path = self
            .edge_path("me/threads")
            .ok_or_else(|| Error::Manifest("missing edge `me/threads`".into()))?;
        let fields = self.endpoint_fields("me/threads");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let cur: String;
        if let Some(c) = cursor {
            cur = c.0;
            q.push(("after", cur.as_str()));
        }
        let env: Envelope<PostDto> = self.http.get_json(&path, &q).await?;
        Ok(envelope_to_page(env, None))
    }

    async fn fetch_my_replies(&self, cursor: Option<Cursor>) -> Result<Page<Post>> {
        let path = self
            .edge_path("me/replies")
            .ok_or_else(|| Error::Manifest("missing edge `me/replies`".into()))?;
        let fields = self.endpoint_fields("me/replies");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let cur: String;
        if let Some(c) = cursor {
            cur = c.0;
            q.push(("after", cur.as_str()));
        }
        let env: Envelope<PostDto> = self.http.get_json(&path, &q).await?;
        Ok(envelope_to_page(env, None))
    }

    async fn fetch_audience_insight(
        &self,
        user_id: &UserId,
        query: AudienceInsightQuery,
    ) -> Result<AudienceInsightResult> {
        let path = self
            .object_path("user/insights")
            .ok_or_else(|| Error::Manifest("missing object `user/insights`".into()))?;
        let path = Self::substitute_user_id(&path, user_id);
        let owned_params = audience_insight_params(&query);
        let params: Vec<(&str, &str)> = owned_params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let insights: InsightsEnvelope = self.http.get_json(&path, &params).await?;
        insight_to_result(insights, user_id, query)
    }

    async fn fetch_mentions(
        &self,
        user_id: &UserId,
        cursor: Option<Cursor>,
        _limit: usize,
    ) -> Result<Page<Post>> {
        let path = self
            .edge_path("user/mentions")
            .ok_or_else(|| Error::Manifest("missing edge `user/mentions`".into()))?;
        let path = Self::substitute_user_id(&path, user_id);
        let fields = self.endpoint_fields("user/mentions");
        let mut params: Vec<(&str, &str)> = vec![("limit", "100")];
        if let Some(ref fields) = fields {
            params.push(("fields", fields));
        }
        let after;
        if let Some(cursor) = cursor {
            after = cursor.0;
            params.push(("after", &after));
        }
        let mentions: Envelope<PostDto> = self.http.get_json(&path, &params).await?;
        Ok(envelope_to_page(mentions, None))
    }

    async fn fetch_replies(&self, post_id: &PostId, cursor: Option<Cursor>) -> Result<Page<Post>> {
        let path = self
            .edge_path("post/replies")
            .ok_or_else(|| Error::Manifest("missing edge `post/replies`".into()))?;
        let path = Self::substitute_post_id(&path, post_id);
        let fields = self.endpoint_fields("post/replies");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let cur: String;
        if let Some(c) = cursor {
            cur = c.0;
            q.push(("after", cur.as_str()));
        }
        let env: Envelope<PostDto> = self.http.get_json(&path, &q).await?;
        Ok(envelope_to_page(env, Some(post_id)))
    }

    async fn fetch_thread(&self, root_id: &PostId) -> Result<Vec<Post>> {
        let path = self
            .edge_path("post/conversation")
            .ok_or_else(|| Error::Manifest("missing edge `post/conversation`".into()))?;
        let path = Self::substitute_post_id(&path, root_id);
        let fields = self.endpoint_fields("post/conversation");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut qq = q.clone();
            if let Some(ref c) = cursor {
                qq.push(("after", c.as_str()));
            }
            let env: Envelope<PostDto> = self.http.get_json(&path, &qq).await?;
            let page = envelope_to_page(env, Some(root_id));
            out.extend(page.items);
            match page.next {
                Some(next) => cursor = Some(next.0),
                None => break,
            }
        }
        Ok(out)
    }

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

    async fn create_container(&self, req: &PublishRequest) -> threads_core::Result<ContainerId> {
        let path = self
            .action_path("post/create")
            .ok_or_else(|| Error::Manifest("missing action `post/create`".into()))?;
        let owned_params = build_create_params(req);
        let borrowed: Vec<(&str, &str)> =
            owned_params.iter().map(|(k, v)| (*k, v.as_str())).collect();
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
            .ok_or_else(|| Error::Manifest("missing action `post/publish`".into()))?;
        let creation_id = id.as_str().to_string();
        let params: Vec<(&str, &str)> = vec![("creation_id", creation_id.as_str())];
        let val = self.http.post_json(&path, &params).await?;
        let resp: crate::dto::PublishResp =
            serde_json::from_value(val).map_err(threads_core::Error::from)?;
        Ok(threads_core::PostId::new(resp.id))
    }

    async fn container_status(&self, id: &ContainerId) -> threads_core::Result<ContainerStatus> {
        let path = self
            .object_path("container")
            .ok_or_else(|| Error::Manifest("missing object `container`".into()))?;
        let path = Self::substitute_container_id(&path, id);
        let fields = self
            .endpoint_fields("container")
            .unwrap_or_else(|| "status".into());
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
            .ok_or_else(|| Error::Manifest("missing object `publishing_limit`".into()))?;
        let fields = self
            .endpoint_fields("publishing_limit")
            .unwrap_or_else(|| "quota_usage,config,reply_quota_usage,reply_config".into());
        // The API may return a `{ "data": [ { ... } ] }` envelope.
        let raw: serde_json::Value = self
            .http
            .get_json(&path, &[("fields", fields.as_str())])
            .await?;
        let item = if let Some(arr) = raw.get("data").and_then(|d| d.as_array()) {
            arr.first().cloned().ok_or_else(|| {
                threads_core::Error::Parse("publishing_limit data array is empty".into())
            })?
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

    async fn fetch_post(
        &self,
        id: &threads_core::PostId,
    ) -> threads_core::Result<threads_core::Post> {
        let path = self
            .object_path("post")
            .ok_or_else(|| Error::Manifest("missing object `post`".into()))?;
        let path = Self::substitute_post_id(&path, id);
        let fields = self.endpoint_fields("post");
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(ref f) = fields {
            q.push(("fields", f.as_str()));
        }
        let dto: crate::dto::PostDto = self.http.get_json(&path, &q).await?;
        Ok(dto_to_post(dto, None))
    }

    async fn create_carousel_item(&self, item: &MediaInput) -> threads_core::Result<ContainerId> {
        let path = self
            .action_path("post/create")
            .ok_or_else(|| threads_core::Error::Manifest("missing action `post/create`".into()))?;
        let owned_params = build_carousel_item_params(item);
        let borrowed: Vec<(&str, &str)> =
            owned_params.iter().map(|(k, v)| (*k, v.as_str())).collect();
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
        let borrowed: Vec<(&str, &str)> =
            owned_params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let val = self.http.post_json(&path, &borrowed).await?;
        let resp: crate::dto::CreateContainerResp =
            serde_json::from_value(val).map_err(threads_core::Error::from)?;
        Ok(ContainerId::new(resp.id))
    }
}

fn audience_insight_params(query: &AudienceInsightQuery) -> Vec<(&'static str, String)> {
    match query {
        AudienceInsightQuery::FollowersCount => vec![("metric", "followers_count".into())],
        AudienceInsightQuery::FollowerDemographics(dimension) => vec![
            ("metric", "follower_demographics".into()),
            ("breakdown", demographic_dimension_wire(dimension).into()),
        ],
    }
}

fn insight_to_result(
    insights: InsightsEnvelope,
    user_id: &UserId,
    query: AudienceInsightQuery,
) -> Result<AudienceInsightResult> {
    let expected_name = audience_query_metric(&query);
    let insight = insights
        .data
        .into_iter()
        .find(|insight| insight.name == expected_name)
        .ok_or_else(|| Error::Parse(format!("missing {expected_name} insight data")))?;

    match query {
        AudienceInsightQuery::FollowersCount => followers_count_result(insight),
        AudienceInsightQuery::FollowerDemographics(dimension) => {
            demographics_result(insight, user_id, dimension)
        }
    }
}

const fn audience_query_metric(query: &AudienceInsightQuery) -> &'static str {
    match query {
        AudienceInsightQuery::FollowersCount => "followers_count",
        AudienceInsightQuery::FollowerDemographics(_) => "follower_demographics",
    }
}

const fn demographic_dimension_wire(dimension: &DemographicDimension) -> &'static str {
    match dimension {
        DemographicDimension::Country => "country",
        DemographicDimension::City => "city",
        DemographicDimension::Age => "age",
        DemographicDimension::Gender => "gender",
    }
}

fn followers_count_result(insight: InsightDto) -> Result<AudienceInsightResult> {
    let value = insight
        .values
        .first()
        .map(|value| value.value)
        .ok_or_else(|| Error::Parse("followers_count insight is missing a value".into()))?;
    Ok(AudienceInsightResult::FollowersCount(value))
}

fn demographics_result(
    insight: InsightDto,
    user_id: &UserId,
    dimension: DemographicDimension,
) -> Result<AudienceInsightResult> {
    let expected_dimension = demographic_dimension_wire(&dimension);
    let [breakdown] = insight.breakdowns.as_slice() else {
        return Err(Error::Parse(
            "follower_demographics insight is missing one breakdown".into(),
        ));
    };
    if breakdown.dimension_keys.as_slice() != [expected_dimension] {
        return Err(Error::Parse(format!(
            "follower_demographics breakdown does not match {expected_dimension}"
        )));
    }
    let buckets = breakdown
        .results
        .iter()
        .map(|result| {
            let bucket = result.dimension_values.first().cloned().ok_or_else(|| {
                Error::Parse("follower_demographics result is missing a dimension value".into())
            })?;
            Ok(DemographicBucket {
                dimension,
                bucket,
                value: result.value,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let followers_count = insight.total_value.ok_or_else(|| {
        Error::Parse("follower_demographics insight is missing total_value".into())
    })?;
    Ok(AudienceInsightResult::Demographics(AudienceSnapshot {
        account_id: user_id.clone(),
        observed_at: Utc::now(),
        followers_count,
        demographics: buckets,
    }))
}

pub(crate) fn envelope_to_page(env: Envelope<PostDto>, root_hint: Option<&PostId>) -> Page<Post> {
    let items = env
        .data
        .into_iter()
        .map(|dto| dto_to_post(dto, root_hint))
        .collect();
    let next = env
        .paging
        .and_then(|p| p.cursors)
        .and_then(|c| c.after)
        .map(Cursor);
    Page { items, next }
}

pub(crate) fn dto_to_post(dto: PostDto, root_hint: Option<&PostId>) -> Post {
    let raw = serde_json::to_value(&dto).ok();
    let created_at = dto.timestamp.as_deref().and_then(parse_timestamp);
    // `@`-prefix marks a synthesized/sparse author (no `owner` in the DTO).
    // Callers can detect it via `starts_with('@')` and resolve later.
    let author = dto
        .owner
        .as_ref()
        .map(|o| UserId::new(&o.id))
        .or_else(|| {
            dto.username
                .as_deref()
                .map(|u| UserId::new(format!("@{u}")))
        })
        .unwrap_or_else(|| UserId::new(""));
    let parent_id = dto.replied_to.as_ref().map(|r| PostId::new(&r.id));
    let root_id = dto
        .root_post
        .as_ref()
        .map(|r| PostId::new(&r.id))
        .or_else(|| root_hint.cloned());
    let media = collect_media(&dto);
    Post {
        id: PostId::new(dto.id),
        author,
        author_username: dto.username.clone(),
        text: dto.text,
        created_at,
        parent_id,
        root_id,
        permalink: dto.permalink,
        media,
        urls: vec![],
        mentions: vec![],
        is_quote_post: dto.is_quote_post,
        raw,
    }
}

fn collect_media(dto: &PostDto) -> Vec<Media> {
    let kind = match dto.media_type.as_deref() {
        Some("IMAGE") => MediaKind::Image,
        Some("VIDEO") => MediaKind::Video,
        Some("CAROUSEL_ALBUM") => MediaKind::Carousel,
        Some("AUDIO") => MediaKind::Audio,
        Some("TEXT_POST") | None => return collect_children_media(dto),
        _ => MediaKind::Unknown,
    };
    if matches!(kind, MediaKind::Carousel) {
        let mut v = vec![Media {
            kind,
            url: dto.media_url.clone(),
            thumbnail_url: dto.thumbnail_url.clone(),
        }];
        v.extend(collect_children_media(dto));
        v
    } else {
        vec![Media {
            kind,
            url: dto.media_url.clone(),
            thumbnail_url: dto.thumbnail_url.clone(),
        }]
    }
}

fn collect_children_media(dto: &PostDto) -> Vec<Media> {
    let Some(children) = &dto.children else {
        return vec![];
    };
    children
        .data
        .iter()
        .map(|child| Media {
            kind: match child.media_type.as_deref() {
                Some("IMAGE") => MediaKind::Image,
                Some("VIDEO") => MediaKind::Video,
                Some("AUDIO") => MediaKind::Audio,
                _ => MediaKind::Unknown,
            },
            url: child.media_url.clone(),
            thumbnail_url: child.thumbnail_url.clone(),
        })
        .collect()
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    // Meta's Threads API returns timestamps as `2026-04-24T18:15:44+0000` —
    // valid ISO 8601 but NOT RFC 3339 (which mandates a colon in the TZ
    // offset). Try RFC 3339 first, then fall back to the colonless form.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z") {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::HttpClient;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{Duration, timeout},
    };

    struct ServerReply {
        status: &'static str,
        body: &'static str,
    }

    async fn one_shot_server(reply: ServerReply) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = vec![0_u8; 8_192];
            let bytes = timeout(Duration::from_secs(2), stream.read(&mut request))
                .await
                .unwrap()
                .unwrap();
            let response = format!(
                "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                reply.status,
                reply.body.len(),
                reply.body,
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request[..bytes].to_vec()).unwrap()
        });
        (format!("http://{address}"), handle)
    }

    fn audience_provider(base_url: &str) -> OfficialProvider {
        let manifest_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../manifests/official_v1.toml"
        );
        OfficialProvider::new(
            HttpClient::new(base_url, "test-token").unwrap(),
            Manifest::from_path(manifest_path).unwrap(),
        )
    }

    fn request_url(request: &str) -> url::Url {
        let target = request.split_whitespace().nth(1).unwrap();
        url::Url::parse(&format!("http://local{target}")).unwrap()
    }

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
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("TEXT"));
        assert_eq!(map.get("text").copied(), Some("Hello Threads!"));
        assert!(!params.iter().any(|(k, _)| *k == "reply_to_id"));
    }

    #[test]
    fn create_container_reply_params_include_reply_to_id() {
        use threads_core::{
            PostId,
            publish::{PublishMediaType, PublishRequest},
        };
        let req = PublishRequest {
            media_type: PublishMediaType::Text,
            text: Some("a reply".into()),
            reply_to_id: Some(PostId::new("parent_post_99")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let params = build_create_params(&req);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
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
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
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

    #[test]
    fn carousel_item_params_set_is_carousel_item() {
        use threads_core::publish::{MediaInput, MediaInputKind};
        let item = MediaInput {
            kind: MediaInputKind::Image,
            url: "https://example.com/img.jpg".into(),
        };
        let params = build_carousel_item_params(&item);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("IMAGE"));
        assert_eq!(
            map.get("image_url").copied(),
            Some("https://example.com/img.jpg")
        );
        assert_eq!(map.get("is_carousel_item").copied(), Some("true"));
    }

    #[test]
    fn carousel_parent_params_set_children_csv() {
        use threads_core::{
            PostId,
            publish::{ContainerId, PublishMediaType, PublishRequest},
        };
        let req = PublishRequest {
            media_type: PublishMediaType::Carousel,
            text: Some("carousel caption".into()),
            reply_to_id: Some(PostId::new("parent_post_7")),
            reply_control: None,
            link_attachment: None,
            media: vec![],
        };
        let children = vec![ContainerId::new("ctr_1"), ContainerId::new("ctr_2")];
        let params = build_carousel_parent_params(&req, &children);
        let map: std::collections::HashMap<&str, &str> =
            params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(map.get("media_type").copied(), Some("CAROUSEL"));
        assert_eq!(map.get("children").copied(), Some("ctr_1,ctr_2"));
        assert_eq!(map.get("text").copied(), Some("carousel caption"));
        assert_eq!(map.get("reply_to_id").copied(), Some("parent_post_7"));
    }

    #[test]
    fn dto_to_post_synthesizes_author_from_username() {
        let dto = PostDto {
            id: "p1".into(),
            username: Some("alice".into()),
            text: Some("hi".into()),
            timestamp: None,
            permalink: None,
            media_type: None,
            media_url: None,
            thumbnail_url: None,
            is_quote_post: false,
            owner: None,
            children: None,
            replied_to: None,
            root_post: None,
            is_reply: None,
            shortcode: None,
        };
        let post = dto_to_post(dto, None);
        assert_eq!(post.id, PostId::new("p1"));
        assert_eq!(post.author, UserId::new("@alice"));
        assert_eq!(post.author_username.as_deref(), Some("alice"));
    }

    #[test]
    fn dto_to_post_propagates_root_hint_when_missing() {
        let dto = PostDto {
            id: "r1".into(),
            username: Some("b".into()),
            text: None,
            timestamp: None,
            permalink: None,
            media_type: None,
            media_url: None,
            thumbnail_url: None,
            is_quote_post: false,
            owner: None,
            children: None,
            replied_to: Some(crate::dto::PostRefDto {
                id: "parent".into(),
            }),
            root_post: None,
            is_reply: Some(true),
            shortcode: None,
        };
        let post = dto_to_post(dto, Some(&PostId::new("root-x")));
        assert_eq!(post.parent_id, Some(PostId::new("parent")));
        assert_eq!(post.root_id, Some(PostId::new("root-x")));
    }

    #[test]
    fn parse_timestamp_accepts_meta_format() {
        // Meta returns `+0000` (no colon), which is valid ISO 8601 but not
        // RFC 3339. chrono's strict RFC 3339 parser rejects it.
        let ts = parse_timestamp("2026-04-24T18:15:44+0000").unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-04-24T18:15:44+00:00");
    }

    #[test]
    fn parse_timestamp_accepts_rfc3339() {
        let ts = parse_timestamp("2026-04-24T18:15:44+00:00").unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-04-24T18:15:44+00:00");
    }

    #[test]
    fn parse_timestamp_rejects_garbage() {
        assert!(parse_timestamp("not a date").is_none());
        assert!(parse_timestamp("").is_none());
    }

    #[test]
    fn envelope_to_page_extracts_after_cursor() {
        let env: Envelope<PostDto> = serde_json::from_str(
            r#"{"data":[{"id":"1","username":"u"}],"paging":{"cursors":{"after":"NXT"}}}"#,
        )
        .unwrap();
        let page = envelope_to_page(env, None);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next.as_ref().map(|c| c.0.as_str()), Some("NXT"));
    }

    #[test]
    fn substitutes_threads_user_id_in_versioned_audience_path() {
        let user_id = UserId::new("17841400000000000");
        let path = OfficialProvider::substitute_user_id(
            "/v1.0/{threads-user-id}/threads_insights",
            &user_id,
        );
        assert_eq!(path, "/v1.0/17841400000000000/threads_insights");
    }

    #[test]
    fn builds_only_documented_audience_queries() {
        let count = audience_insight_params(&AudienceInsightQuery::FollowersCount);
        assert_eq!(count, [("metric", "followers_count".to_string())]);

        for (dimension, wire) in [
            (DemographicDimension::Country, "country"),
            (DemographicDimension::City, "city"),
            (DemographicDimension::Age, "age"),
            (DemographicDimension::Gender, "gender"),
        ] {
            let demographics =
                audience_insight_params(&AudienceInsightQuery::FollowerDemographics(dimension));
            assert_eq!(
                demographics,
                [
                    ("metric", "follower_demographics".to_string()),
                    ("breakdown", wire.to_string()),
                ]
            );
        }
    }

    #[tokio::test]
    async fn fetches_followers_count_over_the_versioned_insights_path() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_followers_count.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_audience_insight(
                &UserId::new("17841400000000000"),
                AudienceInsightQuery::FollowersCount,
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);

        assert_eq!(result, AudienceInsightResult::FollowersCount(1234));
        assert_eq!(
            request_url.path(),
            "/v1.0/17841400000000000/threads_insights"
        );
        assert_eq!(
            request_url
                .query_pairs()
                .collect::<std::collections::HashMap<_, _>>(),
            std::collections::HashMap::from([
                ("metric".into(), "followers_count".into()),
                ("access_token".into(), "test-token".into()),
            ])
        );
    }

    #[tokio::test]
    async fn fetches_one_demographic_breakdown_without_date_ranges() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_demographics_country.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_audience_insight(
                &UserId::new("17841400000000000"),
                AudienceInsightQuery::FollowerDemographics(DemographicDimension::Country),
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);
        let query = request_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert!(
            matches!(result, AudienceInsightResult::Demographics(snapshot) if snapshot.demographics.len() == 2)
        );
        assert_eq!(
            query.get("metric").map(|value| value.as_ref()),
            Some("follower_demographics")
        );
        assert_eq!(
            query.get("breakdown").map(|value| value.as_ref()),
            Some("country")
        );
        assert!(!query.contains_key("since"));
        assert!(!query.contains_key("until"));
    }

    #[tokio::test]
    async fn fetches_mentions_with_a_fixed_limit_and_cursor() {
        let reply = ServerReply {
            status: "200 OK",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/mentions_page.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let page = provider
            .fetch_mentions(
                &UserId::new("17841400000000000"),
                Some(Cursor("after-cursor".into())),
                12,
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let request_url = request_url(&request);
        let query = request_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(request_url.path(), "/v1.0/17841400000000000/mentions");
        assert_eq!(query.get("limit").map(|value| value.as_ref()), Some("100"));
        assert_eq!(
            query.get("after").map(|value| value.as_ref()),
            Some("after-cursor")
        );
        assert_eq!(page.next, Some(Cursor("QVFIUnhR".into())));
        assert_eq!(page.items[0].author_username.as_deref(), Some("author"));
    }

    #[test]
    fn parses_terminal_mentions_fixture_without_a_cursor() {
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/mentions_terminal_page.json"
        ));
        let mentions: Envelope<PostDto> = serde_json::from_str(fixture).unwrap();
        let page = envelope_to_page(mentions, None);

        assert!(page.items.is_empty());
        assert!(page.next.is_none());
    }

    #[test]
    fn rejects_empty_insight_data_during_conversion() {
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../threads-ingest/tests/fixtures/audience_empty_data.json"
        ));
        let insights: InsightsEnvelope = serde_json::from_str(fixture).unwrap();
        let result = insight_to_result(
            insights,
            &UserId::new("17841400000000000"),
            AudienceInsightQuery::FollowersCount,
        );

        assert!(matches!(result, Err(Error::Parse(_))));
    }

    #[tokio::test]
    async fn fetch_mentions_maps_forbidden_responses_to_permission_denied() {
        let reply = ServerReply {
            status: "403 Forbidden",
            body: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../threads-ingest/tests/fixtures/audience_403.json"
            )),
        };
        let (base_url, server) = one_shot_server(reply).await;
        let provider = audience_provider(&base_url);

        let result = provider
            .fetch_mentions(&UserId::new("17841400000000000"), None, 100)
            .await;
        let request = server.await.unwrap();

        assert!(request.starts_with("GET /v1.0/17841400000000000/mentions?"));
        assert!(matches!(result, Err(Error::PermissionDenied(_))));
    }
}
