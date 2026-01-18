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
use std::sync::atomic::{AtomicBool, Ordering};
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
        // Determine k8s_mode from config
        let k8s_mode = config.is_k8s_mode();
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

        Ok(log_guard)
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
    /// Sets conf_center.set_all_ready() when complete (global state)
    async fn wait_all_ready(conf_center: &Arc<ConfCenter>, timeout_secs: u64) {
        let config_server = conf_center.config_server();
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        loop {
            if config_server.is_each_cache_ready() {
                conf_center.set_all_ready();
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
                conf_center.set_all_ready();
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
        conf_center: Arc<ConfCenter>,
        schema_validator: Arc<SchemaValidator>,
        grpc_addr: SocketAddr,
        admin_port: u16,
    ) -> Result<()> {
        let config_server = conf_center.config_server();
        let sync_server = ConfigSyncServer::new(config_server.clone());

        // Wait for all caches to be ready before starting services
        Self::wait_all_ready(&conf_center, 30).await;

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
            crate::core::api::controller::serve(conf_center.clone(), schema_validator, admin_port)
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

    /// Supervisor loop for handling relink scenarios
    /// 
    /// This loop monitors the Kubernetes controller and automatically
    /// triggers relink when:
    /// - 410 Gone is detected (resourceVersion expired)
    /// - Leader election is lost (in HA mode)
    async fn run_with_relink_loop(conf_center: Arc<ConfCenter>, all_ready: Arc<AtomicBool>) {
        const MAX_RELINK_RETRIES: u32 = 10;
        const RELINK_BACKOFF_BASE_SECS: u64 = 1;
        const RELINK_BACKOFF_MAX_SECS: u64 = 60;

        let mut relink_count: u32 = 0;

        loop {
            // Wait for controller to exit
            let exit_reason = conf_center.wait_for_exit().await;

            match exit_reason {
                Some(reason) => {
                    tracing::info!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "controller_exit",
                        reason = ?reason,
                        relink_count = relink_count,
                        "Controller exited"
                    );

                    if ConfCenter::needs_relink(&reason) {
                        relink_count += 1;

                        if relink_count > MAX_RELINK_RETRIES {
                            tracing::error!(
                                component = COMPONENT_EDGION_CONTROLLER,
                                event = "max_relink_retries",
                                relink_count = relink_count,
                                "Maximum relink retries exceeded, stopping supervisor"
                            );
                            break;
                        }

                        // Calculate backoff delay with exponential backoff
                        let backoff_secs = std::cmp::min(
                            RELINK_BACKOFF_BASE_SECS * 2u64.pow(relink_count.saturating_sub(1)),
                            RELINK_BACKOFF_MAX_SECS,
                        );

                        tracing::info!(
                            component = COMPONENT_EDGION_CONTROLLER,
                            event = "relink_scheduled",
                            relink_count = relink_count,
                            backoff_secs = backoff_secs,
                            "Scheduling relink with backoff"
                        );

                        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;

                        // Reset all_ready flag before relink
                        all_ready.store(false, Ordering::SeqCst);

                        // Perform relink
                        match conf_center.relink().await {
                            Ok(()) => {
                                tracing::info!(
                                    component = COMPONENT_EDGION_CONTROLLER,
                                    event = "relink_success",
                                    relink_count = relink_count,
                                    "Relink successful"
                                );
                                // Reset retry count on successful relink
                                // (or keep it to eventually stop on persistent issues)
                            }
                            Err(e) => {
                                tracing::error!(
                                    component = COMPONENT_EDGION_CONTROLLER,
                                    event = "relink_failed",
                                    error = %e,
                                    relink_count = relink_count,
                                    "Relink failed"
                                );
                                // Continue to next iteration which will try again
                            }
                        }
                    } else {
                        // Normal exit (Shutdown, etc.) - stop the loop
                        tracing::info!(
                            component = COMPONENT_EDGION_CONTROLLER,
                            event = "supervisor_stopping",
                            reason = ?reason,
                            "Supervisor loop stopping (normal exit)"
                        );
                        break;
                    }
                }
                None => {
                    // Not in K8s mode or no controller - just return
                    tracing::debug!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        "No controller exit to wait for"
                    );
                    break;
                }
            }
        }
    }

    /// Main entry point
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

        // 3. Create global all_ready flag
        let all_ready = Arc::new(AtomicBool::new(false));

        // 4. Create ConfCenter (internally creates ConfigServer with shared all_ready)
        let conf_center = Arc::new(ConfCenter::create(conf_center_config, &config.conf_sync, all_ready.clone()).await?);

        // 5. Link ConfCenter (load resources + start watcher/controller)
        conf_center.link().await?;

        // 6. Load CRD schemas for validation
        let schema_validator = Self::load_schemas();

        // 7. Parse addresses and start services
        let grpc_addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_port = 5800;

        // 8. Spawn supervisor loop for K8s mode (handles relink on 410 Gone / leadership loss)
        if conf_center.is_k8s_mode() {
            let supervisor_conf_center = conf_center.clone();
            let supervisor_all_ready = all_ready.clone();
            tokio::spawn(async move {
                Self::run_with_relink_loop(supervisor_conf_center, supervisor_all_ready).await;
            });
        }

        Self::start_services(conf_center, schema_validator, grpc_addr, admin_port).await
    }
}
