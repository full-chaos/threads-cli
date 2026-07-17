//! Ingestion orchestrator — drives Provider → Normalizer → Store pipeline.
//!
//! One `Ingestor` per invocation context (provider + normalizer + store).
//! Each call to `ingest_me` / `ingest_thread` gets its own `FetchRun` UUID,
//! paginates fully, deduplicates by `PostId`, and batch-upserts via
//! `StoreWrite`.

use std::sync::Arc;

use chrono::Utc;
use threads_core::{FetchRun, PostId, Provider, Result, UserId};
use uuid::Uuid;

use crate::{normalizer::Normalizer, store_shim::StoreWrite};

/// Maximum posts to upsert in a single `StoreWrite::upsert_posts` call.
const BATCH_SIZE: usize = 100;

mod audience_refresh;
mod base_ingest;
mod engagement;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionIngestWarning {
    MissingAuthenticatedUsername,
    PermissionDenied(String),
    ApiFailure(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudienceRefreshSummary {
    pub account_id: UserId,
    pub followers_count: u64,
    pub demographics_count: usize,
    pub mentions_ingested: u64,
    pub mention_warning: Option<MentionIngestWarning>,
}

/// Drives the full `provider → normalizer → store` pipeline.
pub struct Ingestor<P: Provider + 'static, S: StoreWrite + 'static> {
    provider: Arc<P>,
    #[allow(dead_code)] // retained so future direct-normalize APIs don't break callers
    normalizer: Box<dyn Normalizer>,
    store: Arc<S>,
}

impl<P: Provider + 'static, S: StoreWrite + 'static> Ingestor<P, S> {
    /// Create a new `Ingestor`.
    pub fn new(provider: Arc<P>, normalizer: Box<dyn Normalizer>, store: Arc<S>) -> Self {
        Self {
            provider,
            normalizer,
            store,
        }
    }

    /// Ingest the authenticated user's full thread history.
    ///
    /// 1. Fetches `/me` threads page by page.
    /// 2. Normalizes each post.
    /// 3. Deduplicates by `PostId` within this run.
    /// 4. Batch-upserts 100 at a time.
    /// 5. Records the `FetchRun` in the store (start + end).
    pub async fn ingest_me(&self) -> Result<FetchRun> {
        let run = self.start_fetch_run()?;
        let result = self.run_ingest_me(&run.id).await;
        self.finish_fetch_run(run, result)
    }

    /// Ingest replies to every post authored by the authenticated user,
    /// recursively descending `fetch_replies` up to `max_depth` levels deep.
    ///
    /// This is the "collect the reply tree under things I said" workflow:
    /// every post where `author_id == me.id` becomes a BFS seed, and every
    /// reply fetched also becomes a seed for the next level (so reply-to-
    /// reply chains fan out correctly). Dedup via a single `HashSet<PostId>`
    /// shared across seeds keeps the traversal O(posts).
    ///
    /// Requires a prior `ingest_me()` to populate the seed set.
    pub async fn ingest_engagement(&self, max_depth: u32) -> Result<FetchRun> {
        let run = self.start_fetch_run()?;
        let result = self.run_ingest_engagement(&run.id, max_depth).await;
        self.finish_fetch_run(run, result)
    }

    /// Ingest a single thread (root post + all replies).
    ///
    /// Fetches replies for `root`, normalizing with the root's `PostId` as hint.
    pub async fn ingest_thread(&self, root: &PostId) -> Result<FetchRun> {
        let run = self.start_fetch_run()?;
        let result = self.run_ingest_thread(&run.id, root).await;
        self.finish_fetch_run(run, result)
    }

    fn start_fetch_run(&self) -> Result<FetchRun> {
        let run = FetchRun {
            id: Uuid::new_v4().to_string(),
            provider: self.provider.name().to_string(),
            started_at: Utc::now(),
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };
        self.store.record_fetch_run_start(&run)?;
        Ok(run)
    }

    fn finish_fetch_run(&self, run: FetchRun, result: Result<u64>) -> Result<FetchRun> {
        let finished_at = Utc::now();
        match result {
            Ok(posts_fetched) => {
                self.store
                    .record_fetch_run_end(&run.id, finished_at, posts_fetched, None)?;
                Ok(FetchRun {
                    finished_at: Some(finished_at),
                    posts_fetched,
                    ..run
                })
            }
            Err(error) => {
                let error_text = error.to_string();
                self.store
                    .record_fetch_run_end(&run.id, finished_at, 0, Some(&error_text))?;
                Err(error)
            }
        }
    }
}
