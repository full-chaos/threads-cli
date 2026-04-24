# Codex Adversarial Review

Target: branch diff against main
Verdict: needs-attention

No-ship: the change leaves recoverable-looking ingest paths that can silently corrupt local thread state and stores OAuth bearer tokens with unsafe fallback file permissions.

Findings:
- [high] Upserts leave stale graph edges behind (crates/threads-store/src/query.rs:119-171)
  `upsert_post_tx` deletes media, urls, and mentions before replacing a post, but it never deletes existing `edges` rows for that post before inserting the new reply/root/mention/quote edges with `INSERT OR IGNORE`. If a post is reingested with a corrected or missing `parent_id`, `root_id`, quote flag, or mentions, the old edges remain and recursive thread traversal can keep returning posts in threads they no longer belong to. That is silent local data corruption and hard to repair after multiple ingest runs.
  Recommendation: Within the same transaction, delete or precisely update all edges owned by `from_id = post.id` for the managed edge kinds before reinserting the edges derived from the current `Post`.
- [high] Fallback token file is created with default readable permissions (crates/threads-provider-official/src/token_store.rs:76-82)
  When keyring storage fails, the access token is serialized with `fs::write`. On Unix this creates the token file with the process umask, commonly `0644`, which can expose a long-lived Threads bearer token to other local users or backup/indexing processes. The code treats this as a normal fallback path, so the exposure can happen silently on systems without a working keyring.
  Recommendation: Create the token directory with private permissions and write the file with mode `0600` using `OpenOptionsExt::mode` on Unix, then verify or set permissions after write. Consider failing closed if private permissions cannot be guaranteed.
- [medium] Single-thread ingest never stores the root post (crates/threads-ingest/src/orchestrator.rs:200-235)
  The public contract says `ingest_thread` ingests the root post plus descendants, but `run_ingest_thread` only calls `fetch_replies(root, cursor)` and upserts those replies. If the root is not already in the store, `threads-cli ingest thread <id>` records a successful run while omitting the requested root post entirely; for a thread with no replies it stores zero posts. This makes later `show --thread`, export, and provenance data incomplete while reporting success.
  Recommendation: Fetch and upsert the root post before walking replies, or use the provider's conversation/thread endpoint if it returns the root plus descendants. Add a test where the store starts empty and a thread with no replies still persists the root.

Next steps:
- Fix the edge replacement semantics and add regression tests for changed parent/root/mentions across repeated upserts.
- Make `ingest_thread` persist the root post and cover empty-thread ingestion.
- Harden token fallback file permissions before shipping OAuth login.
