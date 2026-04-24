use std::{io::stdout, path::Path};

use anyhow::{anyhow, Result};
use threads_core::PostId;

use crate::{cli::ShowArgs, output::{render_posts, OutputFormat}};

pub fn run(
    args: ShowArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    fmt: OutputFormat,
) -> Result<()> {
    let cli_cfg = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;
    let id = PostId::new(args.post_id.clone());

    let posts = if args.thread {
        store
            .thread_rooted_at(&id)
            .map_err(|e| anyhow!("thread traversal: {e}"))?
    } else {
        match store.get_post(&id).map_err(|e| anyhow!("get post: {e}"))? {
            Some(p) => vec![p],
            None => {
                eprintln!("no post with id {}", args.post_id);
                return Ok(());
            }
        }
    };
    render_posts(&posts, fmt, &mut stdout())?;
    Ok(())
}
