use threads_core::{PostId, UserId, publish::ContainerId};
use threads_manifest::Manifest;

pub(super) fn endpoint_fields(manifest: &Manifest, key: &str) -> Option<String> {
    let fields = manifest
        .edges
        .iter()
        .find(|edge| edge.name == key)
        .map(|edge| &edge.fields)
        .or_else(|| {
            manifest
                .objects
                .iter()
                .find(|object| object.name == key)
                .map(|object| &object.fields)
        })?;
    (!fields.is_empty()).then(|| fields.join(","))
}

pub(super) fn object_path(manifest: &Manifest, key: &str) -> Option<String> {
    manifest
        .objects
        .iter()
        .find(|object| object.name == key)
        .map(|object| object.path.clone())
}

pub(super) fn edge_path(manifest: &Manifest, key: &str) -> Option<String> {
    manifest
        .edges
        .iter()
        .find(|edge| edge.name == key)
        .map(|edge| edge.path.clone())
}

pub(super) fn action_path(manifest: &Manifest, key: &str) -> Option<String> {
    manifest
        .actions
        .iter()
        .find(|action| action.name == key)
        .map(|action| action.path.clone())
}

pub(super) fn substitute_post_id(path: &str, post_id: &PostId) -> String {
    let encoded = encode_segment(post_id.as_str());
    path.replace("{post-id}", &encoded)
        .replace("{reply-id}", &encoded)
}

pub(super) fn substitute_user_id(path: &str, user_id: &UserId) -> String {
    path.replace("{threads-user-id}", &encode_segment(user_id.as_str()))
}

pub(super) fn substitute_container_id(path: &str, id: &ContainerId) -> String {
    path.replace("{container-id}", &encode_segment(id.as_str()))
}

fn encode_segment(value: &str) -> String {
    value.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
        encoded
    })
}
