use std::io::Write;

use anyhow::Result;
use threads_core::{Post, User};

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
                let text = p.text.as_deref().unwrap_or("");
                let text = one_line(text, 80);
                writeln!(w, "{:<22} {:<14} {:<19} {}", p.id, p.author, created, text)?;
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
            writeln!(w, "id:         {}", user.id)?;
            if let Some(u) = &user.username {
                writeln!(w, "username:   {u}")?;
            }
            if let Some(n) = &user.name {
                writeln!(w, "name:       {n}")?;
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

fn one_line(s: &str, max: usize) -> String {
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
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
}
