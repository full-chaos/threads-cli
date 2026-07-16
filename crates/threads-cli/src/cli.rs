use std::{num::NonZeroUsize, path::PathBuf};

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

    /// Print and optionally open Meta's official Follow Intent; this does not
    /// perform or confirm a follow.
    Follow(FollowArgs),

    /// Refresh and inspect locally observed audience data.
    #[command(subcommand)]
    Audience(AudienceCommand),

    /// Ingest records from the provider into the local store.
    #[command(subcommand)]
    Ingest(IngestCommand),

    /// Show a post, optionally the full thread rooted at it.
    Show(ShowArgs),

    /// Full-text search the local store.
    Search(SearchArgs),

    /// Export records from the store.
    Export(ExportArgs),

    /// Delete posts or replies on Threads (remote, irreversible).
    /// Default is DRY-RUN; pass --apply to actually delete.
    #[command(subcommand)]
    Delete(DeleteCommand),

    /// Publish a new post or reply to Threads.
    #[command(subcommand)]
    Post(PostCommand),
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Overwrite an existing config file.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, clap::Args)]
pub struct FollowArgs {
    /// Threads username for the official Follow Intent (with or without a leading @).
    pub username: String,

    /// Print the follow URL without opening a browser.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Debug, Subcommand)]
pub enum AudienceCommand {
    /// Fetch aggregate audience insights and observed public mentions; no follower identities.
    Refresh,
    /// Show locally stored audience observation history.
    Show(AudienceShowArgs),
    /// Rank accounts by observed replies and mentions in local data.
    Engaged(AudienceEngagedArgs),
    /// Remove locally stored audience observations before a cutoff.
    Purge(AudiencePurgeArgs),
}

#[derive(Debug, clap::Args)]
pub struct AudienceShowArgs {
    /// Number of observations to show.
    #[arg(long, default_value = "10")]
    pub history: NonZeroUsize,
}

#[derive(Debug, clap::Args)]
pub struct AudienceEngagedArgs {
    /// Number of observed accounts to show.
    #[arg(long, default_value = "20")]
    pub limit: NonZeroUsize,
    /// Rank by total observed engagement, replies, or mentions.
    #[arg(long, value_enum, default_value_t = AudienceSortArg::Total)]
    pub sort: AudienceSortArg,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum AudienceSortArg {
    Total,
    Replies,
    Mentions,
}

#[derive(Debug, clap::Args)]
pub struct AudiencePurgeArgs {
    /// Purge observations before this RFC 3339 timestamp or YYYY-MM-DD date.
    #[arg(long)]
    pub before: String,
    /// Actually remove matching observations. Without this flag, prints a dry-run count.
    #[arg(long)]
    pub apply: bool,
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

#[derive(Debug, Subcommand)]
pub enum DeleteCommand {
    /// Delete top-level posts authored by you.
    Posts(DeleteArgs),
    /// Delete replies authored by you.
    Replies(DeleteArgs),
}

#[derive(Debug, clap::Args)]
pub struct DeleteArgs {
    /// Only consider posts created STRICTLY BEFORE this time (RFC 3339 or YYYY-MM-DD).
    #[arg(long)]
    pub before: Option<String>,

    /// Only consider posts created AT OR AFTER this time (RFC 3339 or YYYY-MM-DD).
    #[arg(long)]
    pub after: Option<String>,

    /// Cap the number of candidates considered. Defaults to no cap, but the
    /// 100/24h rate limit always applies on --apply.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Actually perform the delete. Without this flag, prints what WOULD
    /// be deleted and changes nothing.
    #[arg(long)]
    pub apply: bool,

    /// Skip the "this endpoint is undocumented for replies" warning prompt.
    /// Only relevant for `delete replies`.
    #[arg(long)]
    pub yes_undocumented: bool,
}

#[derive(Debug, Subcommand)]
pub enum PostCommand {
    /// Create and publish a text, image, video, or carousel post (or reply).
    Create(PostCreateArgs),
}

/// Clap-facing enum for `--reply-control`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ReplyControlArg {
    Everyone,
    AccountsYouFollow,
    MentionedOnly,
}

#[derive(Debug, clap::Args)]
pub struct PostCreateArgs {
    /// Text body of the post. Pass `-` to read from stdin.
    #[arg(long)]
    pub text: Option<String>,

