use crate::api::{create_admin_router, serve_with_shutdown as serve_admin_api_with_shutdown};
use crate::conf_mgr::conf_center::traits::CenterLifeCycle;
use crate::conf_mgr::{ConfMgr, SchemaValidator, ShutdownHandle};
use crate::conf_sync::{ConfigSyncGrpcServer, ConfigSyncServerProvider};
use crate::config::EdgionControllerConfig;
use crate::observe::init_logging;
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_resources::{init_work_dir, work_dir, COMPONENT_EDGION_CONTROLLER, VERSION};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing_appender::non_blocking::WorkerGuard;

/// Resolve the raw local admin token: `EDGION_ADMIN_TOKEN` env wins over the
/// `admin_token` config field. The returned value is unvalidated; blank/whitespace
/// handling (and rejection) happens in `authz::build_admin_token_entry`.
pub(crate) fn resolve_admin_token_raw(config: &crate::config::EdgionControllerConfig) -> Option<String> {
    std::env::var("EDGION_ADMIN_TOKEN")
        .ok()
        .or_else(|| config.admin_token.clone())
}

/// Provider that wraps ConfMgr to dynamically provide ConfigSyncServer
///
/// This allows the gRPC server to always use the latest ConfigSyncServer
/// instance, which is important for reload operations.
struct ConfMgrProvider {
    conf_mgr: Arc<ConfMgr>,
}

impl ConfigSyncServerProvider for ConfMgrProvider {
    fn config_sync_server(&self) -> Option<Arc<crate::conf_sync::ConfigSyncServer>> {
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
        probe_addr: SocketAddr,
        metrics_addr: SocketAddr,
        security: ConfSyncSecurityConfig,
        config_file: Option<String>,
        authorizer: std::sync::Arc<crate::authz::Authorizer>,
        center_reload_tx: tokio::sync::watch::Sender<crate::config::CenterClientConfig>,
        allow_admin_ips: Vec<String>,
        shutdown_handle: ShutdownHandle,
    ) -> Result<()> {
        tracing::info!(
            component = COMPONENT_EDGION_CONTROLLER,
            event = "services_starting",
            grpc_addr = %grpc_addr,
            admin_addr = %admin_addr,
            probe_addr = %probe_addr,
            metrics_addr = %metrics_addr,
            "Starting gRPC, Admin API, probe, and metrics servers with graceful shutdown support"
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
                // Brief delay so Admin API finishes first -- gRPC clients can complete final sync
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

        // Dedicated probe listener (liveness/readiness) — own socket, no auth.
        {
            let probe_state = crate::api::build_admin_state(
                conf_mgr.clone(),
                schema_validator.clone(),
                config_file.clone(),
                authorizer.clone(),
                center_reload_tx.clone(),
            );
            let mut probe_signal = shutdown_handle.signal();
            tokio::spawn(async move {
                let app = crate::api::create_probe_router(probe_state);
                match tokio::net::TcpListener::bind(probe_addr).await {
                    Ok(listener) => {
                        let _ = axum::serve(listener, app)
                            .with_graceful_shutdown(async move { probe_signal.wait().await })
                            .await;
                    }
                    Err(e) => tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "probe_listener_error",
                        error = %e,
                        addr = %probe_addr,
                        "Controller probe listener failed to bind"
                    ),
                }
            });
        }

        // Dedicated metrics listener (Prometheus scrape) — own socket, no auth.
        {
            let mut metrics_signal = shutdown_handle.signal();
            tokio::spawn(async move {
                let app = crate::api::create_metrics_router();
                match tokio::net::TcpListener::bind(metrics_addr).await {
                    Ok(listener) => {
                        let _ = axum::serve(listener, app)
                            .with_graceful_shutdown(async move { metrics_signal.wait().await })
                            .await;
                    }
                    Err(e) => tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "metrics_listener_error",
                        error = %e,
                        addr = %metrics_addr,
                        "Controller metrics listener failed to bind"
                    ),
                }
            });
        }

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
                    config_file,
                    authorizer,
                    center_reload_tx,
                    allow_admin_ips,
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
    /// 4. Spawn ConfMgr.start_with_shutdown() (manages lifecycle: leader election, link, relink)
    /// 5. Load CRD schemas (before federation client so router can be created)
    /// 6. Start federation client with admin router (HTTP-over-gRPC proxy)
    /// 7. Start gRPC and Admin services with shutdown support
    pub async fn run(&self) -> Result<()> {
        // Load and merge configuration
        let config = EdgionControllerConfig::load(self.config.clone())?;

        // Initialize global controller config for runtime access (e.g., by ReferenceGrant validator)
        crate::config::init_controller_config(config.clone());

        // Initialize test mode if enabled via CLI --test-mode
        // This affects: metrics test features
        edgion_common::test_mode::init_global_test_mode(self.config.test_mode);

        // 1. Initialize environment (work_dir, logging)
        let _log_guard = self.init_environment(&config).await?;

        // 1b. Install the process-wide Prometheus recorder so any metric emitted
        // during startup (ConfMgr init, federation client connect, etc.) is
        // captured. The recorder is idempotent, so tests and combined modes
        // do not panic on re-install.
        if let Err(e) = edgion_common::observe::metrics_api::install_global_recorder(
            edgion_common::observe::metrics_api::RecorderConfig {
                service: "edgion-controller",
                idle_timeout_secs: 0,
                histogram_buckets: &[],
            },
        ) {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                event = "metrics_recorder_install_failed",
                error = %e,
                "Failed to install Prometheus recorder; /metrics will return 500"
            );
        }

