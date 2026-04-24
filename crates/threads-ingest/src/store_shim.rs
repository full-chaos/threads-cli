//! Store shim — local trait definition bridging ingest and the real store.
//!
//! `threads-store` is a Phase-0 placeholder on this branch.  Rather than
//! depending on a half-implemented crate, `threads-ingest` defines the minimal
//! write interface it needs here.  Phase 2 will implement
//! `StoreWrite for threads_store::Store` and wire the two crates together.

use chrono::{DateTime, Utc};
use threads_core::{FetchRun, Post, PostId, Result, UserId};

/// Write interface required by the ingestion orchestrator.
///
/// Implemented by [`threads_store::Store`] below; the trait keeps
/// `threads-ingest` decoupled from the concrete store for tests.
pub trait StoreWrite: Send + Sync {
    /// Upsert a batch of posts, tagging each with `fetch_run_id` when provided.
    /// Returns the number of rows actually written/updated.
    fn upsert_posts(&self, posts: &[Post], fetch_run_id: Option<&str>) -> Result<usize>;

    /// Record that a fetch run has started (insert the initial row).
    fn record_fetch_run_start(&self, run: &FetchRun) -> Result<()>;

    /// Update the fetch run row when it finishes (success or error).
    fn record_fetch_run_end(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        posts_fetched: u64,
        error: Option<&str>,
    ) -> Result<()>;

    /// Look up a single post by id (used for dedup / incremental checks).
    fn get_post(&self, id: &PostId) -> Result<Option<Post>>;

    /// Return the ids of every post authored by `author`. Used by
    /// `ingest_engagement` to enumerate seeds for the BFS.
    fn posts_by_author(&self, author: &UserId) -> Result<Vec<PostId>>;
}

impl StoreWrite for threads_store::Store {
    fn upsert_posts(&self, posts: &[Post], fetch_run_id: Option<&str>) -> Result<usize> {
        Self::upsert_posts(self, posts, fetch_run_id).map_err(Into::into)
    }

    fn record_fetch_run_start(&self, run: &FetchRun) -> Result<()> {
        Self::record_fetch_run_start(self, run).map_err(Into::into)
    }

    fn record_fetch_run_end(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        posts_fetched: u64,
        error: Option<&str>,
    ) -> Result<()> {
        Self::record_fetch_run_end(self, id, finished_at, posts_fetched, error)
            .map_err(Into::into)
    }

    fn get_post(&self, id: &PostId) -> Result<Option<Post>> {
        Self::get_post(self, id).map_err(Into::into)
    }

    fn posts_by_author(&self, author: &UserId) -> Result<Vec<PostId>> {
        Self::posts_by_author(self, author).map_err(Into::into)
    }
}
