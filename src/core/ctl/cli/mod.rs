pub mod client;
pub mod commands;
pub mod output;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use client::EdgionClient;
use output::OutputFormat;

/// Target type for edgion-ctl commands
///
/// - `center`: ConfCenter API (default, full CRUD operations)
/// - `server`: ConfigServer cache (read-only, Controller side)
/// - `client`: ConfigClient cache (read-only, Gateway side)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum TargetType {
    /// ConfCenter API - full CRUD operations (default)
    #[default]
    Center,
    /// ConfigServer cache - read-only (Controller)
    Server,
    /// ConfigClient cache - read-only (Gateway)
    Client,
}

impl std::fmt::Display for TargetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetType::Center => write!(f, "center"),
            TargetType::Server => write!(f, "server"),
            TargetType::Client => write!(f, "client"),
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "edgion-ctl")]
#[command(about = "Edgion control tool for managing gateway resources", long_about = None)]
#[command(version)]
pub struct Cli {
    /// Target API to connect to:
    /// - center: ConfCenter API (default, port 5800)
    /// - server: ConfigServer cache (port 5800)
    /// - client: ConfigClient cache (port 5900)
    #[arg(long, short = 't', value_enum, default_value = "center")]
    pub target: TargetType,

    /// Server address (e.g., http://localhost:5800)
    #[arg(long)]
    pub server: Option<String>,

    /// Unix socket path (e.g., /var/run/edgion/edgion.sock)
    #[arg(long)]
    pub socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Apply a configuration from a file or directory
    Apply {
        /// File or directory path
        #[arg(short, long)]
        file: String,

        /// Perform a dry run without actually applying changes
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },

    /// Get resources
    Get {
        /// Resource kind (e.g., httproute, service, gateway)
        kind: String,

        /// Resource name (optional, lists all if not specified)
        name: Option<String>,

        /// Namespace
        #[arg(short, long)]
        namespace: Option<String>,

        /// Output format: table, json, yaml, wide
        #[arg(short, long, default_value = "table")]
        output: String,
    },

    /// Delete a resource
    Delete {
        /// Resource kind (required if not using --file)
        kind: Option<String>,

        /// Resource name (required if not using --file)
        name: Option<String>,

        /// Namespace
        #[arg(short, long)]
        namespace: Option<String>,

        /// Delete resource specified in file
        #[arg(short, long)]
        file: Option<String>,
    },

    /// Reload all resources from storage
    Reload,
}

impl Cli {
    pub async fn run(&self) -> Result<()> {
        // Create API client with target type
        let client = EdgionClient::new(self.target, self.server.clone(), self.socket.clone())?;

        // Execute command
        match &self.command {
            Commands::Apply { file, dry_run } => {
                // Apply command only supported for 'center' target
                if self.target != TargetType::Center {
                    anyhow::bail!("apply command only supported for 'center' target");
                }
                commands::apply::apply(&client, file, *dry_run).await
            }
            Commands::Get {
                kind,
                name,
                namespace,
                output,
            } => {
                let format = OutputFormat::parse(output)?;
                commands::get::get(&client, kind, name.as_deref(), namespace.as_deref(), format).await
            }
            Commands::Delete {
                kind,
                name,
                namespace,
                file,
            } => {
                // Delete command only supported for 'center' target
                if self.target != TargetType::Center {
                    anyhow::bail!("delete command only supported for 'center' target");
                }
                commands::delete::delete(
                    &client,
                    kind.as_deref(),
                    name.as_deref(),
                    namespace.as_deref(),
                    file.as_deref(),
                )
                .await
            }
            Commands::Reload => {
                // Reload command only supported for 'center' target
                if self.target != TargetType::Center {
                    anyhow::bail!("reload command only supported for 'center' target");
                }
                commands::reload::reload(&client).await
            }
        }
    }
}
