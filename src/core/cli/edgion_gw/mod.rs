use crate::core::conf_sync::conf_client::{ConfigClient, ConfigSyncClient};
use crate::core::gateway::gateway_base::GatewayBase;
use crate::core::observe::init_logging;
use crate::types::{init_prefix_dir, DEFAULT_PREFIX_DIR};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod log_config;
use log_config::GatewayLogConfig;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-gateway",
    version,
    about = "Edgion Gateway standalone executable",
    long_about = None
)]
pub struct EdgionGwCli {
    /// Prefix directory for Edgion (default: /usr/local/edgion)
    #[arg(short = 'p', long, value_name = "DIR", default_value = DEFAULT_PREFIX_DIR)]
    pub prefix_dir: PathBuf,

    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// Operator gRPC address (e.g., http://127.0.0.1:50061)
    #[arg(long, value_name = "ADDR")]
    pub server_addr: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    /// Log directory path (defaults to <prefix_dir>/logs if not specified)
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,
}

impl EdgionGwCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Spawn a background task to periodically print all gateway class configs every 10 seconds
    /// This can be easily removed in the future if not needed
    fn spawn_config_printer(config_client: Arc<ConfigClient>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                config_client.print_config();
            }
        });
    }

    /// Async bootstrap function that handles all async initialization
    /// 
    /// Returns a tuple of (GatewayBase, WorkerGuard).
    /// The WorkerGuard MUST be kept alive for logging to work properly.
    async fn bootstrap(&self) -> Result<(Arc<GatewayBase>, tracing_appender::non_blocking::WorkerGuard)> {
        // Initialize and create prefix directory
        init_prefix_dir(&self.prefix_dir)
            .map_err(|e| anyhow!("Failed to create prefix directory {:?}: {}", &self.prefix_dir, e))?;
        
        // Initialize logging system with configuration
        let log_config = GatewayLogConfig::new(self.log_dir.clone());
        
        // Validate configuration
        log_config.validate()?;
        
        tracing::info!("Logging initialized at: {:?}", log_config.log_dir);
        
        // Initialize logging and get the WorkerGuard
        // The guard owns a background thread that performs actual file writes
        let log_guard = init_logging(log_config.to_log_config()).await?;

        let server_addr = self
            .server_addr
            .as_deref()
            .ok_or_else(|| anyhow!("server_addr is required, please provide --conf_server-addr"))?;

        let mut sync_client = ConfigSyncClient::new(
            server_addr,
            self.gateway_class.clone(),
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
        )
        .await?;

        // Get config_client and set as global immediately
        let config_client = sync_client.get_config_client();
        crate::core::conf_sync::init_global_config_client(config_client.clone())
            .map_err(|e| anyhow!("Failed to initialize global config client: {}", e))?;

        // Initialize base configuration and sync all resources
        sync_client.init_base_conf().await?;

        // Initialize GatewayBase and bootstrap (before start_watch_all)
        let base_conf = config_client.get_base_conf()
            .ok_or_else(|| anyhow!("Base configuration not available"))?;

        // bootstrap Gateway
        let gateway = Arc::new(GatewayBase::new(base_conf));
        gateway.bootstrap()?;
        tracing::info!("Gateway bootstrap completed successfully");

        // Start watching for changes
        sync_client.start_watch_all().await?;

        Self::spawn_config_printer(config_client.clone());

        // Spawn Admin API server in background
        let config_client_for_admin = config_client.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::core::api::gw::serve(config_client_for_admin, 5900).await {
                tracing::error!(
                    component = "admin_api_gw",
                    event = "server_error",
                    error = %e,
                    "Gateway Admin API server failed"
                );
            }
        });

        tracing::info!(
            component = "admin_api_gw",
            event = "server_spawned",
            port = 5900,
            "Gateway Admin API server spawned"
        );

        // Wait for all caches to be ready
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match config_client.is_ready() {
                Ok(()) => {
                    tracing::info!("All caches are ready");
                    break;
                }
                Err(msg) => {
                    tracing::info!("{}", msg);
                }
            }
        }
        
        Ok((gateway, log_guard))
    }

    pub fn run(&self) -> Result<()> {
        // Create a Tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new()?;
        
        // Bootstrap all async operations in one block_on call
        // Keep the log_guard alive for the entire lifetime of the application
        let (gateway, _log_guard) = runtime.block_on(self.bootstrap())?;
        
        // Move the Tokio runtime to a background thread for async tasks
        // (config printer, config watchers, etc.)
        std::thread::spawn(move || {
            runtime.block_on(async {
                std::future::pending::<()>().await;
            });
        });
        
        // Run Pingora in the MAIN thread to properly handle system signals
        // This ensures graceful shutdown (Ctrl+C, SIGTERM) and zero-downtime upgrades work correctly
        // With tracing-appender, logging works from any thread without runtime context dependency
        gateway.run_forever();
        
        Ok(())
    }
}

