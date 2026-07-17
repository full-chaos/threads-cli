use std::io::Write;

use anyhow::Result;
use threads_core::{Post, User};

#[allow(dead_code)] // T9 consumes the renderer APIs without changing command wiring here.
#[path = "output/audience.rs"]
mod audience;
#[allow(unused_imports)] // T9 imports these stable presentation APIs from `output`.
pub use audience::{
    AudienceReport, AudienceReportError, render_audience_report, render_engaged_accounts,
};

#[derive(Copy, Clone, Debug)]
pub enum OutputFormat {
    Human,
    Json,
    Jsonl,
    Csv,
}

pub fn render_posts(posts: &[Post], fmt: OutputFormat, w: &mut dyn Write) -> Result<()> {
    match fmt {
        OutputFormat::Human => {
            writeln!(w, "{:<22} {:<14} {:<19} text", "id", "author", "created_at")?;
            for p in posts {
                let created = p
                    .created_at
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".to_string());
                let text = one_line(p.text.as_deref().unwrap_or(""), 80);
                writeln!(
                    w,
                    "{:<22} {:<14} {:<19} {}",
                    sanitize_terminal_text(p.id.as_str()),
                    sanitize_terminal_text(p.author.as_str()),
                    created,
                    text
                )?;
            }
        }
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut *w, posts)?;
            writeln!(w)?;
        }
        OutputFormat::Jsonl => {
            for p in posts {
                serde_json::to_writer(&mut *w, p)?;
                writeln!(w)?;
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(&mut *w);
            wtr.write_record(["id", "author", "created_at", "text", "permalink"])?;
            for p in posts {
                wtr.write_record([
                    p.id.as_str(),
                    p.author.as_str(),
                    p.created_at
                        .map(|t| t.to_rfc3339())
                        .as_deref()
                        .unwrap_or(""),
                    p.text.as_deref().unwrap_or(""),
                    p.permalink.as_deref().unwrap_or(""),
                ])?;
            }
            wtr.flush()?;
        }
    }
    Ok(())
}

#[allow(dead_code)] // reserved for Phase-N commands like `whoami`
pub fn render_user(user: &User, fmt: OutputFormat, w: &mut dyn Write) -> Result<()> {
    match fmt {
        OutputFormat::Human => {
            writeln!(
                w,
                "id:         {}",
                sanitize_terminal_text(user.id.as_str())
            )?;
            if let Some(u) = &user.username {
                writeln!(w, "username:   {}", sanitize_terminal_text(u))?;
            }
            if let Some(n) = &user.name {
                writeln!(w, "name:       {}", sanitize_terminal_text(n))?;
            }
            if let Some(b) = &user.biography {
                writeln!(w, "biography:  {}", one_line(b, 120))?;
            }
        }
        OutputFormat::Json | OutputFormat::Jsonl => {
            serde_json::to_writer_pretty(&mut *w, user)?;
            writeln!(w)?;
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(&mut *w);
            wtr.write_record(["id", "username", "name", "biography"])?;
            wtr.write_record([
                user.id.as_str(),
                user.username.as_deref().unwrap_or(""),
                user.name.as_deref().unwrap_or(""),
                user.biography.as_deref().unwrap_or(""),
            ])?;
            wtr.flush()?;
        }
    }
    Ok(())
}

