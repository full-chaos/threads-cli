//! # threads-provider-web (EXPERIMENTAL, disabled by default)
//!
//! Per the PRD, the private `threads.net/api/graphql` adapter is an optional,
//! replaceable, read-only enrichment provider that must be explicitly opted in
//! via the `enabled` Cargo feature. It is NOT part of v1 and is not built by
//! default.
//!
//! See `docs/architecture.md` and `README.md` for rationale.

#[cfg(feature = "enabled")]
pub mod adapter {
    //! TODO(future): operation-id registry, fixture-validated read-only
    //! enrichment. Not in v1.
}

#[cfg(not(feature = "enabled"))]
/// When the `enabled` feature is off (the default), this crate exports nothing
/// functional — it exists only so the workspace compiles as a complete graph.
pub const DISABLED_NOTICE: &str =
    "threads-provider-web is disabled by default. Enable the `enabled` feature to opt in.";
