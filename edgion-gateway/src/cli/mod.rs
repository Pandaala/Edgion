use crate::ai::{get_adaptive_telemetry_store, run_adaptive_telemetry_ticker};
use crate::api;
use crate::cli::pingora::{create_and_configure_server, run_server};
use crate::conf_sync::{ConfigClient, ConfigSyncClient};
use crate::lb::leastconn::BackendCleaner;
use crate::observe::access_log::init_access_logger;
use crate::observe::logs::{init_logging, init_ssl_logger, init_tcp_logger, init_tls_logger, init_udp_logger};
use crate::observe::metrics;
use crate::plugins::http::request_mirror::init_global_budget;
use crate::routes::udp::get_udp_listener_registry;
use crate::runtime::server::proxy_protocol_policy::init_global_proxy_protocol_policy_runtime;
use anyhow::{anyhow, bail, Result};
use clap::Parser;
use edgion_common::observe::metrics_facade::fixed_gauge;
use edgion_resources::{init_work_dir, work_dir};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod config;
mod pingora;

use crate::conf_sync::init_global_config_client;
use config::{EdgionGatewayConfig, MAX_UNKNOWN_CONFIG_FIELDS};

fn report_unknown_config_fields<'a>(
    config_file: &str,
    field_paths: impl IntoIterator<Item = &'a str>,
    truncated: bool,
) {
    for field_path in field_paths {
        tracing::error!(
            component = "config",
            config_file,
            field_path,
            action = "ignored",
            "Gateway process configuration contains an unknown field"
        );
    }
    if truncated {
        tracing::error!(
            component = "config",
            config_file,
            reported_fields = MAX_UNKNOWN_CONFIG_FIELDS,
            action = "ignored",
            "Additional unknown Gateway process configuration fields were omitted from diagnostics"
        );
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "edgion-gateway",
    version,
    about = "Edgion Gateway standalone executable",
    long_about = None
)]
pub struct EdgionGatewayCli {
    /// Print compile-time Gateway capabilities in a machine-readable format and exit.
    #[arg(long)]
    pub print_build_capabilities: bool,

    #[command(flatten)]
    pub config: EdgionGatewayConfig,
}

/// Conf-sync may publish exporters before the remaining startup steps succeed.
/// Keep their IO runtime alive until the error path has joined every SDK worker.
struct OtlpStartupGuard<'a> {
    runtime: &'a tokio::runtime::Runtime,
    armed: bool,
}

impl Drop for OtlpStartupGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.runtime
                .block_on(crate::link_sys::runtime::store::shutdown_otlp_clients());
        }
    }
}