pub(crate) fn sanitize_terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn one_line(s: &str, max: usize) -> String {
    let collapsed = sanitize_terminal_text(s);
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max - 1).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use threads_core::{PostId, UserId};

    fn sample_post() -> Post {
        Post {
            id: PostId::new("1"),
            author: UserId::new("u1"),
            author_username: Some("u1".into()),
            text: Some("Hello world".into()),
            created_at: None,
            parent_id: None,
            root_id: None,
            permalink: Some("https://www.threads.net/@u/post/1".into()),
            media: vec![],
            urls: vec![],
            mentions: vec![],
            is_quote_post: false,
            raw: None,
        }
    }

    #[test]
    fn json_roundtrips() {
        let posts = vec![sample_post()];
        let mut buf = Vec::new();
        render_posts(&posts, OutputFormat::Json, &mut buf).unwrap();
        let parsed: Vec<Post> = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed, posts);
    }

    #[test]
    fn jsonl_one_per_line() {
        let posts = vec![sample_post(), sample_post()];
        let mut buf = Vec::new();
        render_posts(&posts, OutputFormat::Jsonl, &mut buf).unwrap();
        let s = std::str::from_utf8(&buf).unwrap();
        assert_eq!(s.lines().count(), 2);
    }

    #[test]
    fn csv_has_header() {
        let posts = vec![sample_post()];
        let mut buf = Vec::new();
        render_posts(&posts, OutputFormat::Csv, &mut buf).unwrap();
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.starts_with("id,author,created_at,text,permalink"));
    }

    #[test]
    fn human_shows_id() {
        let posts = vec![sample_post()];
        let mut buf = Vec::new();
        render_posts(&posts, OutputFormat::Human, &mut buf).unwrap();
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.contains('1'));
        assert!(s.contains("Hello world"));
    }

    #[test]
    fn human_renderers_replace_terminal_controls_in_external_text() {
        // Given: provider fields containing C0, C1, and escape-sequence controls plus Unicode.
        let mut post = sample_post();
        post.id = PostId::new("post\n\r\t\u{1b}\u{85}日本");
        post.author = UserId::new("author\n\r\t\u{1b}\u{85}café");
        post.text = Some("text\n\r\t\u{1b}\u{85}東京".into());
        let user = User {
            id: UserId::new("user\n\r\t\u{1b}\u{85}日本"),
            username: Some("username\n\r\t\u{1b}\u{85}café".into()),
            name: Some("name\n\r\t\u{1b}\u{85}東京".into()),
            biography: Some("bio\n\r\t\u{1b}\u{85}日本".into()),
            profile_picture_url: Some("https://example.test/avatar\u{1b}".into()),
        };
        let mut post_output = Vec::new();
        let mut user_output = Vec::new();

        // When: the human renderers write provider-originated fields.
        render_posts(&[post], OutputFormat::Human, &mut post_output).unwrap();
        render_user(&user, OutputFormat::Human, &mut user_output).unwrap();

        // Then: terminal controls cannot alter rows while benign Unicode remains visible.
        let output = String::from_utf8([post_output, user_output].concat()).unwrap();
        assert!(output.lines().all(|line| !line.contains(char::is_control)));
        assert!(output.contains("日本"));
        assert!(output.contains("café"));
        assert!(output.contains("東京"));
    }

    #[test]
    fn machine_post_formats_preserve_hostile_control_data() {
        // Given: a post whose machine-contract fields contain terminal control characters.
        let mut post = sample_post();
        post.id = PostId::new("post\u{1b}\u{85}");
        post.author = UserId::new("author\u{1b}\u{85}");
        post.text = Some("text\u{1b}\u{85}".into());
        post.permalink = Some("https://example.test/\u{1b}\u{85}".into());

        // When: every machine format is rendered.
        let mut json = Vec::new();
        let mut jsonl = Vec::new();
        let mut csv = Vec::new();
        render_posts(&[post.clone()], OutputFormat::Json, &mut json).unwrap();
        render_posts(&[post.clone()], OutputFormat::Jsonl, &mut jsonl).unwrap();
        render_posts(&[post.clone()], OutputFormat::Csv, &mut csv).unwrap();

        // Then: decoding recovers the unmodified source data.
        assert_eq!(serde_json::from_slice::<Vec<Post>>(&json).unwrap()[0], post);
        assert_eq!(serde_json::from_slice::<Post>(&jsonl).unwrap(), post);
        let records: Vec<csv::StringRecord> = csv::Reader::from_reader(csv.as_slice())
            .records()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(records[0].get(0), Some("post\u{1b}\u{85}"));
        assert_eq!(records[0].get(1), Some("author\u{1b}\u{85}"));
        assert_eq!(records[0].get(3), Some("text\u{1b}\u{85}"));
        assert_eq!(records[0].get(4), Some("https://example.test/\u{1b}\u{85}"));
    }
}

#[cfg(test)]
#[path = "output/audience_tests.rs"]
mod audience_tests;
