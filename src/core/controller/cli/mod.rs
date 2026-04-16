use crate::core::common::config::ConfSyncSecurityConfig;
use crate::core::common::{config::EdgionControllerConfig, utils};
use crate::core::controller::api::{create_admin_router, serve_with_shutdown as serve_admin_api_with_shutdown};
use crate::core::controller::conf_mgr::conf_center::traits::CenterLifeCycle;
use crate::core::controller::conf_mgr::{ConfMgr, SchemaValidator, ShutdownHandle};
use crate::core::controller::conf_sync::{ConfigSyncGrpcServer, ConfigSyncServerProvider};
use crate::core::controller::observe::init_logging;
use crate::types::{init_work_dir, work_dir, COMPONENT_EDGION_CONTROLLER, VERSION};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_appender::non_blocking::WorkerGuard;

/// Provider that wraps ConfMgr to dynamically provide ConfigSyncServer
///
/// This allows the gRPC server to always use the latest ConfigSyncServer
/// instance, which is important for reload operations.
struct ConfMgrProvider {
    conf_mgr: Arc<ConfMgr>,
}

impl ConfigSyncServerProvider for ConfMgrProvider {
    fn config_sync_server(&self) -> Option<Arc<crate::core::controller::conf_sync::ConfigSyncServer>> {
        self.conf_mgr.config_sync_server()
    }
}

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

        let crd_path = work_dir().resolve("config/crd");
        let schema_validator = SchemaValidator::from_crd_dir(&crd_path).unwrap_or_else(|e| {
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

    /// Start gRPC and Admin services with graceful shutdown support
    ///
    /// Note: Services can start immediately. When ConfigSyncServer is not ready,
    /// they will return UNAVAILABLE errors until ConfMgr.start() completes.
    ///
    /// The gRPC server uses a provider pattern to dynamically obtain the latest
    /// ConfigSyncServer instance, which allows it to use the new instance after
    /// reload operations without restarting the server.
    async fn start_services(
        conf_mgr: Arc<ConfMgr>,
        schema_validator: Arc<SchemaValidator>,
        grpc_addr: SocketAddr,
        admin_addr: SocketAddr,
        security: ConfSyncSecurityConfig,
        auth_config: Option<crate::core::common::auth::AdminAuthConfig>,
        local_auth_config: Option<crate::core::common::local_auth::LocalAuthConfig>,
        shutdown_handle: ShutdownHandle,
    ) -> Result<()> {
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %grpc_addr,
            admin_addr = %admin_addr,
            "Starting gRPC and Admin API servers with graceful shutdown support"
        );

        // Create shutdown signals with ordered shutdown:
        // Admin drains immediately, gRPC continues serving for 2s to let clients finish last sync.
        let admin_shutdown = {
            let mut signal = shutdown_handle.signal();
            async move { signal.wait().await }
        };
        let grpc_shutdown = {
            let mut signal = shutdown_handle.signal();
            async move {
                signal.wait().await;
                // Brief delay so Admin API finishes first — gRPC clients can complete final sync
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        };

        // Create provider that dynamically obtains ConfigSyncServer from ConfMgr
        // This allows the gRPC server to use the latest instance after reload
        let provider = Arc::new(ConfMgrProvider {
            conf_mgr: conf_mgr.clone(),
        });

        // Create shutdown token for gRPC stream tasks
        let grpc_shutdown_token = CancellationToken::new();

        // Bridge: cancel token when ShutdownHandle fires
        {
            let mut signal = shutdown_handle.signal();
            let token = grpc_shutdown_token.clone();
            tokio::spawn(async move {
                signal.wait().await;
                token.cancel();
            });
        }

        // Create gRPC server with dynamic provider
        // Note: gRPC server will return UNAVAILABLE until ConfigSyncServer is ready
        let grpc_server = ConfigSyncGrpcServer::new(provider, grpc_shutdown_token);

        // Run both services concurrently with shutdown support and global timeout.
        // In K8s, kubelet sends SIGKILL after terminationGracePeriodSeconds (default 30s).
        // We use 25s to leave headroom. The 2s gRPC delay is included in this budget.
        //
        // IMPORTANT: the 25s timeout must start from when the shutdown SIGNAL fires,
        // not from server startup. Using `tokio::select!` instead of a top-level
        // `tokio::time::timeout` ensures the countdown only begins after SIGTERM/SIGINT.
        let serve_fut = async {
            tokio::join!(
                grpc_server.serve_with_shutdown(grpc_addr, &security, grpc_shutdown),
                serve_admin_api_with_shutdown(
                    conf_mgr.clone(),
                    schema_validator,
                    admin_addr,
                    auth_config.as_ref(),
                    local_auth_config.as_ref(),
                    admin_shutdown
                )
            )
        };
        let drain_timeout_fut = {
            let mut signal = shutdown_handle.signal();
            async move {
                signal.wait().await;
                tokio::time::sleep(std::time::Duration::from_secs(25)).await;
            }
        };
        tokio::select! {
            (sync_result, admin_result) = serve_fut => {
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
                sync_result?;
                admin_result?;
            }
            _ = drain_timeout_fut => {
                tracing::warn!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    "Graceful shutdown timed out 25s after signal, forcing exit"
                );
            }
        }

        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "system_shutdown",
            "Edgion Controller shutting down gracefully"
        );

        Ok(())
    }

    /// Main entry point
    ///
    /// Architecture:
    /// 1. Initialize environment
    /// 2. Create shutdown handler for graceful shutdown
    /// 3. Create ConfMgr
    /// 4. Spawn ConfMgr.start() (manages lifecycle: leader election, link, relink)
    /// 5. Load CRD schemas (before federation client so router can be created)
    /// 6. Start federation client with admin router (HTTP-over-gRPC proxy)
    /// 7. Start gRPC and Admin services with shutdown support
    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionControllerConfig::load(self.config.clone())?;

        // Initialize global controller config for runtime access (e.g., by ReferenceGrant validator)
        crate::core::common::config::init_controller_config(config.clone());

        // Initialize test mode if enabled via CLI --test-mode
        // This affects: endpoint_mode (forced to Both), metrics test features
        crate::core::common::config::init_global_test_mode(self.config.test_mode);

        // 1. Initialize environment (work_dir, logging)
        let _log_guard = self.init_environment(&config).await?;

        // 2. Log ConfMgr configuration
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "conf_mgr_config",
            k8s_mode = config.is_k8s_mode(),
            config = ?config.conf_center,
            "ConfMgr configuration"
        );

        // 3. Create shutdown handler for graceful shutdown (SIGINT/SIGTERM)
        let shutdown_handle = ShutdownHandle::new();
        let signal_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_shutdown.wait_for_signals().await;
        });

        // 4. Create ConfMgr (ConfigSyncServer is None initially)
        let conf_center_config = config.conf_center.clone();
        let conf_mgr = Arc::new(ConfMgr::create(conf_center_config).await?);

        // 5. Spawn ConfMgr.start_with_shutdown() in background
        // Pass the shutdown_handle so ConfMgr uses the same signal source
        // This avoids duplicate signal handlers and ensures coordinated shutdown
        let start_conf_mgr = conf_mgr.clone();
        let conf_mgr_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            if let Err(e) = start_conf_mgr.start_with_shutdown(conf_mgr_shutdown).await {
                tracing::error!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "conf_mgr_start_error",
                    error = %e,
                    "ConfMgr start failed"
                );
            }
        });

        // 6. Load CRD schemas for validation (skip in K8s mode).
        // Must happen before federation client so the base admin router can be created.
        let schema_validator = Self::load_schemas(config.is_k8s_mode());

        // 7. Start federation client if center config is present.
        // Must be after conf_mgr.start() so processor_registry is populated.
        // The base admin router (without auth middleware) is passed so the client
        // can proxy HTTP requests from Center directly to the local admin API.
        let _fed_shutdown_tx = if let Some(center_config) = config.center.clone() {
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

            let center_api: Arc<dyn crate::core::controller::conf_mgr::conf_center::traits::CenterApi> =
                conf_mgr.clone();

            let kinds: Vec<String> = conf_mgr
                .runtime()
                .processor_registry
                .all_kinds()
                .iter()
                .map(|s| s.to_string())
                .collect();

            // Create the base admin router without auth — Center is trusted, auth handled externally
            let admin_router = create_admin_router(conf_mgr.clone(), schema_validator.clone());

            tokio::spawn(crate::core::controller::fed_sync::fed_client::run(
                center_config,
                center_api,
                kinds,
                shutdown_rx,
                admin_router,
                conf_mgr.clone(),
            ));

            Some(shutdown_tx) // kept alive for duration of process
        } else {
            None
        };

        // 8. Parse addresses and start services with shutdown support
        // Note: Services start immediately but return UNAVAILABLE until ConfigServer is ready
        let grpc_addr = utils::parse_listen_addr(Some(&config.grpc_listen()), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_addr = utils::parse_listen_addr(Some(&config.admin_listen()), "0.0.0.0:8080")?;
        let security = config.conf_sync_security.clone();
        security.validate()?;
        let auth_config = config.auth.clone();

        // Resolve local_auth: if [local_auth] is present with real credentials use it as-is;
        // if it has empty password/secret, or if there is no [local_auth] at all (and no OIDC),
        // auto-generate credentials and store them as a K8s Secret via CenterApi.
        let local_auth_config = if let Some(cfg) = config.local_auth.clone() {
            if cfg.needs_generation() {
                // [local_auth] exists but password/secret is empty — use Secret
                match crate::core::common::local_auth::secret::ensure_admin_credentials(&*conf_mgr).await {
                    Ok(generated) => Some(generated),
                    Err(e) => {
                        tracing::error!(
                            component = COMPONENT_EDGION_CONTROLLER,
                            error = %e,
                            "Failed to ensure admin credentials Secret"
                        );
                        None
                    }
                }
            } else {
                Some(cfg)
            }
        } else if config.auth.is_none() {
            // No [auth] and no [local_auth] — auto-generate via Secret
            match crate::core::common::local_auth::secret::ensure_admin_credentials(&*conf_mgr).await {
                Ok(generated) => Some(generated),
                Err(e) => {
                    tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        error = %e,
                        "Failed to ensure admin credentials Secret"
                    );
                    None
                }
            }
        } else {
            None // OIDC auth
        };

        Self::start_services(
            conf_mgr,
            schema_validator,
            grpc_addr,
            admin_addr,
            security,
            auth_config,
            local_auth_config,
            shutdown_handle,
        )
        .await
    }
}
