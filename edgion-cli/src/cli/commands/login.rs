use anyhow::{Context, Result};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::cli::token::save_token;

/// Execute the login command
pub async fn login(base_url: &str, username: Option<&str>, password: Option<&str>) -> Result<()> {
    // Resolve username: use flag value or prompt interactively
    let username = match username {
        Some(u) => u.to_string(),
        None => {
            eprint!("Username: ");
            let mut buf = String::new();
            std::io::stdin()
                .read_line(&mut buf)
                .context("Failed to read username")?;
            buf.trim().to_string()
        }
    };

    // Resolve password: use flag value or prompt with no echo.
    //
    // NOTE: passing the password via `-p` / `--password` puts it in argv where it
    // is visible to other local users (ps / /proc). Prefer interactive prompt or
    // stdin pipe; `-p` is retained for backwards compatibility only.
    let password: Zeroizing<String> = match password {
        Some(p) => Zeroizing::new(p.to_string()),
        None => Zeroizing::new(rpassword::prompt_password("Password: ").context("Failed to read password")?),
    };

    if username.is_empty() {
        anyhow::bail!("Username cannot be empty");
    }
    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    // Build login URL and payload
    let url = format!("{}/api/v1/auth/login", base_url);
    let payload = serde_json::json!({
        "username": username,
        "password": password.as_str(),
    });

    // Send request (no token needed for login)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Failed to connect to {}", url))?;

    let status = resp.status();
    let body = resp.text().await.context("Failed to read response body")?;

    if !status.is_success() {
        // Try to extract error message from JSON
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            if let Some(error) = json.get("error") {
                anyhow::bail!("Login failed: {}", error.as_str().unwrap_or("Unknown error"));
            }
            if let Some(msg) = json.get("message") {
                anyhow::bail!("Login failed: {}", msg.as_str().unwrap_or("Unknown error"));
            }
        }
        anyhow::bail!("Login failed with status {}: {}", status, body);
    }

    // Parse success response: {"success": true, "data": {"token": "...", "expires_in": 86400}}
    let json: Value = serde_json::from_str(&body).context("Failed to parse login response")?;

    let token = Zeroizing::new(
        json.get("data")
            .and_then(|d| d.get("token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow::anyhow!("Login response missing 'data.token' field"))?
            .to_string(),
    );

    let expires_in = json
        .get("data")
        .and_then(|d| d.get("expires_in"))
        .and_then(|e| e.as_u64());

    // Save token to ~/.edgion/token
    save_token(token.as_str()).context("Failed to save token")?;

    // Print success message
    if let Some(secs) = expires_in {
        let hours = secs / 3600;
        println!("Login successful. Token saved (expires in {}h).", hours);
    } else {
        println!("Login successful. Token saved.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn cli_parses_login_with_no_args_leaves_username_and_password_none() {
        let cli = Cli::try_parse_from(["edgion-cli", "login"]).expect("login with no flags should parse");
        match cli.command {
            Commands::Login { username, password } => {
                assert!(username.is_none());
                assert!(password.is_none());
            }
            other => panic!("expected Login, got {:?}", other),
        }
    }

    #[test]
    fn cli_parses_login_with_username_and_password_long_flags() {
        let cli = Cli::try_parse_from(["edgion-cli", "login", "--username", "admin", "--password", "secret"])
            .expect("login with long flags should parse");
        match cli.command {
            Commands::Login { username, password } => {
                assert_eq!(username.as_deref(), Some("admin"));
                assert_eq!(password.as_deref(), Some("secret"));
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn cli_parses_login_with_short_flags() {
        let cli = Cli::try_parse_from(["edgion-cli", "login", "-u", "alice", "-p", "pw"])
            .expect("login with short flags should parse");
        match cli.command {
            Commands::Login { username, password } => {
                assert_eq!(username.as_deref(), Some("alice"));
                assert_eq!(password.as_deref(), Some("pw"));
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn cli_login_with_server_flag_at_root_parses() {
        let cli = Cli::try_parse_from([
            "edgion-cli",
            "--server",
            "http://ctl.example.com:5800",
            "login",
            "-u",
            "admin",
        ])
        .expect("login with --server should parse");
        assert_eq!(cli.server.as_deref(), Some("http://ctl.example.com:5800"));
        match cli.command {
            Commands::Login { username, .. } => assert_eq!(username.as_deref(), Some("admin")),
            _ => panic!("expected Login"),
        }
    }
}
