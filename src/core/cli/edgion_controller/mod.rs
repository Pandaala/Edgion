use crate::core::api::controller::serve as serve_admin_api;
use crate::core::cli::config::EdgionControllerConfig;
use crate::core::conf_mgr::{ConfCenter, SchemaValidator};
use crate::core::conf_sync::ConfigSyncServer;
use crate::core::observe::init_logging;
use crate::core::utils;
use crate::types::{init_work_dir, work_dir, COMPONENT_EDGION_CONTROLLER, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_appender::non_blocking::WorkerGuard;

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

    /// Initialize environment: work_dir, logging
    /// Returns log_guard
    async fn init_environment(&self, config: &EdgionControllerConfig) -> Result<WorkerGuard> {
        // Determine work_dir (priority: CLI > ENV > Config > Default)
        let work_dir_path = self
            .config
            .work_dir
            .clone()
            .or_else(|| std::env::var("EDGION_WORK_DIR").ok().map(PathBuf::from))
            .or_else(|| config.work_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        // Initialize and validate work_dir
        init_work_dir(work_dir_path).map_err(|e| anyhow!("Failed to initialize work directory: {}", e))?;
        let wd = work_dir();
        wd.validate()
            .map_err(|e| anyhow!("Work directory validation failed: {}", e))?;

        tracing::info!(work_dir = %wd.base().display(), "Work directory initialized");

        // Initialize logging system
        let log_config = config.to_log_config();
        let log_guard = init_logging(log_config).await?;

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

        Ok(log_guard)
    }

    /// Load CRD schemas for validation
    ///
    /// In K8s mode: Skip schema loading, validation is handled by K8s API Server
    /// In non-K8s mode: Load schemas from CRD files, exit if loading fails
    fn load_schemas(k8s_mode: bool) -> Arc<SchemaValidator> {
        if k8s_mode {
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "skip_schema_loading",
                "K8s mode: schema validation handled by K8s API Server"
            );
            return Arc::new(SchemaValidator::empty());
        }

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "loading_schemas",
            "Non-K8s mode: loading CRD schemas for validation"
        );

        let schema_validator = SchemaValidator::from_crd_dir(Path::new("config/crd")).unwrap_or_else(|e| {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "schema_load_failed",
                error = %e,
                "FATAL: Failed to load CRD schemas in non-K8s mode"
            );
            std::process::exit(1);
        });

        let count = schema_validator.schema_count();
        if count == 0 {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "no_schemas_loaded",
                "FATAL: No schemas loaded in non-K8s mode"
            );
            std::process::exit(1);
        }

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "schemas_loaded",
            schema_count = count,
            "CRD schemas loaded successfully"
        );

        Arc::new(schema_validator)
    }

    /// Start gRPC and Admin services
    ///
    /// Note: Services can start immediately. When ConfigServer is not ready,
    /// they will return UNAVAILABLE errors until ConfCenter.start() completes.
    async fn start_services(
        conf_center: Arc<ConfCenter>,
        schema_validator: Arc<SchemaValidator>,
        grpc_addr: SocketAddr,
        admin_addr: SocketAddr,
    ) -> Result<()> {
        // Create ConfigSyncServer with ConfCenter (not ConfigServer directly)
        // It will dynamically get ConfigServer when handling requests
        let sync_server = ConfigSyncServer::new(conf_center.clone());

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %grpc_addr,
            admin_addr = %admin_addr,
            "Starting gRPC and Admin API servers"
        );

        // Run both services concurrently
        let (sync_result, admin_result) = tokio::join!(
            sync_server.serve(grpc_addr),
            serve_admin_api(conf_center.clone(), schema_validator, admin_addr)
        );

        // Check results
        if let Err(e) = &sync_result {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "grpc_server_error",
                error = %e,
                "gRPC server failed"
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

        sync_result.map_err(|e| anyhow!("gRPC server error: {}", e))?;
        admin_result?;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "system_shutdown",
            "Edgion Controller shutting down"
        );

        Ok(())
    }

    /// Main entry point
    ///
    /// Architecture:
    /// 1. Initialize environment
    /// 2. Create ConfCenter
    /// 3. Spawn ConfCenter.start() (manages lifecycle: leader election, link, relink)
    /// 4. Start gRPC and Admin services (immediately, they return UNAVAILABLE until ready)
    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionControllerConfig::load(self.config.clone())?;

        // 1. Initialize environment (work_dir, logging)
        let _log_guard = self.init_environment(&config).await?;

        // 2. Get ConfCenterConfig directly from config
        let conf_center_config = config.conf_center.clone();

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "conf_center_config",
            k8s_mode = config.is_k8s_mode(),
            config = ?conf_center_config,
            "ConfCenter configuration"
        );

        // 3. Create ConfCenter (ConfigServer is None initially)
        let conf_center = Arc::new(ConfCenter::create(conf_center_config, &config.conf_sync).await?);

        // 4. Spawn ConfCenter.start() in background
        // This manages the entire lifecycle: leader election, link, relink
        let start_conf_center = conf_center.clone();
        tokio::spawn(async move {
            if let Err(e) = start_conf_center.start().await {
                tracing::error!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "conf_center_start_error",
                    error = %e,
                    "ConfCenter start failed"
                );
            }
        });

        // 5. Load CRD schemas for validation (skip in K8s mode)
        let schema_validator = Self::load_schemas(config.is_k8s_mode());

        // 6. Parse addresses and start services
        // Note: Services start immediately but return UNAVAILABLE until ConfigServer is ready
        let grpc_addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_addr = utils::parse_listen_addr(Some(&config.admin_listen()), "0.0.0.0:8080")?;

        Self::start_services(conf_center, schema_validator, grpc_addr, admin_addr).await
    }
}
