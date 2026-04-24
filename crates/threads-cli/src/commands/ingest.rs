use std::{path::Path, sync::Arc};

use anyhow::{anyhow, Result};
use threads_core::PostId;
use threads_ingest::{Ingestor, OfficialNormalizer};

use crate::cli::IngestCommand;

pub async fn run(
    cmd: IngestCommand,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<()> {
    let cli_cfg = crate::commands::load_config(config_override)?;
    let provider = crate::commands::open_provider(&cli_cfg).await?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;
    let ingestor = Ingestor::new(Arc::new(provider), Box::new(OfficialNormalizer), store);

    match cmd {
        IngestCommand::Me => {
            let run = ingestor
                .ingest_me()
                .await
                .map_err(|e| anyhow!("ingest me: {e}"))?;
            summary(&run);
        }
        IngestCommand::Thread { post_id } => {
            let run = ingestor
                .ingest_thread(&PostId::new(post_id.clone()))
                .await
                .map_err(|e| anyhow!("ingest thread {post_id}: {e}"))?;
            summary(&run);
        }
    }
    Ok(())
}

fn summary(run: &threads_core::FetchRun) {
    println!(
        "run: id={} provider={} posts_fetched={} {}",
        run.id,
        run.provider,
        run.posts_fetched,
        run.finished_at
            .map(|t| format!("finished_at={t}"))
            .unwrap_or_else(|| "(still running?)".to_string())
    );
    if let Some(err) = &run.error {
        println!("error: {err}");
    }
}
