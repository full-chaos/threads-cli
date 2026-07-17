//! # threads-core
//!
//! Shared types, the `Provider` trait, and the internal graph model used across
//! the threads-cli workspace. Every downstream crate depends on this one and
//! should NOT leak provider-specific details past this boundary.
//!
//! Per the PRD: provider responses flow `raw -> typed provider DTO -> normalizer
//! -> internal model (this crate) -> SQLite store`. The internal model is
//! deliberately stable; provider changes become normalizer edits, never model
//! migrations.

pub mod error;
pub mod model;
pub mod provider;
pub mod publish;

pub use error::{Error, Result};
pub use model::{
    AudienceInsightQuery, AudienceInsightResult, AudienceSnapshot, Cursor, DemographicBucket,
    DemographicDimension, DemographicInsight, Edge, EdgeKind, EngagedAccount, EngagementSort,
    FetchRun, Media, MediaKind, Mention, Page, Post, PostId, UrlEntity, User, UserId,
};
pub use provider::Provider;
pub use publish::{
    ContainerId, ContainerStatus, MediaInput, MediaInputKind, PublishMediaType, PublishRequest,
    PublishingLimits, ReplyControl, validate_text,
};
