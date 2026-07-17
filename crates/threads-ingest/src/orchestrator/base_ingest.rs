use std::collections::HashSet;

use threads_core::{PostId, Result};
use tracing::info;

use super::{BATCH_SIZE, Ingestor};
use crate::store_shim::StoreWrite;

impl<P: threads_core::Provider + 'static, S: StoreWrite + 'static> Ingestor<P, S> {
    pub(super) async fn run_ingest_me(&self, run_id: &str) -> Result<u64> {
        let me = self.provider.fetch_me().await?;
        self.store.upsert_user(&me)?;
        if let Some(username) = &me.username {
            self.store.resolve_author(username, &me.id)?;
        }

        let mut seen = HashSet::new();
        let mut batch = Vec::new();
        let mut total = 0;
        total += self
            .ingest_post_pages(&mut seen, &mut batch, run_id, true)
            .await?;
        total += self
            .ingest_post_pages(&mut seen, &mut batch, run_id, false)
            .await?;
        if !batch.is_empty() {
            total += self.store.upsert_posts(&batch, Some(run_id))? as u64;
        }
        Ok(total)
    }

    async fn ingest_post_pages(
        &self,
        seen: &mut HashSet<PostId>,
        batch: &mut Vec<threads_core::Post>,
        run_id: &str,
        threads: bool,
    ) -> Result<u64> {
        let mut cursor = None;
        let mut page_num = 0;
        let mut total = 0;
        loop {
            page_num += 1;
            let page = if threads {
                info!(page = page_num, edge = "me/threads", "fetching page");
                self.provider.fetch_my_threads(cursor).await?
            } else {
                info!(page = page_num, edge = "me/replies", "fetching page");
                self.provider.fetch_my_replies(cursor).await?
            };
            let has_next = page.next.is_some();
            for post in page.items {
                if seen.insert(post.id.clone()) {
                    batch.push(post);
                }
                if batch.len() >= BATCH_SIZE {
                    total += self.store.upsert_posts(batch, Some(run_id))? as u64;
                    batch.clear();
                }
            }
            cursor = page.next;
            if !has_next {
                return Ok(total);
            }
        }
    }

    pub(super) async fn run_ingest_thread(&self, run_id: &str, root: &PostId) -> Result<u64> {
        let mut seen = HashSet::new();
        let mut batch = Vec::new();
        let mut total = 0;

        info!(root = %root, "fetching conversation");
        for post in self.provider.fetch_thread(root).await? {
            if seen.insert(post.id.clone()) {
                batch.push(post);
            }
            if batch.len() >= BATCH_SIZE {
                total += self.store.upsert_posts(&batch, Some(run_id))? as u64;
                batch.clear();
            }
        }
        if !batch.is_empty() {
            total += self.store.upsert_posts(&batch, Some(run_id))? as u64;
        }
        Ok(total)
    }
}
