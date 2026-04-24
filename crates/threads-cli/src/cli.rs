use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "threads-cli",
    version,
    about = "Ingest, model, search, and export Threads content via the official Threads Graph API."
)]
pub struct Cli {
    /// Override the config-file path (default: ~/.config/threads-cli/config.toml).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Override the SQLite store path (default: ~/.local/share/threads-cli/store.db).
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Increase logging verbosity (-v, -vv, -vvv).
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Output format for commands that render records.
    #[arg(long, value_enum, default_value_t = OutputFormatArg::Human, global = true)]
    pub format: OutputFormatArg,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum OutputFormatArg {
    Human,
    Json,
    Jsonl,
    Csv,
}

impl From<OutputFormatArg> for crate::output::OutputFormat {
    fn from(v: OutputFormatArg) -> Self {
        match v {
            OutputFormatArg::Human => Self::Human,
            OutputFormatArg::Json => Self::Json,
            OutputFormatArg::Jsonl => Self::Jsonl,
            OutputFormatArg::Csv => Self::Csv,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Interactively register credentials for the Meta Threads app.
    Init(InitArgs),

    /// Authentication subcommands.
    #[command(subcommand)]
    Auth(AuthCommand),

    /// Ingest records from the provider into the local store.
    #[command(subcommand)]
    Ingest(IngestCommand),

    /// Show a post, optionally the full thread rooted at it.
    Show(ShowArgs),

    /// Full-text search the local store.
    Search(SearchArgs),

    /// Export records from the store.
    Export(ExportArgs),
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Overwrite an existing config file.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Run OAuth flow and store the access token.
    Login,
    /// Show the current token status.
    Status,
    /// Remove the stored token.
    Logout,
}

#[derive(Debug, Subcommand)]
pub enum IngestCommand {
    /// Ingest the authenticated user's threads + replies.
    Me,
    /// Ingest a single thread (root + descendants).
    Thread {
        /// The root post id.
        post_id: String,
    },
    /// BFS descend fetch_replies from every post you authored, up to
    /// `--depth` levels deep. Populates replies-to-your-replies (and their
    /// branching conversation trees) into the local store. Requires a prior
    /// `ingest me` so the store knows which posts you own.
    Engagement {
        /// Max BFS depth below each seed. Real Threads conversations
        /// rarely exceed 4-5 levels; 8 is a safe default.
        #[arg(long, default_value_t = 8)]
        depth: u32,
    },
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    /// The post id to show.
    pub post_id: String,
    /// Show the full thread rooted at this post (recursive CTE).
    #[arg(long)]
    pub thread: bool,
}

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// The FTS5 MATCH query.
    pub query: String,
    /// Limit the number of results.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Write to a file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_structure_is_valid() {
        Cli::command().debug_assert();
    }
}
