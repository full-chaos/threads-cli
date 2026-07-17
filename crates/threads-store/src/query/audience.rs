use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use threads_core::model::{AudienceSnapshot, DemographicBucket, DemographicDimension, UserId};

use crate::error::{Result, StoreError};

fn demographic_dimension_str(dimension: DemographicDimension) -> &'static str {
    match dimension {
        DemographicDimension::Country => "country",
        DemographicDimension::City => "city",
        DemographicDimension::Age => "age",
        DemographicDimension::Gender => "gender",
    }
}

fn demographic_dimension_from_str(value: &str) -> Result<DemographicDimension> {
    match value {
        "country" => Ok(DemographicDimension::Country),
        "city" => Ok(DemographicDimension::City),
        "age" => Ok(DemographicDimension::Age),
        "gender" => Ok(DemographicDimension::Gender),
        _ => Err(StoreError::InvalidData(format!(
            "unknown demographic dimension {value}"
        ))),
    }
}

pub fn upsert_audience_snapshot(
    conn: &mut Connection,
    snapshot: &AudienceSnapshot,
    fetch_run_id: Option<&str>,
) -> Result<()> {
    let followers_count = i64::try_from(snapshot.followers_count).map_err(|_| {
        StoreError::InvalidData("followers count exceeds SQLite INTEGER range".into())
    })?;
    let observed_at = snapshot.observed_at.to_rfc3339();
    let tx = conn.transaction().map_err(StoreError::Sqlite)?;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO users (id, username, name, biography, profile_picture_url, updated_at)
         VALUES (?1, NULL, NULL, NULL, NULL, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![snapshot.account_id.as_str(), now],
    )
    .map_err(StoreError::Sqlite)?;
    tx.execute(
        "INSERT INTO audience_snapshots (account_id, observed_at, followers_count, fetch_run_id)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id, observed_at) DO UPDATE SET
             followers_count = excluded.followers_count,
             fetch_run_id = excluded.fetch_run_id",
        params![
            snapshot.account_id.as_str(),
            observed_at,
            followers_count,
            fetch_run_id,
        ],
    )
    .map_err(StoreError::Sqlite)?;
    let snapshot_id: i64 = tx
        .query_row(
            "SELECT id FROM audience_snapshots WHERE account_id = ?1 AND observed_at = ?2",
            params![snapshot.account_id.as_str(), observed_at],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    tx.execute(
        "DELETE FROM audience_demographics WHERE snapshot_id = ?1",
        params![snapshot_id],
    )
    .map_err(StoreError::Sqlite)?;
    for demographic in &snapshot.demographics {
        let value = i64::try_from(demographic.value).map_err(|_| {
            StoreError::InvalidData("demographic value exceeds SQLite INTEGER range".into())
        })?;
        tx.execute(
            "INSERT INTO audience_demographics (snapshot_id, dimension, bucket, value)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot_id,
                demographic_dimension_str(demographic.dimension),
                demographic.bucket,
                value,
            ],
        )
        .map_err(StoreError::Sqlite)?;
    }
    tx.commit().map_err(StoreError::Sqlite)?;
    Ok(())
}

pub fn audience_history(
    conn: &Connection,
    account_id: &UserId,
    limit: usize,
) -> Result<Vec<AudienceSnapshot>> {
    let limit = i64::try_from(limit).map_err(|_| {
        StoreError::InvalidData("history limit exceeds SQLite INTEGER range".into())
    })?;
    let mut snapshot_stmt = conn
        .prepare(
            "SELECT id, observed_at, followers_count
             FROM (
                 SELECT id, observed_at, followers_count
                 FROM audience_snapshots
                 WHERE account_id = ?1
                 ORDER BY observed_at DESC
                 LIMIT ?2
             )
             ORDER BY observed_at ASC",
        )
        .map_err(StoreError::Sqlite)?;
    let snapshots: Vec<(i64, String, i64)> = snapshot_stmt
        .query_map(params![account_id.as_str(), limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::Sqlite)?;
    let mut demographic_stmt = conn
        .prepare(
            "SELECT dimension, bucket, value
             FROM audience_demographics
             WHERE snapshot_id = ?1
             ORDER BY dimension ASC, bucket ASC",
        )
        .map_err(StoreError::Sqlite)?;
    let mut history = Vec::with_capacity(snapshots.len());
    for (snapshot_id, observed_at, followers_count) in snapshots {
        let observed_at = DateTime::parse_from_rfc3339(&observed_at)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?
            .with_timezone(&Utc);
        let followers_count = u64::try_from(followers_count)
            .map_err(|_| StoreError::InvalidData("negative audience follower count".into()))?;
        let demographics: Vec<(String, String, i64)> = demographic_stmt
            .query_map(params![snapshot_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;
        let demographics = demographics
            .into_iter()
            .map(|(dimension, bucket, value)| {
                Ok(DemographicBucket {
                    dimension: demographic_dimension_from_str(&dimension)?,
                    bucket,
                    value: u64::try_from(value).map_err(|_| {
                        StoreError::InvalidData("negative demographic value".into())
                    })?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        history.push(AudienceSnapshot {
            account_id: account_id.clone(),
            observed_at,
            followers_count,
            demographics,
        });
    }
    Ok(history)
}

pub fn count_audience_snapshots_before(
    conn: &Connection,
    account_id: &UserId,
    cutoff: DateTime<Utc>,
) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audience_snapshots
             WHERE account_id = ?1 AND observed_at < ?2",
            params![account_id.as_str(), cutoff.to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    u64::try_from(count).map_err(|_| StoreError::InvalidData("negative snapshot count".into()))
}

pub fn delete_audience_snapshots_before(
    conn: &mut Connection,
    account_id: &UserId,
    cutoff: DateTime<Utc>,
) -> Result<u64> {
    let deleted = conn
        .execute(
            "DELETE FROM audience_snapshots
             WHERE account_id = ?1 AND observed_at < ?2",
            params![account_id.as_str(), cutoff.to_rfc3339()],
        )
        .map_err(StoreError::Sqlite)?;
    u64::try_from(deleted)
        .map_err(|_| StoreError::InvalidData("negative deleted snapshot count".into()))
}
