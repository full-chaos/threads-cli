use anyhow::{Result, bail};
use url::Url;

use crate::cli::FollowArgs;

const FOLLOW_INTENT_URL: &str = "https://www.threads.com/intent/follow";

pub fn run(args: FollowArgs) -> Result<()> {
    run_with_opener(args, super::browser::open)
}

fn run_with_opener(args: FollowArgs, opener: impl FnOnce(&str) -> Result<()>) -> Result<()> {
    let url = follow_intent_url(&args.username)?;
    println!("{url}");

    if !args.no_open {
        open_nonfatal(&url, opener);
    }

    Ok(())
}

fn follow_intent_url(raw_username: &str) -> Result<Url> {
    let username = raw_username
        .trim()
        .strip_prefix('@')
        .unwrap_or(raw_username.trim());
    if username.is_empty() {
        bail!("username must not be empty");
    }
    if username
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("username must not contain control characters or whitespace");
    }

    let mut url = Url::parse(FOLLOW_INTENT_URL)?;
    url.query_pairs_mut().append_pair("username", username);
    Ok(url)
}

fn open_nonfatal(opener_url: &Url, opener: impl FnOnce(&str) -> Result<()>) {
    if let Err(error) = opener(opener_url.as_str()) {
        eprintln!("could not open browser ({error}); visit the URL above manually");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_intent_url_normalizes_at_prefixed_outer_whitespace() {
        let url = follow_intent_url("  @threads  ").expect("username should be valid");

        assert_eq!(
            url.as_str(),
            "https://www.threads.com/intent/follow?username=threads"
        );
    }

    #[test]
    fn follow_intent_url_percent_encodes_remaining_at_sign() {
        let url = follow_intent_url("@@threads").expect("username should be valid");

        assert_eq!(
            url.as_str(),
            "https://www.threads.com/intent/follow?username=%40threads"
        );
    }

    #[test]
    fn follow_intent_url_rejects_empty_or_invalid_usernames() {
        for username in ["", "   ", "bad handle", "bad\thandle", "bad\u{0000}handle"] {
            assert!(
                follow_intent_url(username).is_err(),
                "{username:?} should fail"
            );
        }
    }

    #[test]
    fn opener_failure_is_nonfatal() {
        let result = run_with_opener(
            FollowArgs {
                username: "threads".to_string(),
                no_open: false,
            },
            |_| Err(anyhow::anyhow!("test opener failure")),
        );

        assert!(result.is_ok());
    }
}
