use crate::api::{create_admin_router, serve_with_shutdown as serve_admin_api_with_shutdown};
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
        auto_ca_ns_sans: Option<(String, Vec<String>)>,
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
        let mut grpc_server = ConfigSyncGrpcServer::new(provider, grpc_shutdown_token.clone());
        // Etcd mode: streams additionally scope to the current leadership term, so
        // a demoted leader actively closes them (spec §6.1). K8s/FS return `None`
        // and keep the process-token-only behavior (S2 zero-impact guard).
        if let Some(slot) = conf_mgr.term_token_slot() {
            grpc_server.set_term_token_slot(slot);
        }

        // conf_sync auto-CA (K8s only): when enabled, the Controller's internal CA
        // ensures the CA Secret, provisions the gateway-client leaf Secret, and mints
        // the controller-server cert in memory (fed straight into tonic — no mount).
        // Idempotent on every startup; a concurrent create is adopted on a 409.
        // Phase 1 relies on this idempotent-adopt for split-brain safety; Phase 2 adds
        // the leader gate + rotation.
        //
        // Build the hot-reloadable server identity (spec §6 T3):
        //  - auto_ca (K8s): from the provisioned in-memory identity
        //  - file mode: from the configured active TLS file paths
        //  - plaintext/skip_tls: none
        use crate::conf_sync::conf_server::cert_watch::{spawn_server_cert_watcher, CertPaths};
        use crate::conf_sync::conf_server::tls_identity::{global_handle, ServerIdentity, ServerIdentityHandle};

        // Controller-side rotation validation (spec §5.2): run BEFORE the auto-CA
        // branch below, which calls `provision()` and persists Secrets minted from
        // the configured TTLs. An invalid or suspect schedule/TTL combo must abort
        // startup before that side effect, not be silently tolerated until the
        // first rotation attempt.
        if let Some(rotation) = &security.rotation {
            if security.auto_ca {
                match crate::services::conf_sync_ca::cert_util::validate_schedule_controller(rotation) {
                    Ok(warnings) if warnings.is_empty() => {}
                    Ok(warnings) => {
                        for warning in &warnings {
                            tracing::warn!(component = "conf_sync_ca", "{warning}");
                        }
                        edgion_common::observe::system_metrics::mark_system_error(
                            edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                            edgion_common::observe::system_metrics::reasons::CERT_ROTATION_CONFIG_SUSPECT,
                        );
                    }
                    Err(e) => anyhow::bail!("invalid conf_sync_security.rotation: {e}"),
                }
            } else {
                tracing::warn!(component = "conf_sync_ca", "rotation config ignored (auto_ca disabled)");
            }
        }

        // Inputs for the per-replica server-identity reconciler, captured in the
        // auto_ca branch below and consumed right after `global_handle().set(..)`.
        let mut auto_ca_identity_sync: Option<(
            kube::Client,
            String,
            Vec<String>,
            edgion_common::conf_sync::config::ConfSyncRotationConfig,
        )> = None;

        let tls_handle: Option<Arc<ServerIdentityHandle>> = if security.auto_ca {
            // ns/sans were already computed once by the caller (`run_after_logging`)
            // so `rotation::init` could run before the ConfMgr leadership spawn
            // (spec §6 T5 init-before-leadership fix); reuse them instead of
            // recomputing.
            let (ns, sans) = auto_ca_ns_sans
                .clone()
                .expect("auto_ca_ns_sans is Some whenever security.auto_ca is set (computed by the caller)");
            let client = kube::Client::try_default().await?;
            let rotation = security.rotation.clone().unwrap_or_default();
            let id = crate::services::conf_sync_ca::provision(client.clone(), &ns, sans.clone(), &rotation).await?;
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                "conf_sync mTLS enabled for gRPC server (auto-CA, in-memory server cert)"
            );
            let h = Arc::new(ServerIdentityHandle::new(ServerIdentity::from_pems(
                &id.cert_pem,
                id.key_pem.as_str(),
                &id.ca_cert_pem,
            )?));
            auto_ca_identity_sync = Some((client, ns, sans, rotation));
            Some(h)
        } else if let Some(tls) = security.resolve_active_tls().filter(|t| !t.skip_tls) {
            let cert = tokio::fs::read_to_string(tls.cert_path()).await?;
            let key = tokio::fs::read_to_string(tls.key_path()).await?;
            let ca = tokio::fs::read_to_string(tls.ca_path()).await?;
            tracing::info!(
                component = COMPONENT_EDGION_CONTROLLER,
                "conf_sync mTLS enabled for gRPC server"
            );
            let h = Arc::new(ServerIdentityHandle::new(ServerIdentity::from_pems(&cert, &key, &ca)?));
            // Hot-reload (spec §6 T3): watch the active TLS set's file paths for
            // in-place replacement (e.g. cert-manager renewal, Secret update).
            // Flipping `active` to a different set remains restart-bound.
            spawn_server_cert_watcher(
                CertPaths {
                    cert: tls.cert_path(),
                    key: tls.key_path(),
                    ca: tls.ca_path(),
                },
                h.clone(),
                grpc_shutdown_token.clone(),
            );
            Some(h)
        } else {
            None
        };
        if let Some(h) = &tls_handle {
            let _ = global_handle().set(h.clone());
        }

        // Per-replica server-identity reconciler (auto_ca only). Started
        // UNCONDITIONALLY — leadership is deliberately NOT involved. Under
        // `ha_mode: all-serve` every replica serves the conf_sync API (which
        // streams resolved Secrets), so every replica must keep its own served
        // leaf AND its client-CA verifier in sync with the current root, or an
        // emergency CA re-key never revokes anything on the non-leaders. Mirrors
        // istiod's `watchRootCertAndGenKeyCert`. Read-only on Secrets; the only
        // mutation is this process's own in-memory identity handle.
        if let Some((client, ns, sans, rotation)) = auto_ca_identity_sync {
            crate::services::conf_sync_ca::start_identity_sync(client, ns, sans, rotation, grpc_shutdown_token.clone());
        }

        let grpc_listener = tokio::net::TcpListener::bind(grpc_addr).await?;

        // conf_sync cert expiry safety net (observability only): watches the LIVE
        // in-service server identity through the hot-swap handle, so the gauge and
        // thresholds stay accurate across rotations and file reloads. This is the
        // outside cross-check on the rotation pipeline; plaintext/skip_tls = nothing
        // to watch.
        if tls_handle.is_some() {
            use edgion_common::conf_sync::cert_expiry_watch as cert_watch;
            cert_watch::spawn_expiry_watch(
                cert_watch::sides::SERVER,
                cert_watch::CertSource::PemFn(crate::conf_sync::conf_server::tls_identity::live_server_cert_pem),
            );
        }

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
                grpc_server.serve_with_shutdown(grpc_listener, &security, tls_handle.clone(), grpc_shutdown),
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

        // Compile and install the plugin policy (fail-fast: aborts startup on
        // a malformed `validation.plugin_policy` config). Must run right after
        // `init_controller_config` and before any plugin resource is preparsed.
        crate::config::plugin_policy::init_compiled_plugin_policy(config.validation.plugin_policy.as_ref())?;

        // Initialize test mode if enabled via CLI --test-mode
        // This affects: metrics test features
        edgion_common::test_mode::init_global_test_mode(self.config.test_mode);

        // 1. Initialize environment (work_dir, logging)
        let _log_guard = self.init_environment(&config).await?;

        // Logging is live from here on. Run the serving body and, on failure,
        // record the error through structured logging while `_log_guard` is
        // still alive: the non-blocking file worker stops accepting writes once
        // the guard drops, so logging the error from `main` (after `run`
        // returns) would be lost. Errors before this point (config load) fall
        // back to stderr via `report_startup_failure` in `main`.
        let result = self.run_after_logging(config).await;
        if let Err(err) = &result {
            tracing::error!(
                component = COMPONENT_EDGION_CONTROLLER,
                error = %format_args!("{err:#}"),
                "Controller exited with error"
            );
        }
        result
    }

    /// Serving body: everything after logging has been initialized. Split out of
    /// `run` so the top-level error can be logged while the log `WorkerGuard` is
    /// still alive (see `run`).
    async fn run_after_logging(&self, config: EdgionControllerConfig) -> Result<()> {
        // 1b. Install the process-wide Prometheus recorder so any metric emitted
        // during startup (ConfMgr init, federation client connect, etc.) is
        // captured. The recorder is idempotent, so tests and combined modes
        // do not panic on re-install.
        if let Err(e) = edgion_common::observe::metrics_api::install_global_recorder(
            edgion_common::observe::metrics_api::RecorderConfig {
                service: "edgion-controller",
                idle_timeout_secs: 0,
                histogram_buckets: &[],
                cardinality_cap: config.metrics.cardinality_cap,
                cardinality_mode: config.metrics.cardinality_mode,
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
            crate::observe::status_metrics::set_status_conditions_threshold(abn_cfg.status_conditions_threshold);
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

        // 4b. conf_sync auto-CA rotation (spec §6 T5): register the rotation
        // parameters via `rotation::init` BEFORE the ConfMgr leadership spawn
        // immediately below. That spawned task can win leadership and reach the
        // caches-ready callback (center.rs), which calls `start_conf_sync_rotation`,
        // well before this function's control flow reaches the auto-CA
        // provisioning code inside `start_services` further down -- if `init()`
        // hasn't run by then, `start_conf_sync_rotation` no-ops silently for the
        // entire leadership term (rotation never starts). Only synchronously
        // available inputs are needed here (POD_NAMESPACE, resolved SANs, rotation
        // config), so there is no reason to wait for the async `provision()`
        // round-trip. ns/sans are computed once here and reused by
        // `start_services` for `provision()` (see `auto_ca_ns_sans`).
        let auto_ca_ns_sans: Option<(String, Vec<String>)> = if config.conf_sync_security.auto_ca {
            let ns = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string());
            let sans = crate::services::conf_sync_ca::server_sans_for(&ns);
            let rotation = config.conf_sync_security.rotation.clone().unwrap_or_default();
            crate::services::conf_sync_ca::rotation::init(ns.clone(), sans.clone(), rotation);
            Some((ns, sans))
        } else {
            None
        };

        // 5. Spawn ConfMgr.start_with_shutdown() in background, wrapped in an
        // inner task so a lifecycle PANIC is observed as a JoinError — a panic
        // unwinds past any linear code after the .await and tokio would
        // otherwise swallow it at the task boundary, reproducing exactly the
        // "lifecycle-dead-but-process-alive" wedge the deadman exists for.
        let start_conf_mgr = conf_mgr.clone();
        let conf_mgr_shutdown = shutdown_handle.clone();
        let deadman_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            let lifecycle = tokio::spawn(async move { start_conf_mgr.start_with_shutdown(conf_mgr_shutdown).await });
            let outcome = lifecycle.await;
            match &outcome {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "conf_mgr_start_error",
                        error = %e,
                        "ConfMgr start failed"
                    );
                }
                Err(join_err) => {
                    tracing::error!(
                        component = COMPONENT_EDGION_CONTROLLER,
                        event = "conf_mgr_lifecycle_panic",
                        error = %join_err,
                        "ConfMgr lifecycle task panicked"
                    );
                }
            }
            // Deadman: any end without an orderly shutdown is abnormal. Mark
            // FIRST (sticky /health 503 backstop), then trigger shutdown so the
            // lease is released within seconds instead of waiting ~30 s+ for
            // kubelet's liveness failure threshold.
            if !deadman_shutdown.is_shutdown() {
                crate::api::mark_lifecycle_terminated();
                tracing::error!(
                    component = COMPONENT_EDGION_CONTROLLER,
                    event = "conf_mgr_lifecycle_terminated",
                    "Control-plane lifecycle ended without a shutdown request; \
                     marking deadman and triggering graceful shutdown"
                );
                deadman_shutdown.shutdown();
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
            let conf_mgr_for_spawn = conf_mgr.clone();
            let authorizer_for_spawn = authorizer.clone();

            let spawn_client = move |rx: watch::Receiver<bool>| {
                let router = fed_router.clone();
                let cfg = center_config.clone();
                let conf_mgr = conf_mgr_for_spawn.clone();
                let authorizer = authorizer_for_spawn.clone();
                tokio::spawn(async move {
                    crate::fed_sync::fed_client::run(cfg, rx, router, conf_mgr, authorizer).await;
                })
            };

            // Get a reload receiver from the shared sender. The original receiver
            // constructed above was intentionally dropped (`_center_reload_rx`);
            // `subscribe()` creates a fresh receiver from the same sender that is
            // stored in AdminState and used by the reload handler.
            let reload_rx = center_reload_tx.subscribe();
            let initial = initial_center_cfg_for_supervisor;
            let authorizer_for_supervisor = authorizer.clone();

            // Leadership gate: the federation session at Center is keyed by
            // `controller_id` (`cluster/name`), identical on every replica, so
            // only the leader may hold it. `None` (FileSystem: no election)
            // leaves the client gated on `center.enabled` alone.
            let leader_signal = conf_mgr.leader_signal();

            let supervisor = crate::fed_sync::supervisor::FederationSupervisor::new(spawn_client);
            // Intentionally detached: run_supervisor lives for the process lifetime,
            // driven by the reload watch channel whose sender is stored in AdminState.
            tokio::spawn(crate::fed_sync::supervisor::run_supervisor(
                supervisor,
                initial,
                reload_rx,
                leader_signal,
                authorizer_for_supervisor,
            ));
        }

        // 8. Parse addresses and start services with shutdown support
        // Note: Services start immediately but return UNAVAILABLE until ConfigServer is ready
        let grpc_addr = parse_listen_addr(Some(&config.grpc_listen()), DEFAULT_OPERATOR_GRPC_ADDR)?;
        let admin_addr = parse_listen_addr(Some(&config.admin_listen()), "127.0.0.1:12101")?;
        let probe_addr = parse_listen_addr(Some(&config.probe_listen()), "0.0.0.0:12100")?;
        let metrics_addr = parse_listen_addr(Some(&config.metrics_listen()), "0.0.0.0:12190")?;
        let security = config.conf_sync_security.clone();
        security.validate()?;
        // Fail-close (2026-07-14): an omitted `conf_sync_security` aborts startup here,
        // BEFORE any listener is bound. The gRPC server has the same guard
        // (`accept_skip_tls_or_refuse`), but it runs inside a `tokio::join!` with the
        // Admin API, so an error there would not promptly stop the process — this is the
        // check that turns the misconfiguration into a non-zero exit (`run()` -> `main`).
        security.ensure_transport_configured()?;

        Self::start_services(
            conf_mgr,
            schema_validator,
            grpc_addr,
            admin_addr,
            probe_addr,
            metrics_addr,
            security,
            auto_ca_ns_sans,
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
