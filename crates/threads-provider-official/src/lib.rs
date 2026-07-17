//! # threads-provider-official
//!
//! Primary [`Provider`] implementation backed by `https://graph.threads.net`.
//! This is Meta's REST-like Graph API (versioned paths, edges, field
//! projection via `fields=`, OAuth permissions, access tokens) — NOT GraphQL
//! despite the name.
//!
//! Driven by the manifest at `manifests/official_v1.toml` and the
//! [`Provider`] trait in [`threads_core`].

/// OAuth helpers for the official Threads endpoints.
///
/// Endpoint injection is test-only: normal builds cannot construct arbitrary
/// credential-bearing OAuth destinations.
#[cfg_attr(
    not(feature = "test-support"),
    doc = r#"
```compile_fail
use threads_provider_official::auth::OAuthEndpoints;

let _ = OAuthEndpoints::new(
    "http://127.0.0.1:8080/exchange".to_owned(),
    "http://127.0.0.1:8080/upgrade".to_owned(),
);
```
"#
)]
pub mod auth;
pub mod client;
pub mod config;
pub mod dto;
pub mod provider;
pub(crate) mod redact;
pub mod token_store;

pub use config::Config;
pub use provider::OfficialProvider;
pub use threads_core::Provider;
pub use token_store::{Token, TokenStore};
