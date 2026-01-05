use anyhow::{Context, Result};

use crate::core::cli::edgion_ctl::client::{handle_response, EdgionClient};
use crate::core::cli::edgion_ctl::output::{print_error, print_success};

/// Reload command - reloads all resources from storage
pub async fn reload(client: &EdgionClient) -> Result<()> {
    println!("Reloading all resources from storage...");

    let resp = client.reload().await.context("Failed to send reload request")?;

    match handle_response(resp).await {
        Ok(msg) => {
            print_success(&format!("Resources reloaded: {}", msg));
        }
        Err(e) => {
            print_error(&format!("Failed to reload resources: {}", e));
            anyhow::bail!(e);
        }
    }

    Ok(())
}
