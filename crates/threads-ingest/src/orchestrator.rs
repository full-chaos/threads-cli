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

        let result = self.run_ingest_engagement(&run_id, max_depth).await;
        let finished_at = Utc::now();
        match result {
            Ok(count) => {
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

        // Phase 1 — own top-level threads.
        let mut cursor: Option<Cursor> = None;
        let mut page_num = 0usize;
        loop {
            page_num += 1;
            info!(page = page_num, edge = "me/threads", "fetching page");
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

        // Phase 2 — own replies (replies I made to other posts).
        let mut cursor: Option<Cursor> = None;
        let mut page_num = 0usize;
        loop {
            page_num += 1;
            info!(page = page_num, edge = "me/replies", "fetching page");
            let page = self.provider.fetch_my_replies(cursor).await?;
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

    async fn run_ingest_engagement(&self, run_id: &str, max_depth: u32) -> Result<u64> {
        // Seed: every post in the store authored by the authenticated user.
        let me = self.provider.fetch_me().await?;
        let seeds = self.store.posts_by_author(&me.id)?;
        info!(
            seeds = seeds.len(),
            author = %me.id,
            max_depth,
            "ingest_engagement: BFS descending fetch_replies from every post I authored"
        );

        let mut seen: HashSet<PostId> = HashSet::with_capacity(seeds.len() * 4);
        for s in &seeds {
            seen.insert(s.clone());
        }

        // BFS queue: (post_id, depth_from_seed).
        let mut frontier: std::collections::VecDeque<(PostId, u32)> =
            seeds.into_iter().map(|id| (id, 0)).collect();

        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut total: u64 = 0;
        while let Some((pid, depth)) = frontier.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut cursor: Option<Cursor> = None;
            loop {
                let page = self.provider.fetch_replies(&pid, cursor).await?;
                let has_next = page.next.is_some();
                for reply in page.items {
                    if !seen.insert(reply.id.clone()) {
                        continue; // already seen — skip.
                    }
                    frontier.push_back((reply.id.clone(), depth + 1));
                    batch.push(reply);
                    if batch.len() >= BATCH_SIZE {
                        total += self.store.upsert_posts(&batch, Some(run_id))? as u64;
                        batch.clear();
                    }
                }
                cursor = page.next;
                if !has_next {
                    break;
                }
            }
        }
        if !batch.is_empty() {
            total += self.store.upsert_posts(&batch, Some(run_id))? as u64;
        }
        Ok(total)
    }

    async fn run_ingest_thread(&self, run_id: &str, root: &PostId) -> Result<u64> {
        let mut seen: HashSet<PostId> = HashSet::new();
        let mut batch = Vec::new();
        let mut total: u64 = 0;

        // Use the provider's `fetch_thread` which returns root + all
        // descendants via the manifest's `post/conversation` edge. Previously
        // this method only called `fetch_replies`, silently dropping the root
        // when it wasn't already in the store and storing ZERO posts for a
        // thread with no replies while still reporting success.
        info!(root = %root, "fetching conversation");
        let posts = self.provider.fetch_thread(root).await?;
        for post in posts {
            if seen.insert(post.id.clone()) {
                batch.push(post);
            }
            if batch.len() >= BATCH_SIZE {
                let written = self.store.upsert_posts(&batch, Some(run_id))?;
                total += written as u64;
                batch.clear();
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
