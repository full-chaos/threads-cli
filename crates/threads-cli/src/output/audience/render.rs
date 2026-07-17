use std::io::Write;

use anyhow::Result;
use serde::Serialize;
use threads_core::EngagedAccount;

use super::{AudienceOutputRow, AudienceReport, EngagedOutputRow, dimension_key};
use crate::output::{OutputFormat, sanitize_terminal_text};

pub fn render_audience_report(
    report: &AudienceReport,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    match format {
        OutputFormat::Human => render_audience_human(report, writer),
        OutputFormat::Json => render_json(report, writer),
        OutputFormat::Jsonl => render_jsonl(&report.flat_rows(), writer),
        OutputFormat::Csv => render_audience_csv(&report.flat_rows(), writer),
    }
}

pub fn render_engaged_accounts(
    accounts: &[EngagedAccount],
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let rows: Vec<EngagedOutputRow> = accounts
        .iter()
        .enumerate()
        .map(|(index, account)| EngagedOutputRow {
            rank: index + 1,
            user_id: account.user_id.to_string(),
            username: account.username.clone(),
            replies: account.replies,
            mentions: account.mentions,
            total: account.total,
        })
        .collect();
    match format {
        OutputFormat::Human => render_engaged_human(&rows, writer),
        OutputFormat::Json => render_json(&rows, writer),
        OutputFormat::Jsonl => render_jsonl(&rows, writer),
        OutputFormat::Csv => render_engaged_csv(&rows, writer),
    }
}

fn render_audience_human(report: &AudienceReport, writer: &mut dyn Write) -> Result<()> {
    if report.snapshots.is_empty() {
        writeln!(writer, "No audience observations found.")?;
        return Ok(());
    }
    writeln!(
        writer,
        "account: {}",
        sanitize_terminal_text(report.account_id.as_deref().unwrap_or_default())
    )?;
    for (index, snapshot) in report.snapshots.iter().enumerate() {
        let group = if index + 1 == report.snapshots.len() {
            "latest"
        } else {
            "history"
        };
        writeln!(
            writer,
            "{group}: {} audience_count={} delta={}",
            snapshot.observed_at.to_rfc3339(),
            snapshot.followers_count,
            format_delta(snapshot.delta)
        )?;
        for demographic in &snapshot.demographics {
            writeln!(
                writer,
                "  demographics: {} {}={}",
                dimension_key(demographic.dimension),
                sanitize_terminal_text(&demographic.bucket),
                demographic.value
            )?;
        }
    }
    Ok(())
}

fn render_engaged_human(rows: &[EngagedOutputRow], writer: &mut dyn Write) -> Result<()> {
    if rows.is_empty() {
        writeln!(writer, "No observed engagement found.")?;
        return Ok(());
    }
    writeln!(writer, "rank user_id username replies mentions total")?;
    for row in rows {
        writeln!(
            writer,
            "{} {} {} {} {} {}",
            row.rank,
            sanitize_terminal_text(&row.user_id),
            sanitize_terminal_text(row.username.as_deref().unwrap_or("")),
            row.replies,
            row.mentions,
            row.total
        )?;
    }
    Ok(())
}

fn render_json<T: Serialize>(value: &T, writer: &mut dyn Write) -> Result<()> {
    serde_json::to_writer_pretty(&mut *writer, value)?;
    writeln!(writer)?;
    Ok(())
}

fn render_jsonl<T: Serialize>(rows: &[T], writer: &mut dyn Write) -> Result<()> {
    for row in rows {
        serde_json::to_writer(&mut *writer, row)?;
        writeln!(writer)?;
    }
    Ok(())
}

fn render_audience_csv(rows: &[AudienceOutputRow], writer: &mut dyn Write) -> Result<()> {
    let mut csv = csv::Writer::from_writer(&mut *writer);
    csv.write_record([
        "kind",
        "account_id",
        "observed_at",
        "followers_count",
        "delta",
        "dimension",
        "bucket",
        "value",
    ])?;
    for row in rows {
        match row {
            AudienceOutputRow::Snapshot {
                account_id,
                observed_at,
                followers_count,
                delta,
            } => csv.write_record([
                "snapshot",
                account_id,
                &observed_at.to_rfc3339(),
                &followers_count.to_string(),
                &format_csv_delta(*delta),
                "",
                "",
                "",
            ])?,
            AudienceOutputRow::Demographic {
                account_id,
                observed_at,
                followers_count,
                delta,
                dimension,
                bucket,
                value,
            } => csv.write_record([
                "demographic",
                account_id,
                &observed_at.to_rfc3339(),
                &followers_count.to_string(),
                &format_csv_delta(*delta),
                dimension_key(*dimension),
                bucket,
                &value.to_string(),
            ])?,
        }
    }
    csv.flush()?;
    Ok(())
}

fn render_engaged_csv(rows: &[EngagedOutputRow], writer: &mut dyn Write) -> Result<()> {
    let mut csv = csv::Writer::from_writer(&mut *writer);
    csv.write_record([
        "rank", "user_id", "username", "replies", "mentions", "total",
    ])?;
    for row in rows {
        csv.write_record([
            &row.rank.to_string(),
            &row.user_id,
            row.username.as_deref().unwrap_or(""),
            &row.replies.to_string(),
            &row.mentions.to_string(),
            &row.total.to_string(),
        ])?;
    }
    csv.flush()?;
    Ok(())
}

fn format_delta(delta: Option<i64>) -> String {
    delta.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn format_csv_delta(delta: Option<i64>) -> String {
    delta.map_or_else(String::new, |value| value.to_string())
}