impl EdgionGatewayCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// UDP and auxiliary HTTP services share the existing five-second grace
    /// window. Forced task cancellation is acknowledged before runtime teardown.
    async fn drain_udp_listeners() -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let outcome = get_udp_listener_registry().shutdown(deadline).await;
        tokio::time::sleep_until(deadline).await;
        if outcome.timed_out || outcome.failed_listeners != 0 {
            tracing::error!(
                component = "gateway",
                timed_out = outcome.timed_out,
                failed_listeners = outcome.failed_listeners,
                "UDP shutdown required forced cancellation or encountered a failed listener"
            );
            bail!("UDP shutdown did not complete gracefully; inspect gateway shutdown diagnostics");
        }
        Ok(())
    }

    /// Create ConfigSyncClient
    async fn create_config_sync_client(config: &EdgionGatewayConfig) -> Result<ConfigSyncClient> {
        let server_addrs = config.server_addrs();
        if server_addrs.is_empty() {
            bail!("server_addrs is required: provide --server-addr (repeatable) or set gateway.server_addrs in the config file");
        }

        let security = &config.conf_sync_security;
        security
            .validate()
            .map_err(|e| anyhow!("Invalid conf_sync_security config: {}", e))?;

        // auto_ca=true with no TLS block at all is a permanent misconfiguration: auto_ca
        // means the Controller's internal CA provisions the certs, so the cert paths must
        // be configured. Fail fast (no retry) — retrying a config error only stalls.
        if security.auto_ca && security.is_plaintext_omitted() {
            bail!(
                "conf_sync_security.auto_ca=true but no cert paths are configured. auto_ca \
                 requires mTLS; set conf_sync_security.certs to the Controller-provisioned \
                 cert files (mounted from the edgion-conf-sync-certs Secret)."
            );
        }

        // Fail-close (2026-07-14): an omitted `conf_sync_security` refuses to start. The
        // conf_sync client has the same guard (`ConfigSyncClient::new`); checking here too
        // fails fast, before the auto_ca retry loop.
        security.ensure_transport_configured()?;

        // Certificate expiry is sampled from the installed Channel at scrape
        // time. Until installation succeeds, the retry diagnostics below expose
        // missing auto-CA material without claiming a disk certificate is active.

        // Under auto_ca the gateway-client Secret is provisioned by the Controller and may
        // not exist yet at cold start (the mounted volume is `optional`). Rather than
        // crash-loop on a transient "cert file not found", retry within the startup-probe
        // budget (~40s < the 60s startupProbe window) so a Secret that lands shortly is
        // picked up in-process. Non-auto_ca keeps the original single-shot behavior:
        // manual certs are expected to be present at startup.
        if security.auto_ca {
            const MAX_ATTEMPTS: u32 = 20;
            const BACKOFF: Duration = Duration::from_secs(2);
            let mut attempt: u32 = 0;
            loop {
                attempt += 1;
                match ConfigSyncClient::new(
                    server_addrs.clone(),
                    "edgion-gateway".to_string(),
                    Duration::from_secs(10),
                    security,
                )
                .await
                {
                    Ok(client) => return Ok(client),
                    Err(e) if attempt < MAX_ATTEMPTS => {
                        tracing::warn!(
                            component = "grpc_client",
                            attempt,
                            max_attempts = MAX_ATTEMPTS,
                            error = %e,
                            "conf_sync client init failed (auto_ca: waiting for the provisioned \
                             cert Secret to appear); retrying"
                        );
                        tokio::time::sleep(BACKOFF).await;
                    }
                    Err(e) => {
                        return Err(anyhow!(
                            "Failed to create ConfigSyncClient after {attempt} attempts: {e}"
                        ));
                    }
                }
            }
        }

        let sync_client = ConfigSyncClient::new(
            server_addrs,
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
            security,
        )
        .await
        .map_err(|e| anyhow!("Failed to create ConfigSyncClient: {}", e))?;

        Ok(sync_client)
    }

    /// Start auxiliary services
    async fn start_auxiliary_services(
        config_client: Arc<ConfigClient>,
        sync_client: Arc<ConfigSyncClient>,
        config: &EdgionGatewayConfig,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        // One process-wide adaptive publisher. Request paths only update
        // captured atomics; no per-request task is created.
        tokio::spawn(run_adaptive_telemetry_ticker(
            Arc::clone(get_adaptive_telemetry_store()),
            shutdown_rx.clone(),
        ));

        // Start backend cleaner; stop it when the shutdown signal fires.
        let mut cleaner = BackendCleaner::new();
        cleaner.start();
        tracing::info!("Backend cleaner task started for LeastConnection LB");
        let mut cleaner_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            while !*cleaner_shutdown.borrow() {
                if cleaner_shutdown.changed().await.is_err() {
                    break;
                }
            }
            cleaner.stop().await;
        });

        // Initialize health check stores and manager (tasks start on resource updates).
        let _ = crate::backends::health::check::get_hc_config_store();
        let _ = crate::backends::health::check::get_health_check_manager();
        tracing::info!("Health check manager initialized");

        // Spawn Admin API server with graceful shutdown
        let admin_listen = config.admin_listen_or_default();
        let config_client_for_admin = config_client.clone();
        let mut admin_shutdown = shutdown_rx.clone();
        let admin_listen_clone = admin_listen.clone();
        // Resolve the gateway admin token (env > config). When unconfigured the
        // admin API fails closed (503); the auth state is built inside
        // `serve_with_shutdown` from this resolved value.
        let admin_token = crate::cli::config::resolve_admin_token(
            std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok(),
            config.gateway.admin_token.as_deref(),
        );
        if admin_token.is_none() {
            tracing::warn!(
                component = "admin_api_gateway",
                event = "no_admin_token",
                "No gateway admin token configured. Admin API will return 503 until \
                 EDGION_GATEWAY_ADMIN_TOKEN or gateway.admin_token is set."
            );
        }
        tokio::spawn(async move {
            let shutdown_future = async move {
                while !*admin_shutdown.borrow() {
                    if admin_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            if let Err(e) = api::serve_with_shutdown(
                config_client_for_admin,
                &admin_listen_clone,
                admin_token,
                shutdown_future,
            )
            .await
            {
                tracing::error!(
                    component = "admin_api_gateway",
                    event = "server_error",
                    error = %e,
                    "Gateway Admin API server failed"
                );
            }
        });

        tracing::info!(
            component = "admin_api_gateway",
            event = "server_spawned",
            listen = %admin_listen,
            "Gateway Admin API server spawned"
        );

        // Spawn probe API server (liveness/readiness only) with graceful shutdown.
        let probe_listen = config.probe_listen_or_default();
        let config_client_for_probe = config_client.clone();
        let mut probe_shutdown = shutdown_rx.clone();
        let probe_listen_clone = probe_listen.clone();
        tokio::spawn(async move {
            let shutdown_future = async move {
                while !*probe_shutdown.borrow() {
                    if probe_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            if let Err(e) =
                api::serve_probe_with_shutdown(config_client_for_probe, &probe_listen_clone, shutdown_future).await
            {
                tracing::error!(
                    component = "probe_api_gateway",
                    event = "server_error",
                    error = %e,
                    "Gateway probe API server failed"
                );
            }
        });

        tracing::info!(
            component = "probe_api_gateway",
            event = "server_spawned",
            listen = %probe_listen,
            "Gateway probe API server spawned"
        );

        // Spawn Metrics API server with graceful shutdown
        let metrics_cfg = config.metrics.clone();
        let mut metrics_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            let shutdown_future = async move {
                while !*metrics_shutdown.borrow() {
                    if metrics_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            if let Err(e) = metrics::serve_with_shutdown(&metrics_cfg, sync_client, shutdown_future).await {
                tracing::error!(
                    component = "metrics_api",
                    event = "server_error",
                    error = %e,
                    "Metrics API server failed"
                );
            }
        });

        tracing::info!(
            component = "metrics_api",
            event = "server_spawned",
            listen = %config.metrics.listen,
            "Metrics API server spawned"
        );
    }

    /// Wait for all caches to be ready
    async fn wait_for_ready(config_client: Arc<ConfigClient>) -> Result<()> {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match config_client.is_ready() {
                Ok(()) => {
                    tracing::info!("All caches are ready");
                    return Ok(());
                }
                Err(msg) => {
                    tracing::info!("{}", msg);
                }
            }
        }
    }

    pub fn run(&self) -> Result<()> {
        // Retry budgets and outbound rate limits must never use the lock-based fallback.
        if !portable_atomic::AtomicU128::is_lock_free() {
            bail!(
                "Gateway requires lock-free 128-bit atomics for retry budgets and outbound rate limits; \
                 use a supported CPU and expose CMPXCHG16B to amd64 virtual machines"
            );
        }

        // Create a Tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new()?;

        // 1. Load configuration (synchronous)
        let loaded_config = EdgionGatewayConfig::load(self.config.clone())?;
        let config = loaded_config.config;

        // 2. Determine work_dir (priority: CLI > ENV > Config > Default)
        let work_dir_path = self
            .config
            .work_dir
            .clone()
            .or_else(|| std::env::var("EDGION_WORK_DIR").ok().map(PathBuf::from))
            .or_else(|| config.work_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        // 3. Initialize and validate work_dir
        init_work_dir(work_dir_path).map_err(|e| anyhow!("Failed to initialize work directory: {}", e))?;
        let wd = work_dir();
        wd.validate()
            .map_err(|e| anyhow!("Work directory validation failed: {}", e))?;

        // 4. Initialize logging (at outermost level, keep WorkerGuard alive)
        let log_config = config.to_log_config();
        let _log_guard = runtime.block_on(init_logging(log_config))?;
        report_unknown_config_fields(
            self.config.config_file.as_deref().unwrap_or("<none>"),
            loaded_config.unknown_fields.paths(),
            loaded_config.unknown_fields.is_truncated(),
        );
        tracing::info!(
            component = "startup",
            work_dir = %wd.base().display(),
            "Work directory initialized"
        );

        // Logging is live from here on. Run the serving body and, on failure,
        // record the error through structured logging while `_log_guard` is
        // still alive: the non-blocking file worker stops accepting writes once
        // the guard drops, so logging the error from `main` (after `run`
        // returns) would be lost. Errors before this point (config / work_dir
        // load) fall back to stderr via `report_startup_failure` in `main`.
        let result = self.run_after_logging(runtime, config);
        if let Err(err) = &result {
            tracing::error!(
                component = "gateway",
                error = %format_args!("{err:#}"),
                "Gateway exited with error"
            );
        }
        result
    }

    /// Serving body: everything after logging has been initialized. Split out of
    /// `run` so the top-level error can be logged while the log `WorkerGuard` is
    /// still alive (see `run`).
    fn run_after_logging(&self, runtime: tokio::runtime::Runtime, config: EdgionGatewayConfig) -> Result<()> {
        // Production guard: the `--integration-testing-mode` flag activates
        // production-unsafe subsystems (test-only admin routes, in-memory
        // access-log storage, per-request metric labels, loopback upstreams).
        // It is refused unless the operator ALSO sets the explicit env override,
        // so a single accidental CLI flag in a production rollout fails closed
        // (loudly) instead of silently widening the attack/cardinality surface.
        let integration_testing = self.config.integration_testing_mode;
        if integration_testing
            && !config::integration_testing_mode_env_enabled(
                std::env::var(config::ALLOW_INTEGRATION_TESTING_MODE_ENV)
                    .ok()
                    .as_deref(),
            )
        {
            return Err(anyhow!(
                "--integration-testing-mode is refused: this mode is production-unsafe and \
                 requires the {env} environment variable to be explicitly set to a truthy value \
                 (e.g. {env}=1). Set it only in local/integration environments, never in production.",
                env = config::ALLOW_INTEGRATION_TESTING_MODE_ENV,
            ));
        }

        // 4b. Install the process-wide Prometheus recorder eagerly, before any
        // startup-time metric is emitted (e.g. the conf_sync client's skip_tls
        // system-error gauge during config-sync client construction, or the
        // integration-testing-mode gauge below).
        // Reuses the gateway's MetricsConfig so custom histogram buckets are
        // preserved; install_global_recorder is OnceLock-guarded, so the metrics
        // server's later lazy install no-ops. Mirrors the controller's "1b" eager install.
        if let Err(e) = metrics::api::init_metrics_exporter(&config.metrics) {
            tracing::error!(
                component = "metrics_api",
                event = "metrics_recorder_install_failed",
                error = %e,
                "Failed to install Prometheus recorder eagerly; /metrics may be incomplete"
            );
        }

        // 4c. FIPS: detect the host state, publish the gauges, enforce the
        // startup requirement, and resolve the approved-profile policy.
        // Placed after the Prometheus recorder is installed (4b) so the gauges
        // are captured by the real exporter, and before any downstream subsystem
        // is initialized so a required-but-missing FIPS module aborts early.
        //
        // The host FIPS state is informational: BoringSSL is linked statically,
        // so the node's mode has no effect on our crypto. It drives the
        // posture guardrail and the `fips_policy: auto` default only.
        use crate::tls::fips_host::{self, HostFipsState};
        use crate::tls::fips_profile;

        let fips_available = fips_profile::fips_module_available();
        // The detection-path override is production-unsafe (it is the sole input
        // to detection, so it can suppress the posture signal entirely), and
        // is honored only under the same integration-testing gate as the other
        // production-unsafe modes.
        let host_state = fips_host::detect_host_fips(integration_testing);

        fixed_gauge!(crate::observe::metrics::registry::names::FIPS_MODULE_ENABLED).set(if fips_available {
            1.0
        } else {
            0.0
        });
        fixed_gauge!(crate::observe::metrics::registry::names::FIPS_HOST_ENABLED)
            .set(if host_state == HostFipsState::Enabled { 1.0 } else { 0.0 });
        fixed_gauge!(crate::observe::metrics::registry::names::FIPS_HOST_DETECT_ERROR)
            .set(if host_state == HostFipsState::Unknown { 1.0 } else { 0.0 });

        let require_fips = config.gateway.require_fips
            || config::require_fips_env_enabled(std::env::var(config::REQUIRE_FIPS_ENV).ok().as_deref());

        config::check_fips_requirement(require_fips, fips_available).map_err(anyhow::Error::msg)?;

        // Posture guardrail: a FIPS node running a binary without BoringSSL
        // FIPS mode. WARN rather than exit, because `fips_enabled=1` is a
        // node property the gateway's operator may not control; fail-fast is
        // opt-in via `require_fips`.
        if fips_host::host_fips_mismatch(host_state, fips_available) {
            tracing::warn!(
                target: "startup",
                "host kernel reports FIPS mode but this binary was not built with \
                 BoringSSL FIPS mode. Deploy a FIPS-mode build, or set require_fips \
                 to fail closed. This signal does not establish CMVP compliance. \
                 Alert on: fips_host_enabled == 1 and fips_module_enabled == 0"
            );
        }

        // Resolve the policy mode: env > CLI/file > Auto. (`merge()` has already
        // collapsed CLI over file into `config.gateway.fips_policy`.)
        let configured = config.gateway.fips_policy;
        let env_mode = match config::fips_policy_env_mode(std::env::var(config::FIPS_POLICY_ENV).ok().as_deref()) {
            config::EnvPolicy::Set(mode) => Some(mode),
            config::EnvPolicy::Unset => None,
            config::EnvPolicy::Invalid(raw) => {
                tracing::warn!(
                    target: "startup",
                    env = config::FIPS_POLICY_ENV,
                    value = %raw,
                    "ignoring an unrecognized FIPS policy value; expected auto, on, or off"
                );
                None
            }
        };

        let mode = env_mode.or(configured).unwrap_or_default();
        fips_profile::set_fips_policy_forced(fips_profile::resolve_policy(mode, host_state));

        let policy_active = fips_profile::fips_policy_active();
        fixed_gauge!(crate::observe::metrics::registry::names::FIPS_POLICY_ACTIVE,
            "module_enabled" => if fips_available { "true" } else { "false" },
        )
        .set(if policy_active { 1.0 } else { 0.0 });

        tracing::info!(
            target: "startup",
            fips_module_enabled = fips_available,
            fips_host_state = ?host_state,
            require_fips,
            fips_policy_mode = mode.as_str(),
            fips_policy_active = policy_active,
            "FIPS status resolved"
        );

        // 4d. Outbound HTTP transport limits must be installed before any
        // canonical reqwest client can be constructed.
        edgion_common::link_sys_core::http_client::configure_outbound_http2_max_header_list_size(
            config.gateway.outbound_http2_max_header_list_size,
        )
        .map_err(|error| anyhow!("Invalid outbound HTTP configuration: {error}"))?;

        // 4e. Outbound egress: install the gateway-local defaults (block_private,
        // DNS cache TTL) that `EgressResolver` falls back to when a resource does
        // not set `target.blockPrivate` / an explicit TTL. Must run before any
        // `ManagedOutboundClient` can be built.
        let outbound_egress_block_private = config.gateway.outbound_egress_block_private
            || config::outbound_block_private_env_enabled(
                std::env::var(config::OUTBOUND_BLOCK_PRIVATE_ENV).ok().as_deref(),
            );
        let outbound_egress_dns_cache_ttl_ms = config::resolve_outbound_egress_dns_cache_ttl_ms(
            config.gateway.outbound_egress_dns_cache_ttl_ms,
            std::env::var(config::OUTBOUND_DNS_CACHE_TTL_MS_ENV).ok().as_deref(),
        );
        crate::plugins::http::common::outbound::egress::configure_egress_defaults(
            outbound_egress_block_private,
            Duration::from_millis(outbound_egress_dns_cache_ttl_ms),
        );

        crate::plugins::http::wasm::admission::initialize(config.gateway.wasm_limits).map_err(anyhow::Error::msg)?;

        // 4f. Wasm OCI artifacts are retained only in bounded process memory.
        {
            use crate::plugins::http::wasm::oci::cache::{configure_oci_memory_cache, OciMemoryCacheConfig};
            configure_oci_memory_cache(OciMemoryCacheConfig {
                ttl: Duration::from_secs(config.gateway.wasm_oci_cache_ttl_secs),
                purge_interval: Duration::from_secs(config.gateway.wasm_oci_cache_purge_interval_secs),
                max_bytes: config.gateway.wasm_oci_cache_max_bytes,
                max_entries: config.gateway.wasm_oci_cache_max_entries,
            })
            .map_err(|error| anyhow!("Failed to initialize Wasm OCI memory cache: {error}"))?;
            crate::plugins::http::wasm::oci::configure_oci_pull(
                Duration::from_secs(config.gateway.wasm_oci_pull_timeout_secs),
                config.gateway.wasm_oci_pull_attempts,
            );
        }

        crate::ai::config::initialize(&config.ai)
            .map_err(|error| anyhow!("Invalid AI conversion configuration: {error}"))?;

        // 5. Initialize RateLimitLocal / cache / access-log sensitive global configurations
        let mirror_limits = config.request_mirror.resolve().map_err(|error| anyhow!(error))?;
        init_global_budget(&mirror_limits)
            .map_err(|error| anyhow!("Failed to initialize request_mirror budget: {error}"))?;
        edgion_resources::rate_limit_local_config::init_rate_limit_local_global_config(&config.rate_limit_local);
        config::init_lru_cache_global_config(&config.lru_cache)
            .map_err(|error| anyhow::anyhow!("Invalid lru_cache configuration: {error}"))?;
        crate::request_body::budget::initialize(&config.gateway.request_body_budget).map_err(anyhow::Error::msg)?;

        crate::response_cache::initialize(&config.response_cache).map_err(anyhow::Error::msg)?;
        config::init_tiny_ufo_ttl_cache_global_config(&config.tiny_ufo_ttl_cache)
            .map_err(|error| anyhow::anyhow!("Invalid tiny_ufo_ttl_cache configuration: {error}"))?;
        edgion_resources::sensitive::init_sensitive(config.access_log_sensitive.clone());

        // 6. Initialize AllEndpointStatus global configuration
        crate::plugins::http::all_endpoint_status::plugin::init_all_endpoint_status_global_config(
            &config.all_endpoint_status,
        );

        // 6b. Initialize the GeoIP provider and, when configured, spawn the
        // background updater. Never blocks on the network and never fails
        // startup — see `crate::geoip::init` for the fail-open rationale.
        //
        // `crate::geoip::init` runs synchronously here, on the plain OS thread
        // between two `runtime.block_on` calls — NOT inside the Tokio runtime
        // context. When `update_url` is set, `GeoIpUpdater::spawn` calls
        // `tokio::spawn`, which unconditionally panics ("there is no reactor
        // running") without an entered runtime. `runtime.enter()` only sets
        // the thread-local pointing at this runtime for the duration the guard
        // is held; it does not block, and the runtime's own worker threads
        // (already running once `Runtime::new()` returns) keep driving the
        // spawned updater task for the rest of the process's life after the
        // guard drops at the end of this scope.
        let _geoip_runtime_guard = runtime.enter();
        crate::geoip::init(&config.gateway.geoip);
        drop(_geoip_runtime_guard);

        tracing::info!(
            component = "startup",
            allocator = crate::allocator_name(),
            version = env!("CARGO_PKG_VERSION"),
            "Starting Edgion Gateway"
        );

        // 7. Initialize the process-wide listener-policy guard before any
        // ConfigClient handler can receive a full or incremental snapshot.
        init_global_proxy_protocol_policy_runtime();

        // 8. Create ConfigSyncClient
        let mut sync_client = runtime.block_on(Self::create_config_sync_client(&config))?;
        let config_client = sync_client.get_config_client();

        // 8. Set global config client
        init_global_config_client(config_client.clone())
            .map_err(|e| anyhow!("Failed to initialize global config client: {}", e))?;

        // 9. Get server info from Controller (includes supported kinds).
        // Bootstrap rotation (spec §7.3): with multiple controllers (etcd HA) the
        // first address may be a follower answering UNAVAILABLE — the previous
        // single-shot call would crash-loop the gateway. Rotate through the list
        // within a bounded budget (mirrors the auto_ca retry shape); do NOT loop
        // forever inside block_on — the supervisor restart supplies "never give
        // up". A single-address deployment retries the same address, which is a
        // strict improvement over the previous instant hard-fail.
        let server_info = runtime
            .block_on(sync_client.bootstrap_server_info())
            .map_err(|e| anyhow!("Failed to get server info from Controller: {}", e))?;

        // 10. Initialize integration testing mode from CLI flag (already gated on
        // the env override at the top of this function).
        // This flag activates: Access Log Store + Metrics Test Data collection
        edgion_common::test_mode::init_integration_testing_mode(integration_testing);
        if integration_testing {
            // Register the active production-unsafe mode so monitoring catches
            // accidental enablement even when startup logs go unread.
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::GATEWAY_RUNTIME,
                edgion_common::observe::system_metrics::reasons::INTEGRATION_TESTING_MODE,
            );
        }

        // 11. Initialize global test mode using the same flag
        edgion_common::test_mode::init_global_test_mode(integration_testing);

        tracing::info!(
            component = "startup",
            test_mode = integration_testing,
            integration_testing_mode = integration_testing,
            server_id = %server_info.server_id,
            "Gateway startup configuration initialized"
        );

        // Initialize metrics before resource watches: route compilation reads
        // these OnceLocks and would otherwise install fallback defaults first.
        config::init_request_duration_config(config.metrics.request_duration.clone());
        config::init_backend_requests_config(config.metrics.backend_requests.clone());
        config::init_genai_tokens_config(config.metrics.genai_tokens.clone());

        // Route and plugin compilation can read this OnceLock as soon as the
        // watches start. Install the configured value before a lazy reader can
        // publish the fallback default.
        edgion_resources::path_normalization::init_path_normalization_config(config.path_normalization.clone());

        // 12. Start watching resources based on Controller's supported kinds
        let mut otlp_startup_guard = OtlpStartupGuard {
            runtime: &runtime,
            armed: true,
        };
        runtime
            .block_on(sync_client.start_watch_kinds(&server_info.supported_kinds))
            .map_err(|e| anyhow!("Failed to start watching resources: {}", e))?;
        tracing::info!(
            supported_kinds = ?server_info.supported_kinds,
            "Started watching resources from Controller"
        );

        // 13. Start watching server metadata (gateway instance count for Cluster scope rate limiting)
        let sync_client_arc = Arc::new(sync_client);
        let sync_client_for_meta = sync_client_arc.clone();
        runtime.spawn(async move {
            sync_client_for_meta.run_leader_session().await;
        });

        // 13b. Hot-reload the conf_sync client TLS identity (spec §6 T4): watch the
        // active TLS set's file paths for in-place replacement (cert-manager renewal,
        // Secret update). Only when a real file-backed mTLS set is active — plaintext
        // and skip_tls have nothing to reload. Flipping `active` remains restart-bound.
        if let Some(tls) = config.conf_sync_security.resolve_active_tls().filter(|t| !t.skip_tls) {
            use crate::conf_sync::conf_client::cert_watch::{spawn_client_cert_watcher, CertPaths};
            let paths = CertPaths {
                cert: tls.cert_path(),
                key: tls.key_path(),
                ca: tls.ca_path(),
            };
            // Process-lifetime watcher: the gateway exits the process on shutdown,
            // so a never-cancelled token is sufficient here.
            let shutdown = tokio_util::sync::CancellationToken::new();
            runtime.block_on(async {
                spawn_client_cert_watcher(sync_client_arc.clone(), paths, shutdown);
            });
        }

        // 16. Initialize access log options (global OnceLock for hot-path access)
        config::init_access_log_options(config.access_log_options.clone());

        // 17. Start auxiliary services
        // Create shutdown channel for auxiliary services
        let (aux_shutdown_tx, aux_shutdown_rx) = tokio::sync::watch::channel(false);
        runtime.block_on(Self::start_auxiliary_services(
            config_client.clone(),
            sync_client_arc.clone(),
            &config,
            aux_shutdown_rx.clone(),
        ));

        // 18. Wait for all resources ready (including Gateway/GatewayClass/EdgionGatewayConfig)
        runtime.block_on(Self::wait_for_ready(config_client.clone()))?;

        // 19. Initialize all loggers. Service selectors are compiled eagerly
        // with each joint Service/EndpointSlice generation, so no preload pass
        // is needed here.
        runtime.block_on(init_access_logger(&config.access_log))?;
        runtime.block_on(init_ssl_logger(&config.ssl_log))?;
        runtime.block_on(init_tcp_logger(&config.tcp_log))?;
        runtime.block_on(init_tls_logger(&config.tls_log))?;
        runtime.block_on(init_udp_logger(&config.udp_log))?;

        tracing::info!("All loggers initialized (access, ssl, tcp, tls, udp)");

        // 21. Create and configure Pingora server (in Tokio runtime context for UDP listeners)
        let pingora_server = runtime.block_on(async {
            tokio::task::spawn_blocking(move || create_and_configure_server(config_client, &config))
                .await
                .map_err(|error| anyhow!("Gateway listener construction task failed: {error}"))?
        });
        let pingora_server = match pingora_server {
            Ok(server) => server,
            Err(error) => {
                // A later listener bind can fail after earlier UDP tasks have
                // started. Drain them while their runtime is still available.
                get_udp_listener_registry().cancel_all();
                let _ = aux_shutdown_tx.send(true);
                let _ = runtime.block_on(Self::drain_udp_listeners());
                return Err(error);
            }
        };

        // 22. Move runtime to background thread with shutdown-aware lifecycle
        otlp_startup_guard.armed = false;
        drop(otlp_startup_guard);
        let bg_shutdown_rx = aux_shutdown_rx;
        let bg_thread = std::thread::spawn(move || {
            runtime.block_on(async {
                let mut rx = bg_shutdown_rx;
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                // UDP shutdown can emit final access logs. Finish its drain
                // before flushing OTLP, including when UDP reports a failure.
                let udp_result = Self::drain_udp_listeners().await;
                crate::link_sys::runtime::store::shutdown_otlp_clients().await;
                udp_result
            })
        });

        // 23. Run Pingora server (blocks until shutdown)
        let run_result = run_server(pingora_server);

        // 24. Signal auxiliary services to shut down
        get_udp_listener_registry().cancel_all();
        tracing::info!(
            component = "gateway",
            "Pingora stopped, shutting down auxiliary services"
        );
        let _ = aux_shutdown_tx.send(true);

        // 25. Wait for UDP tasks and the shared auxiliary grace window.
        let shutdown_result = bg_thread
            .join()
            .map_err(|_| anyhow!("Gateway auxiliary shutdown thread panicked"))?;
        run_result?;
        shutdown_result?;

        tracing::info!(component = "gateway", "Gateway shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/cli/mod_test/config_diagnostic_tests.rs"]
mod config_diagnostic_tests;

#[cfg(test)]
#[path = "../../tests/unit/cli/mod_test/otlp_startup_tests.rs"]
mod otlp_startup_tests;
