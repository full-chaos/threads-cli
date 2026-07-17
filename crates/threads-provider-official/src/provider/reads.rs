use threads_core::{Cursor, Error, Page, Post, PostId, Result, User, UserId};

use super::{OfficialProvider, posts};
use crate::dto::{Envelope, MeDto, PostDto};

pub(super) async fn fetch_me(provider: &OfficialProvider) -> Result<User> {
    let path = provider
        .object_path("me")
        .ok_or_else(|| Error::Manifest("missing object `me`".into()))?;
    let fields = provider.endpoint_fields("me");
    let mut query = Vec::new();
    if let Some(ref fields) = fields {
        query.push(("fields", fields.as_str()));
    }
    let dto: MeDto = provider.http.get_json(&path, &query).await?;
    Ok(User {
        id: UserId::new(dto.id),
        username: dto.username,
        name: dto.name,
        biography: dto.threads_biography,
        profile_picture_url: dto.threads_profile_picture_url,
    })
}

pub(super) async fn fetch_my_threads(
    provider: &OfficialProvider,
    cursor: Option<Cursor>,
) -> Result<Page<Post>> {
    fetch_post_page(provider, "me/threads", cursor, None).await
}

pub(super) async fn fetch_my_replies(
    provider: &OfficialProvider,
    cursor: Option<Cursor>,
) -> Result<Page<Post>> {
    fetch_post_page(provider, "me/replies", cursor, None).await
}

pub(super) async fn fetch_mentions(
    provider: &OfficialProvider,
    user_id: &UserId,
    cursor: Option<Cursor>,
) -> Result<Page<Post>> {
    let path = provider
        .edge_path("user/mentions")
        .ok_or_else(|| Error::Manifest("missing edge `user/mentions`".into()))?;
    let path = OfficialProvider::substitute_user_id(&path, user_id);
    let fields = provider.endpoint_fields("user/mentions");
    let mut params = vec![("limit", "100")];
    if let Some(ref fields) = fields {
        params.push(("fields", fields));
    }
    let after;
    if let Some(cursor) = cursor {
        after = cursor.0;
        params.push(("after", &after));
    }
    let response: Envelope<PostDto> = provider.http.get_json(&path, &params).await?;
    posts::envelope_to_page(response, None)
}

pub(super) async fn fetch_replies(
    provider: &OfficialProvider,
    post_id: &PostId,
    cursor: Option<Cursor>,
) -> Result<Page<Post>> {
    let path = provider
        .edge_path("post/replies")
        .ok_or_else(|| Error::Manifest("missing edge `post/replies`".into()))?;
    let path = OfficialProvider::substitute_post_id(&path, post_id);
    fetch_post_page_at(provider, "post/replies", path, cursor, Some(post_id)).await
}

pub(super) async fn fetch_thread(
    provider: &OfficialProvider,
    root_id: &PostId,
) -> Result<Vec<Post>> {
    let path = provider
        .edge_path("post/conversation")
        .ok_or_else(|| Error::Manifest("missing edge `post/conversation`".into()))?;
    let path = OfficialProvider::substitute_post_id(&path, root_id);
    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = fetch_post_page_at(
            provider,
            "post/conversation",
            path.clone(),
            cursor,
            Some(root_id),
        )
        .await?;
        items.extend(page.items);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(items)
}

pub(super) async fn fetch_post(provider: &OfficialProvider, id: &PostId) -> Result<Post> {
    let path = provider
        .object_path("post")
        .ok_or_else(|| Error::Manifest("missing object `post`".into()))?;
    let path = OfficialProvider::substitute_post_id(&path, id);
    let fields = provider.endpoint_fields("post");
    let mut query = Vec::new();
    if let Some(ref fields) = fields {
        query.push(("fields", fields.as_str()));
    }
    let dto: PostDto = provider.http.get_json(&path, &query).await?;
    posts::dto_to_post(dto, None)
}

async fn fetch_post_page(
    provider: &OfficialProvider,
    key: &str,
    cursor: Option<Cursor>,
    root_hint: Option<&PostId>,
) -> Result<Page<Post>> {
    let path = provider
        .edge_path(key)
        .ok_or_else(|| Error::Manifest(format!("missing edge `{key}`")))?;
    fetch_post_page_at(provider, key, path, cursor, root_hint).await
}

async fn fetch_post_page_at(
    provider: &OfficialProvider,
    key: &str,
    path: String,
    cursor: Option<Cursor>,
    root_hint: Option<&PostId>,
) -> Result<Page<Post>> {
    let fields = provider.endpoint_fields(key);
    let mut query = Vec::new();
    if let Some(ref fields) = fields {
        query.push(("fields", fields.as_str()));
    }
    let after;
    if let Some(cursor) = cursor {
        after = cursor.0;
        query.push(("after", after.as_str()));
    }
    let response: Envelope<PostDto> = provider.http.get_json(&path, &query).await?;
    posts::envelope_to_page(response, root_hint)
}
