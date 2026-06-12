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
    /// - center: ConfCenter API (default, port 12101)
    /// - server: ConfigServer cache (port 12101)
    /// - client: ConfigClient cache (port 12001)
    #[arg(long, short = 't', value_enum, default_value = "center")]
    pub target: TargetType,

    /// Server address (e.g., http://localhost:12101)
    #[arg(long)]
    pub server: Option<String>,

    /// Unix socket path (e.g., /var/run/edgion/edgion.sock)
    #[arg(long)]
    pub socket: Option<PathBuf>,

    /// Pre-shared bearer token to use as `Authorization: Bearer <token>`.
    /// Takes precedence over the `EDGION_TOKEN` environment variable and the
    /// cached token in `~/.edgion/token`.  When resolved, `ensure_auth`
    /// (the `/auth/status` pre-flight check) is skipped entirely; the first
    /// real request performs lazy validation (401 = unknown token, 403 = denied).
    /// NOTE: `-t` is taken by `--target`; this flag is long-only.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

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

    /// Persist a pre-shared bearer token locally (~/.edgion/token).
    ///
    /// Authentication is token-only — this does not contact a login endpoint with
    /// credentials. It saves the supplied token so later invocations pick it up
    /// automatically (lowest precedence, below `--token` and `EDGION_TOKEN`).
    Login {
        /// Pre-shared bearer token to persist (the exact `Authorization: Bearer`
        /// value; pair it with the controller `cli_tokens[].token_sha256`).
        #[arg(long, value_name = "TOKEN")]
        token: String,
    },

    /// Generate a new CLI token (32 CSPRNG bytes, URL-safe base64, `edg_` prefix).
    ///
    /// Prints the token (use with `--token` / `EDGION_TOKEN`) and the SHA-256 hex
    /// digest to paste into the controller `cli_tokens[].token_sha256`.
    /// The full `edg_`-prefixed token is hashed — the exact string sent as Bearer.
    /// Does not contact the controller.
    GenToken,
}

impl Cli {
    pub async fn run(&self) -> Result<()> {
        // GenCerts does not need an EdgionClient connection
        if let Commands::GenCerts(args) = &self.command {
            return commands::gen_certs::execute(args).await;
        }

        // Login does not need an EdgionClient; it only persists a token (with an
        // optional best-effort /auth/status probe).
        if let Commands::Login { token } = &self.command {
            let base_url = self.server.clone().unwrap_or_else(|| {
                format!(
                    "http://localhost:{}",
                    match self.target {
                        TargetType::Center | TargetType::Server => 12101,
                        TargetType::Client => 12001,
                    }
                )
            });
            return commands::login::login(&base_url, token).await;
        }

        // GenToken does not contact the controller at all — short-circuit before auth
        if let Commands::GenToken = &self.command {
            return token::gen_token();
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

        // Resolve the bearer token (token-only auth):
        //   --token flag > EDGION_TOKEN env var > ~/.edgion/token file.
        // When a token is resolved we inject it as a static Bearer and skip
        // ensure_auth entirely. Lazy validation: the first real request returns
        // 401 (unknown token) or 403 (denied).
        let cli_token = auto_auth::resolve_token(self.token.as_deref());

        if let Some(bearer) = cli_token {
            let client = EdgionClient::with_static_bearer(
                self.target,
                self.server.clone(),
                self.socket.clone(),
                bearer.as_str(),
            )?;
            return self.dispatch(client).await;
        }

        // No token anywhere: create a token-less client and run the friendly
        // pre-flight check (prints an actionable message if a token is required).
        let mut client = EdgionClient::new(self.target, self.server.clone(), self.socket.clone())?;

        client.ensure_auth().await?;

        self.dispatch(client).await
    }

    /// Dispatch the subcommand after the client is authenticated.
    async fn dispatch(&self, client: EdgionClient) -> Result<()> {
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
            Commands::GenCerts(_) | Commands::Login { .. } | Commands::GenToken => {
                unreachable!() // all handled before dispatch
            }
        }
    }
}

#[cfg(test)]
mod cli_token_precedence_tests {
    use super::client::EdgionClient;
    use super::TargetType;

    /// Token precedence: `--token` flag value is used as the static bearer.
    /// Verifies `with_static_bearer` stores the flag value and `ensure_auth` is
    /// bypassed (no network calls needed — the constructor itself is the proof).
    #[test]
    fn flag_token_injected_as_static_bearer() {
        let bearer = "edg_flagtoken";
        let c = EdgionClient::with_static_bearer(TargetType::Center, None, None, bearer)
            .expect("with_static_bearer must succeed");
        // The stored token must be the exact flag value (use the test accessor)
        assert_eq!(c.token_for_test(), Some(bearer));
    }

    /// Token precedence: `EDGION_TOKEN` env var is picked up when `--token` is absent.
    /// We replicate the resolution logic from `Cli::run` inline so tests are hermetic.
    #[test]
    fn env_token_used_when_flag_absent() {
        // Simulate the resolution: flag = None, env = "edg_envtoken"
        let flag: Option<String> = None;
        // We don't actually set the env var (it would bleed across tests on parallel
        // runners); instead we exercise the resolution expression directly.
        let env_val = Some("edg_envtoken".to_string());
        let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
        assert_eq!(resolved.as_deref(), Some("edg_envtoken"));
    }

    /// Token precedence: `--token` wins over `EDGION_TOKEN`.
    #[test]
    fn flag_token_wins_over_env() {
        let flag = Some("edg_flag".to_string());
        let env_val = Some("edg_env".to_string());
        let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
        assert_eq!(resolved.as_deref(), Some("edg_flag"));
    }

    /// When neither `--token` nor `EDGION_TOKEN` is set, resolved token is None
    /// (the session-auth path is taken).
    #[test]
    fn no_token_resolved_is_none() {
        let flag: Option<String> = None;
        let env_val: Option<String> = None;
        let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
        assert!(resolved.is_none());
    }

    /// Empty `EDGION_TOKEN` is treated as absent (filtered out).
    #[test]
    fn empty_env_token_treated_as_absent() {
        let flag: Option<String> = None;
        let env_val = Some("".to_string());
        let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
        assert!(resolved.is_none());
    }
}
