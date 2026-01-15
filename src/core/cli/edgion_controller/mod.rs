use crate::core::cli::config::EdgionControllerConfig;
use crate::core::conf_mgr::{
    load_all_resources_from_store, ConfStore, FileSystemStore, KubernetesStore, ResourceMgrAPI, SchemaValidator,
};
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
use crate::core::observe::init_logging;
use crate::core::utils;
use crate::types::{init_work_dir, work_dir, COMPONENT_EDGION_CONTROLLER, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

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

    /// Wait for all caches to be ready, with timeout
    /// Once all caches are ready, set the global all_ready flag
    async fn wait_all_ready(config_server: &Arc<crate::core::conf_sync::ConfigServer>, timeout_secs: u64) {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        loop {
            if config_server.is_each_cache_ready() {
                // All caches are ready, set global all_ready flag
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
                // Timeout: still set all_ready to let system continue
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

    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionControllerConfig::load(self.config.clone())?;

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

        tracing::info!(
            work_dir = %wd.base().display(),
            "Work directory initialized"
        );

        // Initialize logging system
        let log_config = config.to_log_config();
        let _log_guard = init_logging(log_config).await?;

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

        // Get gateway_class_name from config
        let gateway_class_name = config
            .gateway_class()
            .ok_or_else(|| anyhow!("gateway_class must be specified in configuration"))?;

        // Select store based on k8s_mode
        let store: Arc<dyn ConfStore> = if k8s_mode {
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "store_init",
                "Running in Kubernetes mode, initializing KubernetesStore"
            );
            KubernetesStore::new().await?
        } else {
            let conf_dir = config.conf_dir();
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "store_init",
                conf_dir = %conf_dir,
                "Running in file system mode, using FileSystemStore"
            );
            FileSystemStore::new(&conf_dir) as Arc<dyn ConfStore>
        };

        // Create ConfigServer without base_conf (resources will be loaded dynamically)
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "config_server_init",
            gateway_class = gateway_class_name,
            "Initializing ConfigServer"
        );

        let config_server = Arc::new(ConfigServer::new(&config.conf_sync));
        let sync_server = ConfigSyncServer::new(config_server.clone());

        // Create ResourceMgrAPI and register backend
        let resource_mgr = Arc::new(ResourceMgrAPI::new());
        let backend_name = if k8s_mode { "kubernetes" } else { "filesystem" };
        resource_mgr.register_backend(backend_name.to_string(), store.clone());
        if let Err(e) = resource_mgr.set_default_backend(backend_name.to_string()) {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "resource_mgr_init_error",
                error = %e,
                "Failed to set default backend"
            );
            return Err(anyhow!("Failed to initialize resource manager: {}", e));
        }

        // Load all user resources from storage into ConfigServer (unified loading)
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "loading_user_conf",
            "Loading all user resources from storage"
        );
        if let Err(e) = load_all_resources_from_store(store.clone(), config_server.clone()).await {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "user_conf_load_error",
                error = %e,
                "Failed to load user resources from storage"
            );
            return Err(anyhow!("Failed to load user configuration: {}", e));
        }

        // If K8s mode, start the controller to watch for changes
        if k8s_mode {
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "k8s_controller_start",
                "Starting Kubernetes controller to watch resources"
            );

            let k8s_store = store
                .as_any()
                .downcast_ref::<KubernetesStore>()
                .expect("Store should be KubernetesStore in K8s mode");

            let k8s_store_arc = Arc::new(k8s_store.clone());

            let controller = crate::core::conf_mgr::conf_store::kubernetes::controller::KubernetesController::new(
                config_server.clone(),
                k8s_store_arc,
                gateway_class_name.to_string(),
                config.watch_namespaces().to_vec(),
                config.label_selector().map(|s| s.to_string()),
            )
            .await?;

            tokio::spawn(async move {
                if let Err(e) = controller.run().await {
                    tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "k8s_controller_error",
                        error = %e,
                        "Kubernetes controller encountered an error"
                    );
                }
            });
        }

        // Load CRD schemas for validation
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
                // Create an empty validator if CRD loading fails
                // This allows the system to continue without validation
                SchemaValidator::empty()
            }),
        );
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "schemas_loaded",
            schema_count = schema_validator.schema_count(),
            "CRD schemas loaded"
        );

        let addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;

        // Wait for all caches to be ready before starting services
        // This ensures data consistency - clients won't get partial data
        Self::wait_all_ready(&config_server, 30).await;

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %addr,
            "Starting gRPC conf_server and configuration loader"
        );

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
            crate::core::api::controller::serve(
                config_server.clone(),
                Some(resource_mgr.clone()),
                schema_validator,
                admin_port
            )
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
