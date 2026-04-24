//! # threads-ingest
//!
//! Orchestrates the pipeline:
//! `provider -> Normalizer -> Store` with pagination, dedup, provenance
//! tagging (fetch_run_id), and raw JSON retention. Per the PRD, normalization
//! is a one-way mapping into the stable internal model.
//!
//! TODO(phase-1-team-D):
//! - Normalizer trait with an `OfficialNormalizer` impl
//! - Orchestrator: pagination loop, dedup by PostId, provenance tagging
//! - Fixture-based unit tests

pub use threads_core::{Edge, FetchRun, Post, User};

/// Placeholder. The real implementation lands in Phase 1 Team D.
pub struct Ingestor {
    _private: (),
}

impl Ingestor {
    pub fn placeholder() -> Self {
        Self { _private: () }
    }
}
