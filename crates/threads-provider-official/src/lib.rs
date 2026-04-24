//! # threads-provider-official
//!
//! Primary [`Provider`] implementation backed by `https://graph.threads.net`.
//! This is Meta's REST-like Graph API (versioned paths, edges, field
//! projection via `fields=`, OAuth permissions, access tokens) — NOT GraphQL
//! despite the name.
//!
//! Driven by the manifest at `manifests/official_v1.toml` and the
//! [`Provider`] trait in [`threads_core`].

pub mod auth;
pub mod client;
pub mod config;
pub mod dto;
pub mod provider;
pub mod token_store;

pub use config::Config;
pub use provider::OfficialProvider;
pub use threads_core::Provider;
pub use token_store::{Token, TokenStore};
