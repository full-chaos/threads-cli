use serde::{Deserialize, Serialize};

/// Shape of `/me` from graph.threads.net.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeDto {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub threads_biography: Option<String>,
    #[serde(default)]
    pub threads_profile_picture_url: Option<String>,
}

/// Shape of a post returned by /me/threads and /{post-id}.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PostDto {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub permalink: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub media_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub is_quote_post: bool,
    #[serde(default)]
    pub owner: Option<OwnerRefDto>,
    #[serde(default)]
    pub children: Option<ChildrenDto>,
    #[serde(default)]
    pub replied_to: Option<PostRefDto>,
    #[serde(default)]
    pub root_post: Option<PostRefDto>,
    #[serde(default)]
    pub is_reply: Option<bool>,
    #[serde(default)]
    pub shortcode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OwnerRefDto {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PostRefDto {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChildrenDto {
    #[serde(default)]
    pub data: Vec<PostDto>,
}

/// Pagination envelope: `{ data: [...], paging: { cursors: { before, after } } }`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Envelope<T> {
    #[serde(default = "Vec::new")]
    pub data: Vec<T>,
    #[serde(default)]
    pub paging: Option<Paging>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Paging {
    #[serde(default)]
    pub cursors: Option<Cursors>,
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Cursors {
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_me_minimal() {
        let v = r#"{"id":"123","username":"me"}"#;
        let m: MeDto = serde_json::from_str(v).unwrap();
        assert_eq!(m.id, "123");
        assert_eq!(m.username.as_deref(), Some("me"));
        assert!(m.name.is_none());
    }

    #[test]
    fn parses_envelope_with_paging() {
        let v = r#"{
            "data": [{"id":"a"},{"id":"b"}],
            "paging": {"cursors":{"after":"CURSOR"}}
        }"#;
        let e: Envelope<PostDto> = serde_json::from_str(v).unwrap();
        assert_eq!(e.data.len(), 2);
        assert_eq!(e.paging.unwrap().cursors.unwrap().after.as_deref(), Some("CURSOR"));
    }

    #[test]
    fn parses_reply_post_with_root_and_replied_to() {
        let v = r#"{
            "id":"r1","text":"hi",
            "replied_to":{"id":"p1"},
            "root_post":{"id":"root1"},
            "is_reply":true
        }"#;
        let p: PostDto = serde_json::from_str(v).unwrap();
        assert_eq!(p.replied_to.unwrap().id, "p1");
        assert_eq!(p.root_post.unwrap().id, "root1");
        assert_eq!(p.is_reply, Some(true));
    }
}
