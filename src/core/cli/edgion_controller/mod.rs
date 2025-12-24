use crate::core::cli::config::EdgionControllerConfig;
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
use crate::core::conf_mgr::{FileSystemStore, load_all_resources_from_store, load_base_conf_from_store, ResourceMgrAPI, SchemaValidator};
use crate::core::observe::init_logging;
use crate::core::utils;
use crate::types::{prefix_dir, COMPONENT_EDGION_CONTROLLER, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-controller",
    version,
    about = "Edgion Controller standalone executable",
    long_about = None
)]
pub struct EdgionControllerCli {
    #[command(flatten)]
    pub config: EdgionControllerConfig,
}

impl EdgionControllerCli {
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
        let config = EdgionControllerConfig::load(self.config.clone())?;

        // Initialize prefix directory (LazyLock auto-initializes on first access)
        let _ = prefix_dir();

        // Initialize logging system
        let log_config = config.to_log_config();
        let _log_guard = init_logging(log_config).await?;

        // Log system startup
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "system_start",
            version = VERSION,
            allocator = crate::allocator_name(),
            grpc_addr = %config.grpc_listen(),
            admin_addr = %config.admin_listen(),
            log_level = %config.log_level(),
            "Edgion Controller starting"
        );

        // Get gateway_class_name from config
        let gateway_class_name = config
            .gateway_class()
            .ok_or_else(|| anyhow!("gateway_class must be specified in configuration"))?;

        // Get configuration directory
        let conf_dir = config.conf_dir();
        
        // Load base configuration (GatewayClass, EdgionGatewayConfig, Gateway)
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "load_base_start",
            gateway_class = gateway_class_name,
            conf_dir = %conf_dir,
            "Loading base configuration"
        );
        
        let file_store = FileSystemStore::new(&conf_dir);
        let base_conf = load_base_conf_from_store(file_store.clone() as Arc<dyn crate::core::conf_mgr::ConfStore>, &gateway_class_name).await?;

        // Print base configuration as pretty JSON
        if let Ok(json) = serde_json::to_string_pretty(&base_conf) {
            tracing::info!("Base configuration loaded successfully:\n{}", json);
        }

        // Validate base configuration schema
        if let Err(e) = base_conf.validate_schema() {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "schema_validation_failed",
                error = %e,
                "Base configuration schema validation failed: {}. Process will exit in 5 seconds.",
                e
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            std::process::exit(1);
        }

        let config_server = Arc::new(ConfigServer::new(base_conf, &config.conf_sync));
        let sync_server = ConfigSyncServer::new(config_server.clone());
        
        // Create ResourceMgrAPI and register filesystem backend
        let resource_mgr = Arc::new(ResourceMgrAPI::new());
        resource_mgr.register_backend("filesystem".to_string(), file_store.clone() as Arc<dyn crate::core::conf_mgr::ConfStore>);
        if let Err(e) = resource_mgr.set_default_backend("filesystem".to_string()) {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "resource_mgr_init_error",
                error = %e,
                "Failed to set default backend"
            );
            return Err(anyhow!("Failed to initialize resource manager: {}", e));
        }
        
        // Load all user resources from storage into ConfigServer
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "loading_user_conf",
            "Loading all user resources from storage"
        );
        if let Err(e) = load_all_resources_from_store(file_store.clone() as Arc<dyn crate::core::conf_mgr::ConfStore>, config_server.clone()).await {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "user_conf_load_error",
                error = %e,
                "Failed to load user resources from storage"
            );
            return Err(anyhow!("Failed to load user configuration: {}", e));
        }
        
        // Load CRD schemas for validation
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "loading_schemas",
            "Loading CRD schemas for validation"
        );
        let schema_validator = Arc::new(
            SchemaValidator::from_crd_dir(Path::new("config/crd"))
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "schema_load_warning",
                        error = %e,
                        "Failed to load CRD schemas, validation will be skipped"
                    );
                    // Create an empty validator if CRD loading fails
                    // This allows the system to continue without validation
                    SchemaValidator::empty()
                })
        );
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "schemas_loaded",
            schema_count = schema_validator.schema_count(),
            "CRD schemas loaded"
        );

        let addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %addr,
            "Starting gRPC conf_server and configuration loader"
        );

        // Spawn task to print config every 10 seconds
        Self::spawn_config_printer(config_server.clone());

        // Admin API port
        let admin_port = 5800;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "admin_api_starting",
            admin_port = admin_port,
            "Starting Admin API server"
        );

        // Run both services concurrently using tokio::join!
        // Note: loader.run() is intentionally removed as we're using conf_store instead of file watcher
        let (sync_result, admin_result) = tokio::join!(
            sync_server.serve(addr),
            crate::core::api::controller::serve(config_server.clone(), Some(resource_mgr.clone()), schema_validator, admin_port)
        );

        // Check results - if any service fails, return error
        if let Err(e) = &sync_result {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "grpc_server_error",
                error = %e,
                "gRPC conf_server failed"
            );
        }

        if let Err(e) = &admin_result {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "admin_api_error",
                error = %e,
                "Admin API server failed"
            );
        }

        sync_result.map_err(|e| anyhow!("gRPC conf_server error: {}", e))?;
        admin_result?;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "system_shutdown",
            "Edgion Operator shutting down"
        );

        Ok(())
    }
}
