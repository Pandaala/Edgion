use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::logging::{init_logging, LogConfig};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;

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
}

impl EdgionGwCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Spawn a background task to periodically print all gateway class configs every 10 seconds
    /// This can be easily removed in the future if not needed
    fn spawn_config_printer(config_client: Arc<ConfigClient>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
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

        // Use temp directory for logs to avoid permission issues
        let log_dir = std::env::temp_dir().join("edgion-gw-logs");
        
        let log_config = LogConfig {
            log_dir,
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

        let config_client = sync_client.get_config_client();
        Self::spawn_config_printer(config_client.clone());

        // Keep the program running indefinitely
        tracing::info!("Gateway is running. Press Ctrl+C to exit.");
        
        // Wait indefinitely until the program is terminated
        tokio::signal::ctrl_c().await?;
        
        tracing::info!("Received shutdown signal, exiting...");
        Ok(())
    }
}

