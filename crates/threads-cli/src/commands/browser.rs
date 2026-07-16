use anyhow::{Context, Result};

pub fn open(url: &str) -> Result<()> {
    open::that_detached(url).context("opening browser")
}
