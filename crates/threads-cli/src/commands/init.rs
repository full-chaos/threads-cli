use std::{
    io::{self, Write as _},
    path::Path,
};

use anyhow::{bail, Result};

use crate::{cli::InitArgs, config::CliConfig};

pub fn run(args: InitArgs, config_override: Option<&Path>) -> Result<()> {
    let path = config_override
        .map(Path::to_path_buf)
        .unwrap_or_else(CliConfig::default_config_path);

    if path.exists() && !args.force {
        bail!(
            "config already exists at {}; rerun with --force to overwrite",
            path.display()
        );
    }

    println!("threads-cli init");
    println!("---------------");
    println!("1. Create a Meta app at https://developers.facebook.com/apps/");
    println!("2. Add the 'Threads API' use case to the app.");
    println!("3. Under Threads API settings -> Redirect Callback URLs, register ONE:");
    println!("   - Any HTTPS URL you control (e.g. https://example.com/threads-cb),");
    println!("     or https://localhost/callback (for manual-paste OAuth).");
    println!("   Meta blocks http:// redirects for the Threads product.");
    println!("4. Copy the Threads App ID and App Secret from the app dashboard.\n");

    let app_id = prompt("Threads App ID: ")?;
    let app_secret = prompt("Threads App Secret: ")?;
    let default_redirect = "https://localhost/callback";
    let redirect = prompt(&format!(
        "Redirect URI [{default_redirect}]: "
    ))?;
    let redirect_uri = if redirect.is_empty() {
        default_redirect.to_string()
    } else {
        redirect
    };
    if redirect_uri.starts_with("http://") && !redirect_uri.starts_with("http://127.0.0.1") {
        eprintln!(
            "warning: Meta blocks non-loopback http:// redirect URIs for the Threads API.\n\
             Consider using an https:// URL or http://127.0.0.1:PORT/callback."
        );
    }

    let cfg = CliConfig {
        app_id: Some(app_id),
        app_secret: Some(app_secret),
        redirect_uri: Some(redirect_uri),
        db_path: CliConfig::default_db_path()
            .to_string_lossy()
            .into_owned(),
    };
    cfg.save_to(&path)?;
    println!("\nConfig written to {}", path.display());
    println!("Next: run `threads-cli auth login` to obtain an access token.");
    Ok(())
}

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn refuses_to_overwrite_without_force() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cfg.toml");
        std::fs::write(&path, "app_id = \"x\"\napp_secret = \"y\"\nredirect_uri = \"z\"\ndb_path = \"/tmp/s.db\"\n").unwrap();
        let err = run(InitArgs { force: false }, Some(&path))
            .expect_err("should fail when file exists");
        assert!(err.to_string().contains("already exists"));
    }
}
