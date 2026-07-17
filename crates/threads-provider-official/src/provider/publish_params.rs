use threads_core::publish::{ContainerId, MediaInput, MediaInputKind, PublishRequest};

pub(super) fn create(request: &PublishRequest) -> Vec<(&'static str, String)> {
    let mut params = vec![("media_type", request.media_type.as_wire_str().to_string())];
    if let Some(text) = &request.text {
        params.push(("text", text.clone()));
    }
    if let Some(reply_to_id) = &request.reply_to_id {
        params.push(("reply_to_id", reply_to_id.as_str().to_string()));
    }
    if let Some(reply_control) = &request.reply_control {
        params.push(("reply_control", reply_control.as_wire_str().to_string()));
    }
    if let Some(link_attachment) = &request.link_attachment {
        params.push(("link_attachment", link_attachment.clone()));
    }
    for media in &request.media {
        match media.kind {
            MediaInputKind::Image => params.push(("image_url", media.url.clone())),
            MediaInputKind::Video => params.push(("video_url", media.url.clone())),
        }
    }
    params
}

pub(super) fn carousel_item(item: &MediaInput) -> Vec<(&'static str, String)> {
    let mut params = match item.kind {
        MediaInputKind::Image => vec![
            ("media_type", "IMAGE".to_string()),
            ("image_url", item.url.clone()),
        ],
        MediaInputKind::Video => vec![
            ("media_type", "VIDEO".to_string()),
            ("video_url", item.url.clone()),
        ],
    };
    params.push(("is_carousel_item", "true".to_string()));
    params
}

pub(super) fn carousel_parent(
    request: &PublishRequest,
    children: &[ContainerId],
) -> Vec<(&'static str, String)> {
    let mut params = vec![("media_type", "CAROUSEL".to_string())];
    if let Some(text) = &request.text {
        params.push(("text", text.clone()));
    }
    if let Some(reply_to_id) = &request.reply_to_id {
        params.push(("reply_to_id", reply_to_id.as_str().to_string()));
    }
    if let Some(reply_control) = &request.reply_control {
        params.push(("reply_control", reply_control.as_wire_str().to_string()));
    }
    let children = children
        .iter()
        .map(|child| child.as_str())
        .collect::<Vec<_>>()
        .join(",");
    params.push(("children", children));
    params
}
