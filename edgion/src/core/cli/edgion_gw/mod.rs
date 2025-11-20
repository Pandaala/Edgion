use crate::core::conf_load::LoaderArgs;
use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::logging::{init_logging, LogConfig};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-gateway",
    version,
    about = "Edgion Gateway standalone executable",
    long_about = None
)]
pub struct EdgionGwCli {
    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// Operator gRPC address (e.g., http://127.0.0.1:50061)
    #[arg(long, value_name = "ADDR")]
    pub server_addr: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

impl EdgionGwCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Spawn a background task to periodically print all gateway class configs in debug mode
    /// This can be easily removed in the future if not needed
    fn spawn_debug_config_printer(config_client: Arc<ConfigClient>, enabled: bool) {
        if !enabled {
            return;
        }

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                config_client.print_config();
            }
        });
    }

    pub async fn run(&self) -> Result<()> {
        // Initialize logging system
        // Use RUST_LOG environment variable if set, otherwise default to "info"
        let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

        let log_config = LogConfig {
            log_dir: std::path::PathBuf::from("logs"),
            file_prefix: "edgion-gateway".to_string(),
            json_format: false,
            console: true,
            level: log_level.clone(),
            buffer_size: 10_000,
        };

        init_logging(log_config).await?;

        let server_addr = self
            .server_addr
            .as_deref()
            .ok_or_else(|| anyhow!("server_addr is required, please provide --server-addr"))?;

        let mut sync_client = ConfigSyncClient::new(
            server_addr,
            self.gateway_class.clone(),
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
        )
        .await?;

        // Initialize base configuration and sync all resources
        sync_client.init().await?;

        // Start watching for changes
        sync_client.start_watch_all().await?;

        let mut gateway = EdgionGw::new(sync_client);

        // Get config_client for debug printing
        let config_client = gateway.config_client();

        // Spawn debug task to print config every 10 seconds in debug mode
        // Check log level to determine if debug mode is enabled
        let debug_enabled = log_level.to_lowercase().contains("debug") || log_level.to_lowercase().contains("trace");

        Self::spawn_debug_config_printer(config_client, debug_enabled);

        tracing::info!(server_addr = server_addr, "Connected to operator");

        gateway.serve().await?;
        gateway.shutdown().await;

        Ok(())
    }
}

pub struct EdgionGw {
    sync_client: ConfigSyncClient,
}

impl EdgionGw {
    pub fn new(sync_client: ConfigSyncClient) -> Self {
        Self { sync_client }
    }

    pub async fn serve(&self) -> Result<()> {
        tracing::info!("Gateway started, waiting for shutdown signal");

        signal::ctrl_c().await.expect("failed to listen for ctrl_c signal");

        tracing::info!("Shutdown signal received");
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        // ConfigSyncClient will be automatically dropped when EdgionGw is dropped
    }

    pub fn config_client(&self) -> Arc<ConfigClient> {
        self.sync_client.get_config_client()
    }
}
