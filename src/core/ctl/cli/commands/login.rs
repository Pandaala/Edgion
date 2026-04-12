use anyhow::{Context, Result};
use serde_json::Value;

use crate::core::ctl::cli::token::save_token;

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

    // Resolve password: use flag value or prompt interactively (visible, no rpassword dep)
    let password = match password {
        Some(p) => p.to_string(),
        None => {
            eprint!("Password: ");
            let mut buf = String::new();
            std::io::stdin()
                .read_line(&mut buf)
                .context("Failed to read password")?;
            buf.trim().to_string()
        }
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
        "password": password,
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

    let token = json
        .get("data")
        .and_then(|d| d.get("token"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("Login response missing 'data.token' field"))?;

    let expires_in = json
        .get("data")
        .and_then(|d| d.get("expires_in"))
        .and_then(|e| e.as_u64());

    // Save token to ~/.edgion/token
    save_token(token).context("Failed to save token")?;

    // Print success message
    if let Some(secs) = expires_in {
        let hours = secs / 3600;
        println!("Login successful. Token saved (expires in {}h).", hours);
    } else {
        println!("Login successful. Token saved.");
    }

    Ok(())
}
