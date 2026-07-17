mod audience;
mod deletions;
mod engagement;
mod fetch_runs;
mod post_reads;
mod post_window;
mod post_write;
mod row_conversion;
mod search;
#[cfg(test)]
mod test_support;
mod thread;
mod users;

pub use audience::{
    audience_history, count_audience_snapshots_before, delete_audience_snapshots_before,
    upsert_audience_snapshot,
};
pub use deletions::{
    delete_post, deletions_in_last_24h, oldest_deletion_in_last_24h, record_deletion,
};
pub use engagement::rank_engaged_accounts;
pub use fetch_runs::{record_fetch_run_end, record_fetch_run_start};
pub use post_reads::{get_post, list_posts, posts_by_author};
pub use post_window::{PostKind, posts_in_window};
pub use post_write::{upsert_post, upsert_posts};
pub use search::search_text;
pub use thread::thread_rooted_at;
pub use users::{resolve_author, upsert_user};

#[cfg(test)]
pub(crate) use test_support::{test_only_count_edges_from, test_only_edge_target};
