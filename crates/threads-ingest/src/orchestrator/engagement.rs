use std::collections::{HashSet, VecDeque};

use threads_core::{PostId, Result};
use tracing::info;

use super::{BATCH_SIZE, Ingestor};
use crate::store_shim::StoreWrite;

impl<P: threads_core::Provider + 'static, S: StoreWrite + 'static> Ingestor<P, S> {
    pub(super) async fn run_ingest_engagement(&self, run_id: &str, max_depth: u32) -> Result<u64> {
        let me = self.provider.fetch_me().await?;
        self.store.upsert_user(&me)?;
        if let Some(username) = &me.username {
            self.store.resolve_author(username, &me.id)?;
        }
        let seeds = self.store.posts_by_author(&me.id)?;
        info!(
            seeds = seeds.len(),
            author = %me.id,
            max_depth,
            "ingest_engagement: BFS descending fetch_replies from every post I authored"
        );

        let mut seen = HashSet::with_capacity(seeds.len() * 4);
        seen.extend(seeds.iter().cloned());
        let mut frontier: VecDeque<(PostId, u32)> = seeds.into_iter().map(|id| (id, 0)).collect();
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut total = 0;

        while let Some((post_id, depth)) = frontier.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut cursor = None;
            loop {
                let page = self.provider.fetch_replies(&post_id, cursor).await?;
                let has_next = page.next.is_some();
                for reply in page.items {
                    if !seen.insert(reply.id.clone()) {
                        continue;
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
}
