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

    // Convenience: treat "*" (and the empty string) as "list everything".
    // FTS5 MATCH rejects "*" alone, and asking users to pick a dummy token
    // is ceremony the store already avoids via list_posts.
    let posts = if args.query.trim().is_empty() || args.query.trim() == "*" {
        store.list_posts(args.limit).map_err(|e| anyhow!("list posts: {e}"))?
    } else {
        store
            .search_text(&args.query, args.limit)
            .map_err(|e| anyhow!("fts5 search: {e}"))?
    };
    if posts.is_empty() {
        eprintln!("no matches");
        return Ok(());
    }
    render_posts(&posts, fmt, &mut stdout())?;
    Ok(())
}
