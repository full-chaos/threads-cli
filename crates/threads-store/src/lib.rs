//! # threads-store
//!
//! SQLite-backed storage for the normalized internal graph model. Owns schema,
//! migrations, typed upserts/queries, FTS5 virtual tables, and recursive-CTE
//! thread traversal.
//!
//! Per the PRD: we never derive the schema from provider responses — only from
//! the stable internal model in [`threads_core`].
//!
//! TODO(phase-1-team-B):
//! - schema::migrations (idempotent, versioned)
//! - schema::tables users/posts/edges/media/urls/mentions/fetch_runs/raw_payloads
//! - schema::fts5 virtual tables + triggers
//! - query::{upsert_post, upsert_user, get_thread, search_fts}
//! - query::recursive_thread (recursive CTE)

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(String),

    #[error("migration: {0}")]
    Migration(String),

    #[error("not found: {0}")]
    NotFound(String),
}

/// Placeholder. The real implementation lands in Phase 1 Team B.
pub struct Store {
    _private: (),
}

impl Store {
    pub fn placeholder() -> Self {
        Self { _private: () }
    }
}