    /// Create as a reply to this post id.
    #[arg(long)]
    pub reply_to: Option<String>,

    /// Public HTTPS URL of an image to attach (repeatable; ≥2 media ⇒ carousel).
    #[arg(long)]
    pub image_url: Vec<String>,

    /// Public HTTPS URL of a video to attach (repeatable).
    #[arg(long)]
    pub video_url: Vec<String>,

    /// Control who can reply to this post.
    #[arg(long, value_enum)]
    pub reply_control: Option<ReplyControlArg>,

    /// Attach a link preview URL.
    #[arg(long)]
    pub link_attachment: Option<String>,

    /// Skip the interactive confirmation prompt (required when not on a TTY).
    #[arg(long)]
    pub yes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_structure_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn root_help_describes_existing_auth_command() {
        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("auth"));
        assert!(help.contains("Authentication subcommands"));
    }

    #[test]
    fn audience_commands_are_exposed_in_help_and_parse() {
        use clap::Parser;

        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("audience"));
        assert!(Cli::try_parse_from(["threads-cli", "audience", "refresh"]).is_ok());
        assert!(Cli::try_parse_from(["threads-cli", "audience", "show"]).is_ok());
        assert!(Cli::try_parse_from(["threads-cli", "audience", "engaged"]).is_ok());
        assert!(Cli::try_parse_from([
            "threads-cli",
            "audience",
            "purge",
            "--before",
            "2026-01-01"
        ])
        .is_ok());
    }

    #[test]
    fn audience_defaults_and_zero_boundaries_parse_as_documented() {
        use clap::Parser;

        let show = Cli::try_parse_from(["threads-cli", "audience", "show"])
            .expect("audience show should parse");
        let engaged = Cli::try_parse_from(["threads-cli", "audience", "engaged"])
            .expect("audience engaged should parse");

        match show.command {
            Command::Audience(AudienceCommand::Show(args)) => assert_eq!(args.history.get(), 10),
            other => panic!("unexpected command: {other:?}"),
        }
        match engaged.command {
            Command::Audience(AudienceCommand::Engaged(args)) => {
                assert_eq!(args.limit.get(), 20);
                assert!(matches!(args.sort, AudienceSortArg::Total));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        assert!(
            Cli::try_parse_from(["threads-cli", "audience", "show", "--history", "0"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["threads-cli", "audience", "engaged", "--limit", "0"]).is_err()
        );
    }

    #[test]
    fn follow_args_parse_plain_handle_with_no_open() {
        use clap::Parser;

        let cli = Cli::try_parse_from(["threads-cli", "follow", "threads", "--no-open"])
            .expect("follow command should parse");

        match cli.command {
            Command::Follow(args) => {
                assert_eq!(args.username, "threads");
                assert!(args.no_open);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn follow_args_parse_at_prefixed_handle() {
        use clap::Parser;

        let cli = Cli::try_parse_from(["threads-cli", "follow", "@threads"])
            .expect("follow command should parse");

        match cli.command {
            Command::Follow(args) => {
                assert_eq!(args.username, "@threads");
                assert!(!args.no_open);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn post_create_args_parse_correctly() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "threads-cli",
            "post",
            "create",
            "--text",
            "Hello world",
            "--yes",
        ])
        .expect("should parse");
        match cli.command {
            Command::Post(PostCommand::Create(args)) => {
                assert_eq!(args.text.as_deref(), Some("Hello world"));
                assert!(args.yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn post_create_reply_args_parse_correctly() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "threads-cli",
            "post",
            "create",
            "--text",
            "A reply",
            "--reply-to",
            "post_abc",
            "--reply-control",
            "mentioned-only",
        ])
        .expect("should parse");
        match cli.command {
            Command::Post(PostCommand::Create(args)) => {
                assert_eq!(args.reply_to.as_deref(), Some("post_abc"));
                assert!(matches!(
                    args.reply_control,
                    Some(ReplyControlArg::MentionedOnly)
                ));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
