use crate::core::cli::config::EdgionControllerConfig;
use crate::core::conf_mgr::{ConfCenter, ConfCenterConfig, ResourceMgrAPI, SchemaValidator};
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
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

    /// Build ConfCenterConfig from EdgionControllerConfig
    fn build_conf_center_config(config: &EdgionControllerConfig, k8s_mode: bool) -> Result<ConfCenterConfig> {
        if k8s_mode {
            let gateway_class = config
                .gateway_class()
                .ok_or_else(|| anyhow!("gateway_class must be specified in Kubernetes mode"))?;

            Ok(ConfCenterConfig::Kubernetes {
                watch_namespaces: config.watch_namespaces().to_vec(),
                label_selector: config.label_selector().map(|s| s.to_string()),
                gateway_class,
            })
        } else {
            Ok(ConfCenterConfig::FileSystem {
                conf_dir: PathBuf::from(config.conf_dir()),
                watch_enabled: false, // File watching not yet implemented
            })
        }
    }

    /// Initialize environment: k8s_mode, work_dir, logging
    /// Returns (k8s_mode, log_guard)
    async fn init_environment(&self, config: &EdgionControllerConfig) -> Result<(bool, WorkerGuard)> {
        // Detect and set K8s mode (must be done early)
        let k8s_mode = utils::detect_k8s_mode(self.config.k8s_mode, config.k8s_mode);
        utils::set_k8s_mode(k8s_mode);

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
            k8s_mode = k8s_mode,
            grpc_addr = %config.grpc_listen(),
            admin_addr = %config.admin_listen(),
            log_level = %config.log_level(),
            "Edgion Controller starting"
        );

        Ok((k8s_mode, log_guard))
    }

    /// Load CRD schemas for validation
    fn load_schemas() -> Arc<SchemaValidator> {
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "loading_schemas",
            "Loading CRD schemas for validation"
        );

        let schema_validator = Arc::new(
            SchemaValidator::from_crd_dir(Path::new("config/crd")).unwrap_or_else(|e| {
                tracing::warn!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "schema_load_warning",
                    error = %e,
                    "Failed to load CRD schemas, validation will be skipped"
                );
                SchemaValidator::empty()
            }),
        );

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "schemas_loaded",
            schema_count = schema_validator.schema_count(),
            "CRD schemas loaded"
        );

        schema_validator
    }

    /// Wait for all caches to be ready, with timeout
    async fn wait_all_ready(config_server: &Arc<ConfigServer>, timeout_secs: u64, k8s_mode: bool) {
        // FileSystem mode: caches are already marked ready by ConfCenter::start
        if !k8s_mode {
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "all_caches_ready",
                "FileSystem mode: all caches ready"
            );
            return;
        }

        // K8s mode: wait for controller to sync all caches
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        loop {
            if config_server.is_each_cache_ready() {
                config_server.set_all_ready();
                tracing::info!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "all_caches_ready",
                    elapsed_ms = start.elapsed().as_millis(),
                    "All caches are ready, set_all_ready called"
                );
                return;
            }

            if start.elapsed() > timeout {
                let not_ready = config_server.not_ready_caches();
                tracing::warn!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "wait_all_ready_timeout",
                    timeout_secs = timeout_secs,
                    not_ready = ?not_ready,
                    "Timeout waiting for caches, proceeding anyway"
                );
                config_server.set_all_ready();
                return;
            }

            let not_ready = config_server.not_ready_caches();
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "waiting_for_caches",
                not_ready = ?not_ready,
                elapsed_ms = start.elapsed().as_millis(),
                "Waiting for caches to be ready..."
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    /// Start gRPC and Admin services
    async fn start_services(
        config_server: Arc<ConfigServer>,
        resource_mgr: Arc<ResourceMgrAPI>,
        schema_validator: Arc<SchemaValidator>,
        grpc_addr: SocketAddr,
        admin_port: u16,
        k8s_mode: bool,
    ) -> Result<()> {
        let sync_server = ConfigSyncServer::new(config_server.clone());

        // Wait for all caches to be ready before starting services
        Self::wait_all_ready(&config_server, 30, k8s_mode).await;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %grpc_addr,
            admin_port = admin_port,
            "Starting gRPC and Admin API servers"
        );

        // Run both services concurrently
        let (sync_result, admin_result) = tokio::join!(
            sync_server.serve(grpc_addr),
            crate::core::api::controller::serve(
                config_server.clone(),
                Some(resource_mgr.clone()),
                schema_validator,
                admin_port
            )
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
    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionControllerConfig::load(self.config.clone())?;

        // 1. Initialize environment (k8s_mode, work_dir, logging)
        let (k8s_mode, _log_guard) = self.init_environment(&config).await?;

        // 2. Build ConfCenterConfig from EdgionControllerConfig
        let conf_center_config = Self::build_conf_center_config(&config, k8s_mode)?;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "conf_center_config",
            k8s_mode = k8s_mode,
            config = ?conf_center_config,
            "ConfCenter configuration built"
        );

        // 3. Create ConfCenter
        let conf_center = ConfCenter::create(conf_center_config).await?;

        // 4. Create ConfigServer
        let config_server = Arc::new(ConfigServer::new(&config.conf_sync));

        // 5. Create ResourceMgrAPI and register ConfWriter as backend
        let resource_mgr = Arc::new(ResourceMgrAPI::new());
        let backend_name = if k8s_mode { "kubernetes" } else { "filesystem" };
        resource_mgr.register_backend(backend_name.to_string(), conf_center.writer());
        if let Err(e) = resource_mgr.set_default_backend(backend_name.to_string()) {
            return Err(anyhow!("Failed to set default backend: {}", e));
        }

        // 6. Start ConfCenter (load resources + start watcher/controller)
        conf_center.start(config_server.clone()).await?;

        // 7. Load CRD schemas for validation
        let schema_validator = Self::load_schemas();

        // 8. Parse addresses and start services
        let grpc_addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_port = 5800;

        Self::start_services(config_server, resource_mgr, schema_validator, grpc_addr, admin_port, k8s_mode).await
    }
}
