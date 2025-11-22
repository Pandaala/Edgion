use crate::core::cli::config::EdgionOpConfig;
use crate::core::conf_load::Loader;
use crate::core::conf_sync::{ConfigServer, ConfigServerEventDispatcher, ConfigSyncServer};
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

    /// Spawn a background task to periodically print all gateway class configs every 10 seconds
    /// This can be easily removed in the future if not needed
    fn spawn_config_printer(config_server: Arc<ConfigServer>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                config_server.print_config().await;
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

        let loader_args = config.to_loader_args();
        let loader = Loader::from_args(&loader_args)?;

        // Get gateway_class_name from config
        let gateway_class_name = config.gateway_class()
            .ok_or_else(|| anyhow!("gateway_class must be specified in configuration"))?;

        // Load base configuration (GatewayClass, EdgionGatewayConfig, Gateway)
        tracing::info!(
            "Loading base configuration for gateway_class: {}",
            gateway_class_name
        );
        let base_conf = loader.load_base(&gateway_class_name).await?;

        // Print base configuration as pretty JSON
        if let Ok(json) = serde_json::to_string_pretty(&base_conf) {
            tracing::info!(
                "Base configuration loaded successfully:\n{}",
                json
            );
        }

        // Validate base configuration schema
        if let Err(e) = base_conf.validate_schema() {
            tracing::error!(
                component = COMPONENT_EDGION_OPERATOR,
                event = "schema_validation_failed",
                error = %e,
                "Base configuration schema validation failed: {}. Process will exit in 5 seconds.",
                e
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            std::process::exit(1);
        }

        let config_server = Arc::new(ConfigServer::new(base_conf));
        let sync_server = ConfigSyncServer::new(config_server.clone());

        // Register dispatcher before using the loader
        loader.register_dispatcher(config_server.clone() as Arc<dyn ConfigServerEventDispatcher>).await;

        let addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;

        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "services_starting",
            grpc_addr = %addr,
            "Starting gRPC server and configuration loader"
        );

        // Spawn task to print config every 10 seconds
        Self::spawn_config_printer(config_server.clone());

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
