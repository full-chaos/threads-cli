//! # threads-ingest
//!
//! Orchestrates the pipeline:
//! `provider → Normalizer → StoreWrite` with pagination, dedup,
//! provenance tagging (`fetch_run_id`), and raw JSON retention.
//!
//! Per the PRD: normalization is a one-way mapping into the stable internal
//! model defined in `threads-core`.

pub mod normalizer;
pub mod orchestrator;
pub mod store_shim;

// Convenience re-exports for downstream crates.
pub use normalizer::{NormalizeError, Normalizer, OfficialNormalizer};
pub use orchestrator::Ingestor;
pub use store_shim::StoreWrite;
