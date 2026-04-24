//! # threads-provider-official
//!
//! Primary [`Provider`] implementation backed by `https://graph.threads.net`.
//! Per the PRD this is Meta's REST-like Graph API (versioned paths, edges,
//! field projection via `fields=`, OAuth permissions, access tokens) — NOT
//! GraphQL despite the name.
//!
//! Driven by the manifest at `manifests/official_v1.toml` and the
//! [`Provider`] trait in [`threads_core`].
//!
//! TODO(phase-1-team-A):
//! - auth::OAuth authorization-code flow (browser + local callback)
//! - auth::TokenStore (keyring primary, file fallback)
//! - client::HttpClient (reqwest, rate-limit, exponential backoff)
//! - dto:: request/response DTOs (derived from manifest)
//! - provider::OfficialProvider impl

pub use threads_core::Provider;

/// Placeholder — the real implementation lands in Phase 1 Team A.
pub struct OfficialProvider {
    _private: (),
}

impl OfficialProvider {
    pub fn placeholder() -> Self {
        Self { _private: () }
    }
}
