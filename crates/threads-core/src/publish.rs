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