        // Start the resource-abnormal metrics snapshot loop.
        // Runs independently of the Prometheus recorder (renders its own
        // text block appended to /metrics). The loop body is panic-safe;
        // the first snapshot runs immediately after renderers are
        // registered downstream (resource controllers do that during
        // ConfMgr.start).
        let abn_cfg = &config.metrics.resource_abnormal;
        if abn_cfg.enabled {
            crate::observe::status_metrics::set_hard_cap(abn_cfg.hard_cap);
            crate::observe::status_metrics::spawn_snapshot_loop(abn_cfg.tick_secs);
        } else {
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                "metrics.resource_abnormal.enabled=false; skipping snapshot loop"
            );
        }

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

        // 6b. Construct the shared Authorizer and center-reload watch channel ONCE.
        // The Authorizer is initialized from `[center].rbac` if present, else the built-in
        // default policy (read-all-except-Secret + EdgionConfigData write + RegionRoute failover)
        // via `center_policy_from`.
        // Both are cloned into every create_admin_router call site and into the fed path.
        let initial_center_cfg = config.center.clone().unwrap_or_default();
        if let Some(rbac) = &initial_center_cfg.rbac {
            if let Err(e) = rbac.validate() {
                tracing::warn!(
                    component = "fed_rbac",
                    error = %e,
                    "invalid center.rbac rule; it will not match any request"
                );
            }
        }
        let initial_policy = crate::fed_sync::default_policy::center_policy_from(initial_center_cfg.rbac.as_ref());
        let authorizer = std::sync::Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all()));

        // 6c. Build the initial cli_tokens map from `config.cli_tokens`.
        // Uses the shared lint pass which applies per-entry shape validation,
        // GLOBAL dedup (no silent last-wins on duplicate name/hash), and emits
        // advisory WARNs for unknown/non-canonical kinds, unknown verbs, and
        // dangerous grants (Secret-read / wildcard-read).
        // Map key = token_sha256 VERBATIM (already a hash; do NOT re-hash).
        // Install both the center policy and the cli_tokens map atomically via
        // `apply()` BEFORE the admin server starts serving so there is no window
        // where cli tokens return 401.
        {
            // Lint the center RBAC policy when the operator explicitly supplied one
            // (the built-in default policy is known-good; no need to lint it).
            if let Some(rbac) = initial_center_cfg.rbac.as_ref() {
                crate::authz::lint::lint_rbac_policy(rbac, "startup", "center");
            }

            let cli_map = crate::authz::lint::build_cli_token_map(&config.cli_tokens, "startup");

            // Register the local admin token (Istio-style) on the Authorizer. It is
            // held on the Authorizer (not in the config-derived map) so `apply`
            // merges it at startup AND on every hot-reload. `EDGION_ADMIN_TOKEN`
            // env wins over the `admin_token` config field; blank values are
            // rejected by `build_admin_token_entry`.
            let raw_admin = resolve_admin_token_raw(&config);
            let admin_entry = crate::authz::build_admin_token_entry(raw_admin.as_deref());
            if admin_entry.is_none() && raw_admin.is_some() {
                tracing::warn!(
                    component = "admin_token",
                    context = "startup",
                    "EDGION_ADMIN_TOKEN / admin_token is set but blank; ignoring (no admin token registered)"
                );
            }
            if admin_entry.is_some()
                && config
                    .cli_tokens
                    .iter()
                    .any(|t| t.name == crate::authz::ADMIN_TOKEN_NAME)
            {
                tracing::warn!(
                    component = "cli_tokens",
                    context = "startup",
                    name = crate::authz::ADMIN_TOKEN_NAME,
                    "a configured cli_token uses the reserved name \"admin\"; the local admin token \
                     also uses it — rename the config entry to disambiguate audit logs"
                );
            }
            authorizer.set_admin_token(admin_entry);

            // Install center policy + cli_tokens (admin token merged by `apply`)
            // atomically before admin server serves.
            authorizer.apply(initial_policy, cli_map);
        }

        // The original receiver is intentionally dropped here. The federation supervisor (Task 7)
        // will obtain its own receiver via `center_reload_tx.subscribe()`, which works even after
        // this initial receiver is dropped -- as long as the Sender is alive. The Sender lives for
        // the duration of the process because it is stored in `AdminState` (via `create_admin_router`)
        // and cloned into the fed path above.
        let initial_center_cfg_for_supervisor = initial_center_cfg.clone();
        let (center_reload_tx, _center_reload_rx) = tokio::sync::watch::channel(initial_center_cfg);
        let config_file = self.config.config_file.clone();

        // 7. Start federation client (via supervisor) if center config is present.
        // Must be after conf_mgr.start() so processor_registry is populated.
        // The supervisor owns the fed client lifecycle: it starts/stops the client
        // on reload and ensures the prior task is awaited before a new one spawns,
        // preventing dual registration at Center.
        if let Some(center_config) = config.center.clone() {
            let kinds: Vec<String> = conf_mgr
                .runtime()
                .processor_registry
                .all_kinds()
                .iter()
                .map(|s| s.to_string())
                .collect();

            // Build the fed router:
            //   1. Base admin router (routes + handlers, no auth).
            //   2. route_layer: authz_layer (innermost route_layer -- added first).
            //   3. .layer: inject_center_identity (outermost -- added last, runs first).
            // Axum LIFO rule for route_layer means adding route_layer first makes it
            // innermost; inject_center_identity is a plain .layer so it wraps everything.
            // Final onion (outer -> inner):
            //   inject_center_identity -> authz_layer -> handler
            let base_router = create_admin_router(
                conf_mgr.clone(),
                schema_validator.clone(),
                config_file.clone(),
                authorizer.clone(),
                center_reload_tx.clone(),
            );
            let fed_router = base_router
                .route_layer(axum::middleware::from_fn_with_state(
                    authorizer.clone(),
                    crate::api::authz_middleware::authz_layer,
                ))
                .layer(axum::middleware::from_fn_with_state(
                    authorizer.clone(),
                    crate::api::authz_middleware::inject_center_identity,
                ));
            // IMPORTANT: Do NOT add any .route()/.merge() to fed_router AFTER this
            // point. route_layer only wraps routes present when it is applied; a
            // route added later would bypass authz_layer entirely.

            // Capture everything the per-spawn closure needs. `axum::Router` is
            // `Clone`; the authz route_layer inside it shares the same
            // `Arc<Authorizer>`, so cloning the router per spawn is cheap and
            // correct -- all spawns see the same live policy.
            let kinds_for_spawn = kinds;
            let conf_mgr_for_spawn = conf_mgr.clone();
            let authorizer_for_spawn = authorizer.clone();

            let spawn_client = move |rx: watch::Receiver<bool>| {
                let router = fed_router.clone();
                let cfg = center_config.clone();
                let kinds = kinds_for_spawn.clone();
                let conf_mgr = conf_mgr_for_spawn.clone();
                let authorizer = authorizer_for_spawn.clone();
                tokio::spawn(async move {
                    crate::fed_sync::fed_client::run(cfg, kinds, rx, router, conf_mgr, authorizer).await;
                })
            };

            // Get a reload receiver from the shared sender. The original receiver
            // constructed above was intentionally dropped (`_center_reload_rx`);
            // `subscribe()` creates a fresh receiver from the same sender that is
            // stored in AdminState and used by the reload handler.
            let reload_rx = center_reload_tx.subscribe();
            let initial = initial_center_cfg_for_supervisor;
            let authorizer_for_supervisor = authorizer.clone();

            let supervisor = crate::fed_sync::supervisor::FederationSupervisor::new(spawn_client);
            // Intentionally detached: run_supervisor lives for the process lifetime,
            // driven by the reload watch channel whose sender is stored in AdminState.
            tokio::spawn(crate::fed_sync::supervisor::run_supervisor(
                supervisor,
                initial,
                reload_rx,
                authorizer_for_supervisor,
            ));
        }

        // 8. Parse addresses and start services with shutdown support
        // Note: Services start immediately but return UNAVAILABLE until ConfigServer is ready
        let grpc_addr = parse_listen_addr(Some(&config.grpc_listen()), DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_addr = parse_listen_addr(Some(&config.admin_listen()), "0.0.0.0:12101")?;
        let probe_addr = parse_listen_addr(Some(&config.probe_listen()), "0.0.0.0:12100")?;
        let metrics_addr = parse_listen_addr(Some(&config.metrics_listen()), "0.0.0.0:12190")?;
        let security = config.conf_sync_security.clone();
        security.validate()?;

        Self::start_services(
            conf_mgr,
            schema_validator,
            grpc_addr,
            admin_addr,
            probe_addr,
            metrics_addr,
            security,
            config_file,
            authorizer,
            center_reload_tx,
            config.server.allow_admin_ips.clone(),
            shutdown_handle,
        )
        .await
    }
}

/// Default gRPC listen address for the controller operator endpoint.
const DEFAULT_OPERATOR_GRPC_ADDR: &str = "127.0.0.1:50061";

/// Parse a listen address string into a [`SocketAddr`], falling back to `default`
/// when `addr` is `None`.
fn parse_listen_addr(addr: Option<&String>, default: &str) -> Result<SocketAddr> {
    let candidate = addr.map(String::as_str).unwrap_or(default);
    SocketAddr::from_str(candidate).with_context(|| format!("failed to parse listen address '{}'", candidate))
}
