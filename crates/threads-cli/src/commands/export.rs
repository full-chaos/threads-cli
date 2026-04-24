use std::{
    fs::File,
    io::{stdout, BufWriter, Write},
    path::Path,
};

use anyhow::{anyhow, Result};

use crate::{cli::ExportArgs, output::{render_posts, OutputFormat}};

pub fn run(
    args: ExportArgs,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    fmt: OutputFormat,
) -> Result<()> {
    let cli_cfg = crate::commands::load_config(config_override)?;
    let store = crate::commands::open_store(&cli_cfg, db_override)?;

    // Enumerate via a plain SELECT rather than FTS5 — FTS5's MATCH needs a
    // non-trivial query token (`*` alone is invalid), and we want to include
    // posts with NULL/empty text too.
    let posts = store
        .list_posts(usize::MAX)
        .map_err(|e| anyhow!("enumerate posts: {e}"))?;

    let mut writer: Box<dyn Write> = match args.out.as_deref() {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Box::new(BufWriter::new(File::create(path)?))
        }
        None => Box::new(BufWriter::new(stdout().lock())),
    };
    render_posts(&posts, fmt, &mut writer)?;
    writer.flush()?;
    Ok(())
}
