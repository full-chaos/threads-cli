use rusqlite::{Connection, params};
use threads_core::model::{EngagedAccount, EngagementSort, UserId};

use crate::error::{Result, StoreError};

pub fn rank_engaged_accounts(
    conn: &Connection,
    account_id: &UserId,
    limit: usize,
    sort: EngagementSort,
) -> Result<Vec<EngagedAccount>> {
    let limit = i64::try_from(limit)
        .map_err(|_| StoreError::InvalidData("engagement limit exceeds SQLite range".into()))?;
    let sort = match sort {
        EngagementSort::Total => "total",
        EngagementSort::Replies => "replies",
        EngagementSort::Mentions => "mentions",
    };
    let mut statement = conn
        .prepare(
            "WITH reply_counts AS (
                 SELECT reply.author_id, COUNT(DISTINCT reply.id) AS replies
                 FROM posts AS reply
                 JOIN posts AS parent ON parent.id = reply.parent_id
                 WHERE parent.author_id = ?1
                   AND reply.author_id <> ?1
                   AND reply.is_quote_post = 0
                 GROUP BY reply.author_id
             ), mention_counts AS (
                 SELECT post.author_id, COUNT(DISTINCT mention.post_id) AS mentions
                 FROM mentions AS mention
                 JOIN posts AS post ON post.id = mention.post_id
                 WHERE mention.user_id = ?1
                   AND post.author_id <> ?1
                   AND post.is_quote_post = 0
                 GROUP BY post.author_id
             ), engaged_ids AS (
                 SELECT author_id FROM reply_counts
                 UNION
                 SELECT author_id FROM mention_counts
             )
             SELECT engaged_ids.author_id, users.username,
                    COALESCE(reply_counts.replies, 0),
                    COALESCE(mention_counts.mentions, 0)
             FROM engaged_ids
             JOIN users ON users.id = engaged_ids.author_id
             LEFT JOIN reply_counts ON reply_counts.author_id = engaged_ids.author_id
             LEFT JOIN mention_counts ON mention_counts.author_id = engaged_ids.author_id
             ORDER BY
                 CASE ?2
                     WHEN 'total' THEN COALESCE(reply_counts.replies, 0) + COALESCE(mention_counts.mentions, 0)
                     WHEN 'replies' THEN COALESCE(reply_counts.replies, 0)
                     WHEN 'mentions' THEN COALESCE(mention_counts.mentions, 0)
                 END DESC,
                 LOWER(TRIM(COALESCE(users.username, engaged_ids.author_id))) ASC,
                 engaged_ids.author_id ASC
             LIMIT ?3",
        )
        .map_err(StoreError::Sqlite)?;
    let rows: Vec<(String, Option<String>, i64, i64)> = statement
        .query_map(params![account_id.as_str(), sort, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<std::result::Result<_, _>>()
        .map_err(StoreError::Sqlite)?;

    rows.into_iter()
        .map(|(user_id, username, replies, mentions)| {
            let replies = u64::try_from(replies).map_err(|_| {
                StoreError::InvalidData("engagement reply count is negative".into())
            })?;
            let mentions = u64::try_from(mentions).map_err(|_| {
                StoreError::InvalidData("engagement mention count is negative".into())
            })?;
            Ok(EngagedAccount {
                user_id: UserId::new(user_id),
                username,
                replies,
                mentions,
                total: replies + mentions,
            })
        })
        .collect()
}
