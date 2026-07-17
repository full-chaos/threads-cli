use threads_core::publish::{
    ContainerId, ContainerStatus, MediaInput, PublishRequest, PublishingLimits,
};
use threads_core::{Error, PostId, Result};

use super::{OfficialProvider, publish_params};
use crate::dto::{ContainerStatusResp, CreateContainerResp, PublishResp, PublishingLimitResp};

pub(super) async fn delete_post(provider: &OfficialProvider, post_id: &PostId) -> Result<()> {
    delete(provider, "post/delete", post_id).await
}

pub(super) async fn delete_reply(provider: &OfficialProvider, reply_id: &PostId) -> Result<()> {
    delete(provider, "reply/delete", reply_id).await
}

pub(super) async fn create_container(
    provider: &OfficialProvider,
    request: &PublishRequest,
) -> Result<ContainerId> {
    create(provider, publish_params::create(request)).await
}

pub(super) async fn publish_container(
    provider: &OfficialProvider,
    id: &ContainerId,
) -> Result<PostId> {
    let path = provider
        .action_path("post/publish")
        .ok_or_else(|| Error::Manifest("missing action `post/publish`".into()))?;
    let creation_id = id.as_str().to_string();
    let response: PublishResp = post(provider, &path, &[("creation_id", creation_id)]).await?;
    Ok(PostId::new(response.id))
}

pub(super) async fn container_status(
    provider: &OfficialProvider,
    id: &ContainerId,
) -> Result<ContainerStatus> {
    let path = provider
        .object_path("container")
        .ok_or_else(|| Error::Manifest("missing object `container`".into()))?;
    let path = OfficialProvider::substitute_container_id(&path, id);
    let fields = provider
        .endpoint_fields("container")
        .unwrap_or_else(|| "status".into());
    let response: ContainerStatusResp = provider
        .http
        .get_json(&path, &[("fields", fields.as_str())])
        .await?;
    ContainerStatus::from_wire(&response.status)
        .ok_or_else(|| Error::Parse(format!("unknown container status: {}", response.status)))
}

pub(super) async fn publishing_limits(provider: &OfficialProvider) -> Result<PublishingLimits> {
    let path = provider
        .object_path("publishing_limit")
        .ok_or_else(|| Error::Manifest("missing object `publishing_limit`".into()))?;
    let fields = provider
        .endpoint_fields("publishing_limit")
        .unwrap_or_else(|| "quota_usage,config,reply_quota_usage,reply_config".into());
    let raw: serde_json::Value = provider
        .http
        .get_json(&path, &[("fields", fields.as_str())])
        .await?;
    let item = if let Some(items) = raw.get("data").and_then(|data| data.as_array()) {
        items
            .first()
            .cloned()
            .ok_or_else(|| Error::Parse("publishing_limit data array is empty".into()))?
    } else {
        raw
    };
    let response: PublishingLimitResp = serde_json::from_value(item).map_err(Error::from)?;
    Ok(PublishingLimits {
        post_usage: response.quota_usage,
        post_total: response.config.quota_total,
        reply_usage: response.reply_quota_usage,
        reply_total: response.reply_config.quota_total,
    })
}

pub(super) async fn create_carousel_item(
    provider: &OfficialProvider,
    item: &MediaInput,
) -> Result<ContainerId> {
    create(provider, publish_params::carousel_item(item)).await
}

pub(super) async fn create_carousel_container(
    provider: &OfficialProvider,
    request: &PublishRequest,
    children: &[ContainerId],
) -> Result<ContainerId> {
    create(provider, publish_params::carousel_parent(request, children)).await
}

async fn delete(provider: &OfficialProvider, key: &str, post_id: &PostId) -> Result<()> {
    let path = provider
        .action_path(key)
        .ok_or_else(|| Error::Manifest(format!("missing action `{key}`")))?;
    let path = OfficialProvider::substitute_post_id(&path, post_id);
    let _ = provider.http.delete_json(&path, &[]).await?;
    Ok(())
}

async fn create(
    provider: &OfficialProvider,
    params: Vec<(&'static str, String)>,
) -> Result<ContainerId> {
    let path = provider
        .action_path("post/create")
        .ok_or_else(|| Error::Manifest("missing action `post/create`".into()))?;
    let response: CreateContainerResp = post(provider, &path, &params).await?;
    Ok(ContainerId::new(response.id))
}

async fn post<T>(provider: &OfficialProvider, path: &str, params: &[(&str, String)]) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let params = params
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    let value = provider.http.post_json(path, &params).await?;
    serde_json::from_value(value).map_err(Error::from)
}
