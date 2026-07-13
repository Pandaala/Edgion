use crate::conf_sync::conf_client::cert_watch::CertPaths;
use crate::conf_sync::conf_client::ConfigClient;
use arc_swap::ArcSwap;
use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use edgion_common::conf_sync::proto::{ServerInfoRequest, ServerInfoResponse, WatchServerMetaRequest};
use edgion_resources::rate_limit_config::set_gateway_instance_count;
use edgion_resources::ResourceKind;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, ClientTlsConfig};
use tracing;
use uuid::Uuid;

/// Parameters captured at `new()` so the cert watcher can rebuild the TLS
/// channel in place (spec §6 T4). Present only when the active TLS set is a
/// real file-backed mTLS identity (not plaintext / `skip_tls`).
struct ClientTlsReload {
    paths: CertPaths,
    grpc_server_addr: String,
    timeout: Duration,
}

/// gRPC conf_client for ConfigSync service
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
    /// Swappable service handle. Held behind `ArcSwap` so `reload_client_tls`
    /// can atomically install a channel built on freshly-rotated certs without
    /// tearing down the client (spec §6 T4: WatchServerMeta must not retry a
    /// retired TLS config forever).
    conf_client_handle: ArcSwap<ConfigSyncClientService<Channel>>,
    /// Client ID for WatchServerMeta registration
    client_id: String,
    /// Client name for WatchServerMeta registration
    client_name: String,
    /// Reload-generation token. Cancelled on each reload so the WatchServerMeta
    /// loop breaks out and reconnects on the new channel. Kept in lockstep with
    /// the process-global token in `conf_client/mod.rs` (which the per-kind
    /// watch loops read).
    reload_generation: ArcSwap<CancellationToken>,
    /// File paths + endpoint params for rebuilding the channel on cert rotation.
    /// `None` when the channel is plaintext / `skip_tls` (nothing to reload).
    client_tls: Option<ClientTlsReload>,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync client and connect to the server.
    /// Uses lazy connection to allow cold start even if the server is not available immediately.
    ///
    /// TODO: Currently this allows cold start (gateway starts without controller).
    /// However, without local cache persistence, the gateway will have no configuration
    /// and cannot serve traffic until it successfully connects to the controller.
    /// Future improvements should include:
    /// 1. Persisting configuration to local disk (Snapshot)
    /// 2. Loading from local disk on startup if controller is unreachable
    pub async fn new(
        grpc_server_addr: &str,
        client_name: String,
        timeout: Duration,
        security: &ConfSyncSecurityConfig,
    ) -> anyhow::Result<Self> {
        let client_id = Uuid::new_v4().to_string();
        let config_client = Arc::new(ConfigClient::new(client_id.clone(), client_name.clone()));

        let tls_config = security.resolve_active_tls();
        let use_tls = tls_config.is_some_and(|t| !t.skip_tls);
        let uri = build_grpc_uri(grpc_server_addr, use_tls);

        tracing::info!(
            server_addr = grpc_server_addr,
            tls_enabled = use_tls,
            "Initializing gRPC client with lazy connection"
        );

        // Rotation is a controller-only concern. The gateway never validates it and
        // never fails startup on it (spec §5.2) — just surface that it is ignored.
        if security.rotation.is_some() {
            tracing::warn!(
                component = "grpc_client",
                "conf_sync_security.rotation config present but ignored by the gateway (controller-only)"
            );
        }

        // Resolve the TLS material (if any) and capture the reload params for the
        // file-mode cert watcher. `client_tls_reload` stays `None` for plaintext
        // and `skip_tls` — there is nothing to hot-reload in those modes.
        let mut client_tls_reload: Option<ClientTlsReload> = None;
        let tls_config_opt: Option<ClientTlsConfig> = if let Some(tls) = tls_config {
            if tls.skip_tls {
                // Explicit emergency escape hatch — plaintext even when auto_ca is on.
                // auto_ca's contract is "always mTLS", so a skip_tls override of it is a
                // higher-severity event than a bare skip_tls; surface it loudly.
                if security.auto_ca {
                    tracing::warn!(
                        component = "grpc_client",
                        "conf_sync skip_tls=true OVERRIDES auto_ca=true: connecting WITHOUT TLS. \
                         Emergency use only — auto_ca is meant to guarantee mTLS on this channel."
                    );
                }
                tracing::error!(
                    component = "grpc_client",
                    "conf_sync mTLS skip_tls=true: connecting WITHOUT TLS. INSECURE, emergency use only!"
                );
                edgion_common::observe::system_metrics::mark_system_error(
                    edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
                    edgion_common::observe::system_metrics::reasons::SKIP_TLS,
                );
                None
            } else {
                let client_tls = edgion_common::grpc_tls::load_client_tls(tls).await?;
                tracing::info!(component = "grpc_client", "conf_sync mTLS enabled for gRPC client");
                client_tls_reload = Some(ClientTlsReload {
                    paths: CertPaths {
                        cert: tls.cert_path(),
                        key: tls.key_path(),
                        ca: tls.ca_path(),
                    },
                    grpc_server_addr: grpc_server_addr.to_string(),
                    timeout,
                });
                Some(client_tls)
            }
        } else {
            // resolve_active_tls() == None. validate() already rejected the
            // dangling-`active` case, so this is the truly-omitted section.
            debug_assert!(security.is_plaintext_omitted());
            // auto_ca=true means the Controller's internal CA provisions the conf_sync
            // certs and the channel MUST be mTLS. An omitted TLS block under auto_ca is a
            // misconfiguration, NOT a license to run plaintext: fail closed rather than
            // silently downgrade a Secret-carrying channel. This mirrors the Controller
            // side, whose provision() failure aborts startup.
            if security.auto_ca {
                anyhow::bail!(
                    "conf_sync_security.auto_ca=true but no TLS certs are configured (no `certs`/`tls`). \
                     auto_ca requires mTLS; refusing to connect in plaintext. Configure the conf_sync \
                     cert paths (the Controller's internal CA provisions the gateway Secret)."
                );
            }
            tracing::error!(
                component = "grpc_client",
                "conf_sync_security omitted: connecting in PLAINTEXT. This channel carries \
                 resolved Secrets (TLS keys, backend credentials). Configure mTLS for production."
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
                edgion_common::observe::system_metrics::reasons::PLAINTEXT_DEFAULT,
            );
            None
        };

        // connect_lazy returns a Channel immediately; connection happens on first request.
        let channel = build_channel(&uri, timeout, tls_config_opt)?;
        let client = ConfigSyncClientService::new(channel);

        // Set gRPC client for all caches via unified dispatch.
        // Uses ResourceKind exhaustive match internally, so adding a new kind
        // without handling it here will cause a compile error.
        config_client.set_all_grpc_clients(client.clone()).await;

        // Install the initial (never-cancelled) reload-generation token, shared
        // with the process-global so the per-kind watch loops observe the same one.
        let generation = Arc::new(CancellationToken::new());
        super::set_global_reload_generation(generation.clone());

        Ok(Self {
            config_client,
            conf_client_handle: ArcSwap::from_pointee(client),
            client_id,
            client_name,
            reload_generation: ArcSwap::new(generation),
            client_tls: client_tls_reload,
        })
    }

    /// Rotate the reload-generation token: cancel the current one (so every
    /// running watch loop selecting on it breaks out) and install a fresh
    /// never-cancelled token in both the instance field and the process-global.
    /// Factored out of `reload_client_tls` so it can be unit-tested directly.
    fn rotate_generation(&self) {
        // Instance and global point at the same Arc, so one cancel is enough.
        self.reload_generation.load().cancel();
        let new_token = Arc::new(CancellationToken::new());
        self.reload_generation.store(new_token.clone());
        super::set_global_reload_generation(new_token);
    }

    /// Hot-reload the client TLS identity from disk (spec §6 T4).
    ///
    /// Reads + validates the configured cert/key/CA (`validate_identity`), then
    /// builds a new lazily-connected channel and installs it. On any error the
    /// current channel is left untouched — the caller keeps serving on it.
    ///
    /// Swap order (see module notes / task report): (1) `set_all_grpc_clients`
    /// stores the new channel in every per-kind cache's client slot; (2) store
    /// the new handle for `get_server_info` / `start_watch_server_meta`; (3)
    /// `rotate_generation` cancels the old token LAST, so when the running watch
    /// loops break and re-clone their client they read the already-installed new
    /// channel (never the retired one).
    pub async fn reload_client_tls(self: &Arc<Self>) -> anyhow::Result<()> {
        let reload = self
            .client_tls
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conf_sync client TLS reload is not configured (plaintext/skip_tls)"))?;

        let (cert, key, ca) = crate::conf_sync::conf_client::cert_watch::read_and_validate(&reload.paths).await?;

        let client_tls = ClientTlsConfig::new()
            .ca_certificate(tonic::transport::Certificate::from_pem(ca))
            .identity(tonic::transport::Identity::from_pem(cert, key));
        let uri = build_grpc_uri(&reload.grpc_server_addr, true);
        let channel = build_channel(&uri, reload.timeout, Some(client_tls))?;
        let new_client = ConfigSyncClientService::new(channel);

        // (1) Point every per-kind cache at the new channel BEFORE cancelling, so
        // a loop that breaks re-clones the new client, never the retired one.
        self.config_client.set_all_grpc_clients(new_client.clone()).await;
        // (2) Install the new handle for get_server_info / WatchServerMeta.
        self.conf_client_handle.store(Arc::new(new_client));
        // (3) Cancel the old generation LAST → running loops break and reconnect.
        self.rotate_generation();
        Ok(())
    }

    /// Get a reference to the `ConfigClient`.
    pub fn get_config_client(&self) -> Arc<ConfigClient> {
        self.config_client.clone()
    }

    /// Get server information including endpoint mode and supported resource kinds
    pub async fn get_server_info(&self) -> Result<ServerInfoResponse, tonic::Status> {
        // Read the current handle through the ArcSwap and clone it out (the
        // service wraps a cheaply-cloneable Channel); do not hold the guard.
        let mut handle = self.conf_client_handle.load().as_ref().clone();
        let response = handle.get_server_info(ServerInfoRequest {}).await?;

        let info = response.into_inner();

        tracing::info!(
            server_id = %info.server_id,
            supported_kinds = ?info.supported_kinds,
            "Received server info"
        );

        Ok(info)
    }

    /// Start watching server metadata (gateway instance count, etc.)
    ///
    /// Runs as a background task. Updates the global GATEWAY_INSTANCE_COUNT
    /// via set_gateway_instance_count(). Automatically reconnects on failure.
    ///
    /// On server switch (server_id change mid-stream), reconnect immediately
    /// without backoff to ensure fast convergence. Exponential backoff is only
    /// applied for real connection/transport failures.
    ///
    /// This should be called once at startup via tokio::spawn.
    pub async fn start_watch_server_meta(self: Arc<Self>) {
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            tracing::info!("Starting WatchServerMeta stream...");

            // Load the current handle and generation token per reconnect iteration
            // so a hot TLS reload is picked up on the very next connect.
            let gen_token = self.reload_generation.load_full();
            let mut handle = self.conf_client_handle.load().as_ref().clone();

            match handle
                .watch_server_meta(WatchServerMetaRequest {
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                })
                .await
            {
                Ok(response) => {
                    backoff = Duration::from_secs(1); // Reset on successful connection
                    let mut stream = response.into_inner();

                    // Track the server_id seen on the first event of this stream session.
                    // If a subsequent event carries a different server_id it is the terminal
                    // server-switch signal from the server side (Stage A); reconnect immediately.
                    let mut stream_server_id: Option<String> = None;
                    let mut server_switched = false;
                    // Set when the TLS reload token cancels this stream; like a server
                    // switch it reconnects immediately (on the new channel), no backoff.
                    let mut reloaded = false;

                    loop {
                        tokio::select! {
                            _ = gen_token.cancelled() => {
                                tracing::info!(
                                    "WatchServerMeta: client TLS reloaded, reconnecting on new channel"
                                );
                                reloaded = true;
                                break;
                            }
                            msg = stream.message() => {
                                match msg {
                                    Ok(Some(event)) => {
                                        set_gateway_instance_count(event.gateway_instance_count);

                                        if !event.server_id.is_empty() {
                                            match &stream_server_id {
                                                None => {
                                                    // First event: record the authoritative server_id
                                                    // and update the config client.
                                                    stream_server_id = Some(event.server_id.clone());
                                                    self.config_client.set_current_server_id(event.server_id);
                                                }
                                                Some(sid) if *sid != event.server_id => {
                                                    // server_id changed mid-stream — server has reloaded.
                                                    // Update config client with new server_id and reconnect
                                                    // immediately (skip backoff sleep below).
                                                    tracing::info!(
                                                        old_server_id = %sid,
                                                        new_server_id = %event.server_id,
                                                        "WatchServerMeta: server switch detected, reconnecting immediately"
                                                    );
                                                    self.config_client.set_current_server_id(event.server_id);
                                                    server_switched = true;
                                                    break;
                                                }
                                                Some(_) => {
                                                    // Same server_id — normal update.
                                                    self.config_client.set_current_server_id(event.server_id);
                                                }
                                            }
                                        }
                                    }
                                    Ok(None) => {
                                        tracing::warn!("WatchServerMeta stream ended, reconnecting...");
                                        break;
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            "WatchServerMeta stream error, reconnecting..."
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    if server_switched || reloaded {
                        // Intentional switch/reload — skip backoff and reconnect immediately.
                        continue;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        backoff_secs = backoff.as_secs(),
                        "WatchServerMeta connection failed, retrying..."
                    );
                }
            }

            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff); // Exponential backoff for real failures
        }
    }

    /// Start watching resource kinds based on server's supported kinds.
    ///
    /// Resolves each kind name to ResourceKind via the exhaustive enum, then
    /// dispatches to the corresponding cache's start_watch.
    ///
    /// - If a kind resolves to a cache that exists on Gateway: start watch.
    /// - If a kind resolves but has no Gateway cache (Secret, ReferenceGrant): skip.
    /// - If a kind string is completely unknown: warn and skip (forward compat).
    pub async fn start_watch_kinds(&mut self, supported_kinds: &[String]) -> Result<(), tonic::Status> {
        let mut effective_kinds = supported_kinds.to_vec();
        for required in [
            "GatewayClass".to_string(),
            "Gateway".to_string(),
            "EdgionGatewayConfig".to_string(),
        ] {
            if !effective_kinds.iter().any(|k| k == &required) {
                effective_kinds.push(required);
            }
        }

        tracing::info!(
            supported_kinds = ?supported_kinds,
            effective_kinds = ?effective_kinds,
            "Starting watch for supported resource kinds"
        );
        self.config_client.set_required_kinds_for_readiness(&effective_kinds);

        for kind_name in &effective_kinds {
            // Resolve string -> ResourceKind via exhaustive enum
            let kind = match ResourceKind::from_kind_name(kind_name) {
                Some(k) => k,
                None => {
                    // Truly unknown kind string — forward compat with newer server
                    tracing::warn!(kind = %kind_name, "Unknown resource kind from server, skipping");
                    continue;
                }
            };

            match self.config_client.get_dyn_cache(kind) {
                Some(cache) => match cache.start_watch_dyn().await {
                    Ok(_) => {
                        tracing::info!(kind = %kind_name, "Watch started successfully");
                    }
                    Err(e) => {
                        tracing::error!(kind = %kind_name, error = %e, "Failed to start watch");
                    }
                },
                None => {
                    // Known kind but intentionally not cached on Gateway (Secret, ReferenceGrant)
                    tracing::debug!(kind = %kind_name, "Resource not cached on Gateway, skipping watch");
                }
            }
        }

        Ok(())
    }
}

/// Build a lazily-connected gRPC [`Channel`] for `uri` with the shared keepalive
/// tuning, optionally applying `tls`. Extracted from `new()` so `reload_client_tls`
/// rebuilds the channel identically. `connect_lazy` defers the actual dial to the
/// first request, so this never blocks or fails on an unreachable server.
fn build_channel(uri: &str, timeout: Duration, tls: Option<ClientTlsConfig>) -> anyhow::Result<Channel> {
    let mut endpoint = tonic::transport::Endpoint::from_shared(uri.to_string())
        .map_err(|e| anyhow::anyhow!("Invalid gRPC URI '{}': {}", uri, e))?
        .timeout(timeout)
        .connect_timeout(timeout)
        .http2_keep_alive_interval(Duration::from_secs(10))
        .keep_alive_timeout(Duration::from_secs(5))
        .keep_alive_while_idle(true);

    if let Some(tls) = tls {
        endpoint = endpoint
            .tls_config(tls)
            .map_err(|e| anyhow::anyhow!("Failed to configure client TLS: {}", e))?;
    }

    Ok(endpoint.connect_lazy())
}

/// Build the gRPC URI with the appropriate scheme based on TLS setting.
fn build_grpc_uri(addr: &str, tls: bool) -> String {
    let host = addr
        .strip_prefix("https://")
        .or_else(|| addr.strip_prefix("http://"))
        .unwrap_or(addr);
    if tls {
        format!("https://{}", host)
    } else {
        format!("http://{}", host)
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the small pure helpers in this module, plus the
    //! `ConfigSyncClient::new` security-branch decisions. `new` uses
    //! `connect_lazy`, so it builds a Channel without any network I/O; the
    //! auto_ca fail-close / plaintext branches are reachable without a live
    //! server. The streaming paths (`start_watch_*`) still need integration
    //! coverage.
    use super::build_grpc_uri;
    use super::ConfigSyncClient;
    use edgion_common::conf_sync::config::{ConfSyncSecurityConfig, ConfSyncTlsConfig};
    use std::time::Duration;

    /// A loopback address that is never dialed (connect_lazy defers the connect).
    const DUMMY_ADDR: &str = "127.0.0.1:65000";

    #[tokio::test]
    async fn reload_cancels_previous_generation() {
        // Uses a plaintext client against no server: connect_lazy never dials until use.
        let sec = ConfSyncSecurityConfig::default();
        let client = ConfigSyncClient::new("127.0.0.1:1", "t".into(), Duration::from_secs(1), &sec)
            .await
            .unwrap();
        let client = std::sync::Arc::new(client);
        let old_gen = client.reload_generation.load_full();
        assert!(!old_gen.is_cancelled());
        client.rotate_generation(); // the token-swap half of reload_client_tls, factored out
        assert!(old_gen.is_cancelled());
        assert!(!client.reload_generation.load_full().is_cancelled());
    }

    #[tokio::test]
    async fn auto_ca_with_omitted_tls_fails_closed_not_plaintext() {
        // auto_ca=true means "always mTLS". An omitted TLS block is a misconfig,
        // so `new` must refuse rather than silently downgrade to plaintext.
        let security = ConfSyncSecurityConfig {
            auto_ca: true,
            ..Default::default()
        };
        // ConfigSyncClient is not Debug, so avoid expect_err (needs Debug on the Ok type).
        let Err(err) = ConfigSyncClient::new(DUMMY_ADDR, "test".to_string(), Duration::from_secs(1), &security).await
        else {
            panic!("auto_ca + omitted TLS must fail closed");
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("auto_ca"),
            "error must explain the auto_ca contract, got: {msg}"
        );
    }

    #[tokio::test]
    async fn no_auto_ca_with_omitted_tls_still_connects_plaintext() {
        // Backward-compatible interim stance: without auto_ca, an omitted section
        // keeps the plaintext-default behavior (loud telemetry, but connects).
        let security = ConfSyncSecurityConfig::default();
        let client = ConfigSyncClient::new(DUMMY_ADDR, "test".to_string(), Duration::from_secs(1), &security).await;
        assert!(client.is_ok(), "non-auto_ca omitted must keep plaintext-default (Ok)");
    }

    #[tokio::test]
    async fn auto_ca_with_skip_tls_is_allowed_escape_hatch() {
        // skip_tls is the explicit emergency bypass; it overrides auto_ca (with a
        // loud WARN at runtime) and must NOT bail. No cert files are read.
        let security = ConfSyncSecurityConfig {
            auto_ca: true,
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: String::new(),
                key: String::new(),
                ca: String::new(),
                skip_tls: true,
            }),
            ..Default::default()
        };
        let client = ConfigSyncClient::new(DUMMY_ADDR, "test".to_string(), Duration::from_secs(1), &security).await;
        assert!(
            client.is_ok(),
            "skip_tls must remain a usable escape hatch under auto_ca"
        );
    }

    #[test]
    fn plain_addr_without_tls_uses_http_scheme() {
        assert_eq!(build_grpc_uri("controller:9443", false), "http://controller:9443");
    }

    #[test]
    fn plain_addr_with_tls_uses_https_scheme() {
        assert_eq!(build_grpc_uri("controller:9443", true), "https://controller:9443");
    }

    #[test]
    fn http_prefix_stripped_then_https_applied_when_tls_enabled() {
        // Operator may configure `http://...`; when TLS is on we still emit `https://`.
        assert_eq!(
            build_grpc_uri("http://controller:9443", true),
            "https://controller:9443"
        );
    }

    #[test]
    fn https_prefix_stripped_then_http_applied_when_tls_disabled() {
        // Symmetric: a stale `https://` prefix must not survive when TLS is off.
        assert_eq!(
            build_grpc_uri("https://controller:9443", false),
            "http://controller:9443"
        );
    }

    #[test]
    fn ipv4_with_port_round_trips() {
        assert_eq!(build_grpc_uri("127.0.0.1:9443", false), "http://127.0.0.1:9443");
        assert_eq!(build_grpc_uri("127.0.0.1:9443", true), "https://127.0.0.1:9443");
    }

    #[test]
    fn matching_existing_scheme_is_idempotent() {
        // Same scheme requested → unchanged URI.
        assert_eq!(
            build_grpc_uri("https://controller:9443", true),
            "https://controller:9443"
        );
        assert_eq!(
            build_grpc_uri("http://controller:9443", false),
            "http://controller:9443"
        );
    }
}
