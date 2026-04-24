use std::{io::stdout, path::Path};

use anyhow::{anyhow, Result};

use crate::{cli::SearchArgs, output::{render_posts, OutputFormat}};

pub fn run(
    args: SearchArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    fmt: OutputFormat,
) -> Result<()> {
    let cli_cfg = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;
    let posts = store
        .search_text(&args.query, args.limit)
        .map_err(|e| anyhow!("fts5 search: {e}"))?;
    if posts.is_empty() {
        eprintln!("no matches");
        return Ok(());
    }
    render_posts(&posts, fmt, &mut stdout())?;
    Ok(())
}
