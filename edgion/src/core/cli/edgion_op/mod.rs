use crate::core::cli::config::EdgionOpConfig;
use crate::core::conf_load::Loader;
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
use crate::core::logging::init_logging;
use crate::core::utils;
use crate::types::{COMPONENT_EDGION_OPERATOR, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-operator",
    version,
    about = "Edgion Operator standalone executable",
    long_about = None
)]
pub struct EdgionOpCli {
    #[command(flatten)]
    pub config: EdgionOpConfig,
}

impl EdgionOpCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Spawn a background task to periodically print all gateway class configs in debug mode
    /// This can be easily removed in the future if not needed
    fn spawn_debug_config_printer(config_server: Arc<ConfigServer>, log_level: String, enabled: bool) {
        if !enabled {
            return;
        }
        
        tokio::spawn(async move {
            if log_level == "debug" || log_level == "trace" {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
                loop {
                    interval.tick().await;
                    config_server.print_config().await;
                }
            }
        });
    }

    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionOpConfig::load(self.config.clone())?;

        // Initialize logging system
        let log_config = config.to_log_config();
        init_logging(log_config).await?;

        // Log system startup
        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "system_start",
            version = VERSION,
            grpc_addr = %config.grpc_listen(),
            admin_addr = %config.admin_listen(),
            log_level = %config.log_level(),
            "Edgion Operator starting"
        );

        let config_server = Arc::new(ConfigServer::new(config.gateway_class()));
        let sync_server = ConfigSyncServer::new(config_server.clone());
        
        // Clone config_server before moving into loader
        let debug_config_server = config_server.clone();
        
        let loader_args = config.to_loader_args();
        let loader = Loader::from_args(
            &loader_args,
            config_server as Arc<dyn crate::core::conf_sync::traits::ConfigServerEventDispatcher>,
        )?;

        let addr = utils::parse_listen_addr(
            Some(&config.grpc_listen()),
            utils::DEFAULT_OPERATOR_GRPC_ADDR,
        )?;

        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "services_starting",
            grpc_addr = %addr,
            "Starting gRPC server and configuration loader"
        );

        // Spawn debug task to print config every 30 seconds in debug mode
        Self::spawn_debug_config_printer(
            debug_config_server,
            config.log_level(),
            config.debug.enabled,
        );

        // Run both services concurrently using tokio::join!
        let (sync_result, loader_result) = tokio::join!(
            sync_server.serve(addr),
            loader.run()
        );

        // Check results - if either service fails, return error
        if let Err(e) = &sync_result {
            tracing::error!(
                component = COMPONENT_EDGION_OPERATOR,
                event = "grpc_server_error",
                error = %e,
                "gRPC server failed"
            );
        }
        
        if let Err(e) = &loader_result {
            tracing::error!(
                component = COMPONENT_EDGION_OPERATOR,
                event = "loader_error",
                error = %e,
                "Configuration loader failed"
            );
        }

        sync_result.map_err(|e| anyhow!("gRPC server error: {}", e))?;
        loader_result?;

        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "system_shutdown",
            "Edgion Operator shutting down"
        );

        Ok(())
    }
}
