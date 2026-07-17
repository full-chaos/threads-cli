use chrono::{DateTime, Utc};
use threads_core::{Cursor, Error, Media, MediaKind, Page, Post, PostId, Result, UserId};

use crate::dto::{Envelope, PostDto};

pub(super) fn envelope_to_page(
    env: Envelope<PostDto>,
    root_hint: Option<&PostId>,
) -> Result<Page<Post>> {
    let items = env
        .data
        .into_iter()
        .map(|dto| dto_to_post(dto, root_hint))
        .collect::<Result<Vec<_>>>()?;
    let next = env
        .paging
        .and_then(|paging| {
            paging
                .cursors
                .and_then(|cursors| cursors.after)
                .or_else(|| next_after(&paging.next))
        })
        .map(Cursor);
    Ok(Page { items, next })
}

fn next_after(next: &Option<String>) -> Option<String> {
    next.as_deref().and_then(|next| {
        url::Url::parse(next)
            .ok()?
            .query_pairs()
            .find_map(|(key, value)| (key == "after").then(|| value.into_owned()))
    })
}

pub(super) fn dto_to_post(dto: PostDto, root_hint: Option<&PostId>) -> Result<Post> {
    let raw = serde_json::to_value(&dto).ok();
    let created_at = dto.timestamp.as_deref().and_then(parse_timestamp);
    let author = dto
        .owner
        .as_ref()
        .filter(|owner| !owner.id.is_empty())
        .map(|owner| UserId::new(&owner.id))
        .or_else(|| {
            dto.username
                .as_deref()
                .filter(|username| !username.is_empty())
                .map(|username| UserId::new(format!("@{username}")))
        })
        .ok_or_else(|| Error::Parse("post is missing owner.id and username".into()))?;
    let parent_id = dto.replied_to.as_ref().map(|reply| PostId::new(&reply.id));
    let root_id = dto
        .root_post
        .as_ref()
        .map(|root| PostId::new(&root.id))
        .or_else(|| root_hint.cloned());
    let media = collect_media(&dto);
    Ok(Post {
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
    })
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
        let mut media = vec![media(dto, kind)];
        media.extend(collect_children_media(dto));
        media
    } else {
        vec![media(dto, kind)]
    }
}

fn media(dto: &PostDto, kind: MediaKind) -> Media {
    Media {
        kind,
        url: dto.media_url.clone(),
        thumbnail_url: dto.thumbnail_url.clone(),
    }
}

fn collect_children_media(dto: &PostDto) -> Vec<Media> {
    dto.children.as_ref().map_or_else(Vec::new, |children| {
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
    })
}

pub(super) fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%z"))
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}
