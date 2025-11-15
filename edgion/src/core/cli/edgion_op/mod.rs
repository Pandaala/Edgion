use crate::core::conf_load::{Loader, LoaderArgs};
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
use crate::core::logging::{init_logging, LogConfig};
use crate::core::utils;
use crate::types::{COMPONENT_EDGION_OPERATOR, LOG_PREFIX_OPERATOR, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-operator",
    version,
    about = "Edgion Operator standalone executable",
    long_about = None
)]
pub struct EdgionOpCli {
    /// Optional gRPC listen address for operator
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Optional HTTP listen address for operator admin plane
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    /// Log directory
    #[arg(long, value_name = "DIR", default_value = "logs")]
    pub log_dir: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    pub log_level: String,

    /// Enable JSON log format
    #[arg(long)]
    pub log_json: bool,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

impl EdgionOpCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        // Initialize logging system
        let log_config = LogConfig {
            log_dir: PathBuf::from(&self.log_dir),
            file_prefix: LOG_PREFIX_OPERATOR.to_string(),
            json_format: self.log_json,
            console: true,
            level: self.log_level.clone(),
            buffer_size: 10_000,
        };
        
        init_logging(log_config).await?;

        // Log system startup
        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "system_start",
            version = VERSION,
            grpc_addr = ?self.grpc_listen,
            admin_addr = ?self.admin_listen,
            log_level = %self.log_level,
            "Edgion Operator starting"
        );

        let config_server = Arc::new(ConfigServer::new());
        let sync_server = ConfigSyncServer::new(config_server.clone());
        let loader = Loader::from_args(
            &self.loader,
            config_server as Arc<dyn crate::core::conf_sync::traits::EventDispatcher>,
        )?;

        let addr =
            utils::parse_listen_addr(self.grpc_listen.as_ref(), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;

        tracing::info!(
            component = COMPONENT_EDGION_OPERATOR,
            event = "services_starting",
            grpc_addr = %addr,
            "Starting gRPC server and configuration loader"
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
