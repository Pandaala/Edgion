use anyhow::{Context, Result};

use crate::cli::client::{handle_response, EdgionClient};
use crate::cli::output::{print_error, print_success};

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

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands, TargetType};
    use clap::Parser;

    #[test]
    fn cli_parses_reload_with_no_extra_args() {
        let cli = Cli::try_parse_from(["edgion-cli", "reload"]).expect("reload should parse");
        assert!(matches!(cli.command, Commands::Reload));
        // Default target should remain Center.
        assert_eq!(cli.target, TargetType::Center);
    }

    #[test]
    fn cli_parses_reload_with_explicit_target_server() {
        let cli =
            Cli::try_parse_from(["edgion-cli", "-t", "server", "reload"]).expect("reload with -t server should parse");
        assert!(matches!(cli.command, Commands::Reload));
        assert_eq!(cli.target, TargetType::Server);
    }

    #[test]
    fn cli_reload_rejects_unexpected_positional() {
        let err = Cli::try_parse_from(["edgion-cli", "reload", "extra-arg"])
            .expect_err("reload should not accept positional");
        assert!(matches!(
            err.kind(),
            clap::error::ErrorKind::UnknownArgument | clap::error::ErrorKind::InvalidSubcommand
        ));
    }
}
