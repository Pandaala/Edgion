pub mod auto_auth;
pub mod client;
pub mod commands;
pub mod output;
pub mod token;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use client::EdgionClient;
use output::OutputFormat;

/// Target type for edgion-cli commands
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
#[command(name = "edgion-cli")]
#[command(about = "Edgion control tool for managing gateway resources", long_about = None)]
#[command(version)]
pub struct Cli {
    /// Target API to connect to:
    /// - center: ConfCenter API (default, port 5800)
    /// - server: ConfigServer cache (port 5800)
    /// - client: ConfigClient cache (port 5810)
    #[arg(long, short = 't', value_enum, default_value = "center")]
    pub target: TargetType,

    /// Server address (e.g., http://localhost:5800)
    #[arg(long)]
    pub server: Option<String>,

    /// Unix socket path (e.g., /var/run/edgion/edgion.sock)
    #[arg(long)]
    pub socket: Option<PathBuf>,

    /// Controller config file path for auto-login.
    /// Auto-detected from default locations if not specified.
    #[arg(long, short = 'C', value_name = "FILE")]
    pub controller_config: Option<String>,

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

        /// Use the metadata-only `/api/v1/keys/...` endpoint (debug / AI-diagnostic
        /// use case). Only valid with `--target center` (default).
        #[arg(long)]
        metadata_only: bool,

        /// Maximum number of items to return per request (K8s-style pagination).
        /// Has no effect on FS / Etcd backends (architectural no-op).
        #[arg(long)]
        limit: Option<u32>,

        /// Continue token from a previous response, to fetch the next page.
        /// Mutually exclusive with `--all`.
        #[arg(long = "continue")]
        continue_token: Option<String>,

        /// Auto-paginate: loop until `continue_token` is null, output items
        /// concatenated as a JSON array / YAML sequence (envelope stripped).
        #[arg(long, conflicts_with = "continue_token")]
        all: bool,
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

    /// Generate mTLS certificates for conf_sync gRPC channel
    GenCerts(commands::gen_certs::GenCertsArgs),

    /// Log in to the controller and save the token locally (~/.edgion/token)
    Login {
        /// Username
        #[arg(short, long)]
        username: Option<String>,

        /// Password
        #[arg(short, long)]
        password: Option<String>,
    },
}

impl Cli {
    pub async fn run(&self) -> Result<()> {
        // GenCerts does not need an EdgionClient connection
        if let Commands::GenCerts(args) = &self.command {
            return commands::gen_certs::execute(args).await;
        }

        // Login does not need an EdgionClient; it uses a raw HTTP call
        if let Commands::Login { username, password } = &self.command {
            let base_url = self.server.clone().unwrap_or_else(|| {
                format!(
                    "http://localhost:{}",
                    match self.target {
                        TargetType::Center | TargetType::Server => 5800,
                        TargetType::Client => 5810,
                    }
                )
            });
            return commands::login::login(&base_url, username.as_deref(), password.as_deref()).await;
        }

        // Pre-flight compat check: --metadata-only only works with --target center.
        // Must fire before ensure_auth so the error is immediate (no network call
        // needed to diagnose an invalid flag combination).
        if let Commands::Get {
            metadata_only: true, ..
        } = &self.command
        {
            if self.target != TargetType::Center {
                eprintln!(
                    "Error: --metadata-only only works with --target center (got {:?}).",
                    self.target
                );
                std::process::exit(2);
            }
        }

        // Create API client with target type
        let mut client = EdgionClient::new(
            self.target,
            self.server.clone(),
            self.socket.clone(),
            self.controller_config.clone(),
        )?;

        // Ensure authentication (auto-detect and auto-login if needed)
        client.ensure_auth().await?;

        // Execute command
        match &self.command {
            Commands::Apply { file, dry_run } => {
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
                metadata_only,
                limit,
                continue_token,
                all,
            } => {
                let format = OutputFormat::parse(output)?;
                commands::get::get(
                    &client,
                    kind,
                    name.as_deref(),
                    namespace.as_deref(),
                    format,
                    commands::get::GetOptions {
                        metadata_only: *metadata_only,
                        limit: *limit,
                        continue_token: continue_token.clone(),
                        all: *all,
                    },
                )
                .await
            }
            Commands::Delete {
                kind,
                name,
                namespace,
                file,
            } => {
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
                if self.target != TargetType::Center {
                    anyhow::bail!("reload command only supported for 'center' target");
                }
                commands::reload::reload(&client).await
            }
            Commands::GenCerts(_) | Commands::Login { .. } => unreachable!(), // handled above
        }
    }
}
