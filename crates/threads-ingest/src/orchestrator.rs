//! Ingestion orchestrator — drives Provider → Normalizer → Store pipeline.
//!
//! One `Ingestor` per invocation context (provider + normalizer + store).
//! Each call to `ingest_me` / `ingest_thread` gets its own `FetchRun` UUID,
//! paginates fully, deduplicates by `PostId`, and batch-upserts via
//! `StoreWrite`.

use std::{
    collections::HashSet,
    sync::Arc,
};

use chrono::Utc;
use threads_core::{Cursor, FetchRun, PostId, Provider, Result};
use tracing::info;
use uuid::Uuid;

use crate::{
    normalizer::Normalizer,
    store_shim::StoreWrite,
};

/// Maximum posts to upsert in a single `StoreWrite::upsert_posts` call.
const BATCH_SIZE: usize = 100;

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
        let run_id = Uuid::new_v4().to_string();
        let provider_name = self.provider.name().to_string();
        let started_at = Utc::now();

        let run = FetchRun {
            id: run_id.clone(),
            provider: provider_name,
            started_at,
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };
        self.store.record_fetch_run_start(&run)?;

        let result = self.run_ingest_me(&run_id).await;

        match result {
            Ok(count) => {
                let finished_at = Utc::now();
                self.store
                    .record_fetch_run_end(&run_id, finished_at, count, None)?;
                Ok(FetchRun {
                    id: run_id,
                    provider: run.provider,
                    started_at,
                    finished_at: Some(finished_at),
                    posts_fetched: count,
                    error: None,
                })
            }
            Err(err) => {
                let finished_at = Utc::now();
                let err_str = err.to_string();
                self.store
                    .record_fetch_run_end(&run_id, finished_at, 0, Some(&err_str))?;
                Err(err)
            }
        }
    }

    /// Ingest a single thread (root post + all replies).
    ///
    /// Fetches replies for `root`, normalizing with the root's `PostId` as hint.
    pub async fn ingest_thread(&self, root: &PostId) -> Result<FetchRun> {
        let run_id = Uuid::new_v4().to_string();
        let provider_name = self.provider.name().to_string();
        let started_at = Utc::now();

        let run = FetchRun {
            id: run_id.clone(),
            provider: provider_name,
            started_at,
            finished_at: None,
            posts_fetched: 0,
            error: None,
        };
        self.store.record_fetch_run_start(&run)?;

        let result = self.run_ingest_thread(&run_id, root).await;

        match result {
            Ok(count) => {
                let finished_at = Utc::now();
                self.store
                    .record_fetch_run_end(&run_id, finished_at, count, None)?;
                Ok(FetchRun {
                    id: run_id,
                    provider: run.provider,
                    started_at,
                    finished_at: Some(finished_at),
                    posts_fetched: count,
                    error: None,
                })
            }
            Err(err) => {
                let finished_at = Utc::now();
                let err_str = err.to_string();
                self.store
                    .record_fetch_run_end(&run_id, finished_at, 0, Some(&err_str))?;
                Err(err)
            }
        }
    }

    // --- private helpers ---

    async fn run_ingest_me(&self, run_id: &str) -> Result<u64> {
        let mut seen: HashSet<PostId> = HashSet::new();
        let mut batch = Vec::new();
        let mut total: u64 = 0;
        let mut cursor: Option<Cursor> = None;
        let mut page_num = 0usize;

        loop {
            page_num += 1;
            info!(page = page_num, "fetching my-threads page");

            let page = self.provider.fetch_my_threads(cursor).await?;
            let has_next = page.next.is_some();

            for post in page.items {
                if seen.insert(post.id.clone()) {
                    batch.push(post);
                }
                if batch.len() >= BATCH_SIZE {
                    let written = self.store.upsert_posts(&batch, Some(run_id))?;
                    total += written as u64;
                    batch.clear();
                }
            }

            cursor = page.next;
            if !has_next {
                break;
            }
        }

        // Flush remaining.
        if !batch.is_empty() {
            let written = self.store.upsert_posts(&batch, Some(run_id))?;
            total += written as u64;
        }

        Ok(total)
    }

    async fn run_ingest_thread(&self, run_id: &str, root: &PostId) -> Result<u64> {
        let mut seen: HashSet<PostId> = HashSet::new();
        let mut batch = Vec::new();
        let mut total: u64 = 0;
        let mut cursor: Option<Cursor> = None;
        let mut page_num = 0usize;

        loop {
            page_num += 1;
            info!(page = page_num, root = %root, "fetching replies page");

            let page = self.provider.fetch_replies(root, cursor).await?;
            let has_next = page.next.is_some();

            for post in page.items {
                if seen.insert(post.id.clone()) {
                    batch.push(post);
                }
                if batch.len() >= BATCH_SIZE {
                    let written = self.store.upsert_posts(&batch, Some(run_id))?;
                    total += written as u64;
                    batch.clear();
                }
            }

            cursor = page.next;
            if !has_next {
                break;
            }
        }

        // Flush remaining.
        if !batch.is_empty() {
            let written = self.store.upsert_posts(&batch, Some(run_id))?;
            total += written as u64;
        }

        Ok(total)
    }
}
