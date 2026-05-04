//! # threads-store
//!
//! SQLite-backed storage for the normalized internal graph model. Owns schema,
//! migrations, typed upserts/queries, FTS5 virtual tables, and recursive-CTE
//! thread traversal.
//!
//! Per the PRD: we never derive the schema from provider responses — only from
//! the stable internal model in [`threads_core`].

mod error;
mod migrations;
mod query;
mod store;
#[cfg(test)]
mod tests;

pub use error::{Result, StoreError};
pub use store::Store;

// Re-export query helpers for callers that want to use them directly.
pub use query::{
    PostKind, delete_post, deletions_in_last_24h, get_post, list_posts,
    oldest_deletion_in_last_24h, posts_by_author, posts_in_window, record_deletion,
    record_fetch_run_end, record_fetch_run_start, search_text, thread_rooted_at, upsert_post,
    upsert_posts, upsert_user,
};
