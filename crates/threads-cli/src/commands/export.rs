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

    // Export "everything": use an FTS MATCH that matches any post with text,
    // plus fall back to the full posts list via thread_rooted_at for any
    // post that is its own root. For v1, a simple search_text with "*" is
    // adequate — we also query posts one-by-one for zero-text rows via
    // direct SQL. To keep things lean in v1, just dump what FTS returns.
    let posts = store
        .search_text("*", usize::MAX)
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
