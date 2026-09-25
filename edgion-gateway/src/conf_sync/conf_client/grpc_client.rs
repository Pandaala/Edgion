use crate::conf_sync::conf_client::cert_watch::CertPaths;
use crate::conf_sync::conf_client::rotation::EndpointRotator;
use crate::conf_sync::conf_client::ConfigClient;
use arc_swap::ArcSwap;
use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_common::conf_sync::payload::MAX_MESSAGE_BYTES;
use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use edgion_common::conf_sync::proto::{ServerInfoRequest, ServerInfoResponse, WatchServerMetaRequest};
use edgion_resources::rate_limit_local_config::set_gateway_instance_count;
use edgion_resources::ResourceKind;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, ClientTlsConfig};
use tracing;
use uuid::Uuid;

/// First-message deadline for the meta stream (spec §7.4): the server sends an
/// initial event unconditionally, so an open-but-silent stream is a wedged peer.
/// Applies ONLY to the meta stream — per-kind watch streams are legitimately
/// silent for quiet kinds and must never carry this deadline.
const FIRST_MSG_DEADLINE: Duration = Duration::from_secs(10);
/// Dwell before rotating away from an address that has previously delivered a
/// meta event (spec §7.4 H2): a controller reload is indistinguishable from a
/// follower by status code, and one dwell absorbs it.
const DWELL: Duration = Duration::from_secs(3);

/// What the coordinator does after a failed attempt on the current address
/// (spec §7.4 H2). Pure so it is unit-testable without a live server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RotateDecision {
    RotateNow,
    Dwell,
}

/// - a peer that never delivered a meta event in this process's lifetime is a
///   follower/dead peer → rotate immediately (bootstrap, kill -9 at startup);
/// - a demoted leader's active stream close → rotate immediately;
/// - an ex-deliverer gets exactly one dwell (reload protection), then rotates.
fn rotate_decision(peer_has_delivered: bool, failures_on_current: u32, demoted_close: bool) -> RotateDecision {
    if demoted_close || !peer_has_delivered || failures_on_current > 1 {
        RotateDecision::RotateNow
    } else {
        RotateDecision::Dwell
    }
}

/// Mark the conf_sync link to the controller as down: a reconnect cycle failed
/// and the client is backing off. Cleared by [`mark_confsync_connected`] once the
/// `WatchServerMeta` stream is re-established.
fn mark_confsync_disconnected() {
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
        edgion_common::observe::system_metrics::reasons::CONFSYNC_DISCONNECTED,
    );
}

/// Clear the `confsync_disconnected` mark — the link to the controller is back up.
fn mark_confsync_connected() {
    edgion_common::observe::system_metrics::clear_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_CLIENT,
        edgion_common::observe::system_metrics::reasons::CONFSYNC_DISCONNECTED,
    );
}

/// One coherent installed transport generation. Candidates never mutate it.
struct InstalledChannel {
    address: String,
    index: usize,
    client: ConfigSyncClientService<Channel>,
    generation: Arc<CancellationToken>,
}

/// The finite publisher is Arc-owned so caller cancellation cannot interrupt a
/// sequence of cache-client replacements. No RPC runs under the swap lock.
struct ChannelManager {
    installed: ArcSwap<InstalledChannel>,
    config_client: Arc<ConfigClient>,
    swap_lock: tokio::sync::Mutex<EndpointRotator>,
}

impl ChannelManager {
    fn is_current(&self, expected: &Arc<InstalledChannel>) -> bool {
        Arc::ptr_eq(&self.installed.load_full(), expected)
    }

    async fn publish_locked(
        &self,
        address: String,
        index: usize,
        client: ConfigSyncClientService<Channel>,
        rotator: &mut EndpointRotator,
    ) -> Arc<InstalledChannel> {
        let old = self.installed.load_full();
        self.config_client.set_all_grpc_clients(client.clone()).await;
        let generation = Arc::new(CancellationToken::new());
        let installed = Arc::new(InstalledChannel {
            address,
            index,
            client,
            generation: generation.clone(),
        });
        self.installed.store(installed.clone());
        rotator.installed();
        super::set_global_reload_generation(generation);
        old.generation.cancel();
        installed
    }

    async fn commit(
        self: &Arc<Self>,
        expected: Arc<InstalledChannel>,
        index: usize,
        revision: u64,
        client: ConfigSyncClientService<Channel>,
    ) -> anyhow::Result<Option<Arc<InstalledChannel>>> {
        let manager = self.clone();
        // Dropping this JoinHandle does not abort a partially completed commit.
        tokio::spawn(async move {
            let mut rotator = manager.swap_lock.lock().await;
            if !manager.is_current(&expected) || !rotator.matches(index, revision) {
                return None;
            }
            let address = rotator.address(index).to_owned();
            Some(manager.publish_locked(address, index, client, &mut rotator).await)
        })
        .await
        .map_err(|error| anyhow::anyhow!("conf_sync channel publisher failed: {error}"))
    }
}

enum RecoveryOutcome {
    Installed(ServerInfoResponse),
    Retry { error: anyhow::Error, local: bool },
    Superseded,
}

/// gRPC conf_client for ConfigSync service.
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
    channels: Arc<ChannelManager>,
    client_id: String,
    client_name: String,
    client_tls: Option<CertPaths>,
    use_tls: bool,
    timeout: Duration,
    single_address: bool,
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
        server_addrs: Vec<String>,
        client_name: String,
        timeout: Duration,
        security: &ConfSyncSecurityConfig,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!server_addrs.is_empty(), "server_addrs must not be empty");
        let client_id = Uuid::new_v4().to_string();
        let gateway_scope = std::env::var("EDGION_GATEWAY_SCOPE").ok();
        let gateway_uid = std::env::var("EDGION_GATEWAY_UID").ok();
        anyhow::ensure!(
            gateway_uid
                .as_ref()
                .is_none_or(|uid| !uid.is_empty() && gateway_scope.is_some()),
            "EDGION_GATEWAY_UID requires EDGION_GATEWAY_SCOPE"
        );
        if let Some(scope) = gateway_scope.as_deref() {
            let Some((namespace, name)) = scope.split_once('/') else {
                anyhow::bail!("EDGION_GATEWAY_SCOPE must have namespace/name format");
            };
            anyhow::ensure!(
                !namespace.is_empty() && !name.is_empty() && !name.contains('/'),
                "EDGION_GATEWAY_SCOPE must have namespace/name format"
            );
        }
        let config_client = Arc::new(ConfigClient::new_scoped(
            client_id.clone(),
            client_name.clone(),
            gateway_scope,
            gateway_uid,
        ));

        // Shuffled per gateway (spec §7.5); the initial channel points at the
        // rotator's current address.
        let rotator = EndpointRotator::new(server_addrs.clone());
        let initial_addr = rotator.address(0).to_owned();
        let single_address = rotator.is_single();
        // URI syntax is immutable configuration. Reject it at startup rather
        // than permanently retrying a malformed candidate during recovery.
        for address in &server_addrs {
            tonic::transport::Endpoint::from_shared(build_grpc_uri(
                address,
                security.resolve_active_tls().is_some_and(|tls| !tls.skip_tls),
            ))?;
        }

        let tls_config = security.resolve_active_tls();
        let use_tls = tls_config.is_some_and(|t| !t.skip_tls);
        let uri = build_grpc_uri(&initial_addr, use_tls);

        tracing::info!(
            server_addr_count = server_addrs.len(),
            initial_addr = %initial_addr,
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

        // Resolve the TLS material (if any) and capture the cert paths for the
        // file-mode cert watcher. `client_tls_reload` stays `None` for plaintext
        // and `skip_tls` — there is nothing to hot-reload in those modes.
        let mut client_tls_reload: Option<CertPaths> = None;
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
                client_tls_reload = Some(CertPaths {
                    cert: tls.cert_path(),
                    key: tls.key_path(),
                    ca: tls.ca_path(),
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
            // Fail-close (2026-07-14): plaintext-by-omission is no longer connected. Only an
            // explicit `skip_tls: true` (handled above) may run this channel in plaintext.
            security.ensure_transport_configured()?;
            // Unreachable in practice: the only other shape that resolves to no TLS is a
            // dangling `active`, which `validate()` already rejects. Refuse anyway rather
            // than fall through to a plaintext channel.
            anyhow::bail!(
                "conf_sync_security resolves to no TLS material; refusing to connect the \
                 Secret-carrying conf_sync channel in plaintext"
            );
        };

        // connect_lazy returns a Channel immediately; connection happens on first request.
        let channel = build_channel(&uri, timeout, tls_config_opt)?;
        let client = configured_client(channel);

        // Set gRPC client for all caches via unified dispatch.
        // Uses ResourceKind exhaustive match internally, so adding a new kind
        // without handling it here will cause a compile error.
        config_client.set_all_grpc_clients(client.clone()).await;

        // Install the initial (never-cancelled) reload-generation token, shared
        // with the process-global so the per-kind watch loops observe the same one.
        let generation = Arc::new(CancellationToken::new());
        super::set_global_reload_generation(generation.clone());

        let channels = Arc::new(ChannelManager {
            installed: ArcSwap::from_pointee(InstalledChannel {
                address: initial_addr,
                index: 0,
                client,
                generation,
            }),
            config_client: config_client.clone(),
            swap_lock: tokio::sync::Mutex::new(rotator),
        });
        Ok(Self {
            config_client,
            channels,
            client_id,
            client_name,
            client_tls: client_tls_reload,
            use_tls,
            timeout,
            single_address,
        })
    }

    /// Probe a replacement without disturbing the installed channel. Both the
    /// installed snapshot and traversal ticket fence asynchronous results.
    async fn recover_step(&self, expected: Arc<InstalledChannel>) -> anyhow::Result<RecoveryOutcome> {
        if self.single_address || !self.channels.is_current(&expected) {
            return Ok(RecoveryOutcome::Superseded);
        }
        let (index, revision, address, delay) = {
            let mut rotator = self.channels.swap_lock.lock().await;
            if !self.channels.is_current(&expected) {
                return Ok(RecoveryOutcome::Superseded);
            }
            rotator.begin_search(expected.index);
            let (index, revision) = rotator.candidate();
            (
                index,
                revision,
                rotator.address(index).to_owned(),
                rotator.take_lap_delay(),
            )
        };
        if let Some(delay) = delay {
            tokio::select! {
                biased;
                _ = expected.generation.cancelled() => return Ok(RecoveryOutcome::Superseded),
                _ = tokio::time::sleep(delay) => {}
            }
        }
        let prepared = tokio::select! {
            biased;
            _ = expected.generation.cancelled() => return Ok(RecoveryOutcome::Superseded),
            result = prepare_client(&address, self.use_tls, self.timeout, self.client_tls.as_ref()) => result,
        };
        let mut client = match prepared {
            Ok(client) => client,
            Err(error) => {
                let rotator = self.channels.swap_lock.lock().await;
                if !self.channels.is_current(&expected) || !rotator.matches(index, revision) {
                    return Ok(RecoveryOutcome::Superseded);
                }
                return Ok(RecoveryOutcome::Retry { error, local: true });
            }
        };
        let result = tokio::select! {
            biased;
            _ = expected.generation.cancelled() => return Ok(RecoveryOutcome::Superseded),
            result = request_server_info(&mut client, self.timeout) => result,
        };
        match result {
            Ok(info) => match self.channels.commit(expected, index, revision, client).await? {
                Some(installed) if self.channels.is_current(&installed) => Ok(RecoveryOutcome::Installed(info)),
                _ => Ok(RecoveryOutcome::Superseded),
            },
            Err(error) => {
                let mut rotator = self.channels.swap_lock.lock().await;
                if !self.channels.is_current(&expected) || !rotator.matches(index, revision) {
                    return Ok(RecoveryOutcome::Superseded);
                }
                rotator.note_failure();
                Ok(RecoveryOutcome::Retry {
                    error: error.into(),
                    local: false,
                })
            }
        }
    }

    /// Bootstrap uses the same candidate probe and transaction as runtime
    /// recovery. Every initial request or candidate preparation consumes one of
    /// the twenty attempts; successful candidate info is returned without redial.
    pub async fn bootstrap_server_info(&self) -> anyhow::Result<ServerInfoResponse> {
        const MAX_ATTEMPTS: usize = 20;
        let mut failed: Option<Arc<InstalledChannel>> = None;
        let mut last_error = anyhow::anyhow!("No conf_sync controller attempt completed");
        for attempt in 0..MAX_ATTEMPTS {
            if let Some(expected) = failed.clone() {
                match self.recover_step(expected.clone()).await? {
                    RecoveryOutcome::Installed(info) => return Ok(info),
                    RecoveryOutcome::Superseded => {
                        failed = None;
                    }
                    RecoveryOutcome::Retry { error, local } => {
                        last_error = error;
                        if local {
                            tokio::select! {
                                _ = expected.generation.cancelled() => { failed = None; }
                                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            }
                        }
                    }
                }
            } else {
                let installed = self.channels.installed.load_full();
                let mut handle = installed.client.clone();
                let result = tokio::select! {
                    biased;
                    _ = installed.generation.cancelled() => continue,
                    result = request_server_info(&mut handle, self.timeout) => result,
                };
                match result {
                    Ok(info) if self.channels.is_current(&installed) => return Ok(info),
                    Ok(_) => continue,
                    Err(error) => {
                        last_error = error.into();
                        if self.channels.is_current(&installed) && !self.single_address {
                            failed = Some(installed.clone());
                        }
                        if self.single_address && attempt + 1 < MAX_ATTEMPTS {
                            tokio::select! {
                                _ = installed.generation.cancelled() => {}
                                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                            }
                        }
                    }
                }
            }
        }
        Err(last_error.context(format!(
            "Failed to obtain controller server info after {MAX_ATTEMPTS} attempts"
        )))
    }

    /// Reload local identity independently of remote availability. Selecting the
    /// address, validating files, and publishing are serialized with rotation.
    pub async fn reload_client_tls(self: &Arc<Self>) -> anyhow::Result<()> {
        let paths = self
            .client_tls
            .clone()
            .ok_or_else(|| anyhow::anyhow!("conf_sync client TLS reload is not configured (plaintext/skip_tls)"))?;
        let manager = self.channels.clone();
        let use_tls = self.use_tls;
        let timeout = self.timeout;
        tokio::spawn(async move {
            let mut rotator = manager.swap_lock.lock().await;
            let installed = manager.installed.load_full();
            let client = prepare_client(&installed.address, use_tls, timeout, Some(&paths)).await?;
            manager
                .publish_locked(installed.address.clone(), installed.index, client, &mut rotator)
                .await;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|error| anyhow::anyhow!("conf_sync TLS publisher failed: {error}"))?
    }

    /// Get a reference to the `ConfigClient`.
    pub fn get_config_client(&self) -> Arc<ConfigClient> {
        self.config_client.clone()
    }

    /// Get server information including endpoint mode and supported resource kinds
    pub async fn get_server_info(&self) -> Result<ServerInfoResponse, tonic::Status> {
        // Read the current handle through the ArcSwap and clone it out (the
        // service wraps a cheaply-cloneable Channel); do not hold the guard.
        let installed = self.channels.installed.load_full();
        let mut handle = installed.client.clone();
        let info = tokio::select! {
            biased;
            _ = installed.generation.cancelled() => return Err(tonic::Status::cancelled("Controller channel replaced")),
            info = request_server_info(&mut handle, self.timeout) => info?,
        };
        if !self.channels.is_current(&installed) {
            return Err(tonic::Status::cancelled("Controller channel replaced"));
        }

        tracing::info!(
            server_id = %info.server_id,
            supported_kinds = ?info.supported_kinds,
            "Received server info"
        );

        Ok(info)
    }

    /// The leader-session / rotation coordinator (spec §7.2), built on the
    /// `WatchServerMeta` stream — the ONE process-wide long-lived stream, so one
    /// probe is a complete leadership signal (`get_server()` gates all four
    /// RPCs on a follower).
    ///
    /// Per attempt on the current address:
    /// - a follower / dead peer fails (UNAVAILABLE or no connect) → rotate,
    ///   with a 3s dwell first iff this address has previously delivered a meta
    ///   event in this process's lifetime (a controller RELOAD is also
    ///   UNAVAILABLE — the dwell is what keeps a reload from flapping the
    ///   gateway off a healthy leader, spec §7.4 H2; the signal is deliberately
    ///   positive-only, never `status.source()` sniffing);
    /// - a stream that opens but never delivers within `FIRST_MSG_DEADLINE` is
    ///   a wedged peer → same failure handling (this deadline exists ONLY here,
    ///   never on per-kind watch streams, which are legitimately silent —
    ///   spec §7.4);
    /// - a clean stream end WITHOUT a switch event is a demoted leader actively
    ///   closing its streams (the per-term token) → rotate immediately;
    /// - a `switch_event` (server_id change: a reload) and a generation cancel
    ///   (cert reload / rotation elsewhere) reconnect immediately on the
    ///   current channel — never a rotation;
    /// - with a single configured address every rotation is a no-op and the
    ///   loop degrades to exactly the previous same-address reconnect-with-
    ///   backoff behavior (spec §10.5 S5).
    ///
    /// This should be called once at startup via tokio::spawn.
    pub async fn run_leader_session(self: Arc<Self>) {
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);
        // Addresses that have delivered a meta event in this process's lifetime
        // (spec §7.4 H2 — the robust positive reload-vs-follower signal).
        let mut delivered: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Consecutive failures on the current address since the last first-event
        // success or rotation; drives the one-shot dwell.
        let mut failures_on_current: u32 = 0;

        loop {
            tracing::info!("Starting WatchServerMeta stream...");

            // Load the address, handle, and generation token per reconnect
            // iteration so a hot TLS reload or a rotation is picked up on the
            // very next connect.
            let installed = self.channels.installed.load_full();
            let addr = installed.address.clone();
            let gen_token = installed.generation.clone();
            let mut handle = installed.client.clone();
            // A demoted leader actively closes the stream with no switch event
            // (Plan A per-term token); distinguishes "leader gone" from blips.
            let mut demoted_close = false;

            let opened = tokio::select! {
                biased;
                _ = gen_token.cancelled() => continue,
                response = handle.watch_server_meta(WatchServerMetaRequest {
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                }) => response,
            };
            if !self.channels.is_current(&installed) {
                continue;
            }
            match opened {
                Ok(response) => {
                    mark_confsync_connected();
                    let mut stream = response.into_inner();

                    // Track the server_id seen on the first event of this stream session.
                    // If a subsequent event carries a different server_id it is the terminal
                    // server-switch signal from the server side (Stage A); reconnect immediately.
                    let mut stream_server_id: Option<String> = None;
                    let mut server_switched = false;
                    // Set when the TLS reload token cancels this stream; like a server
                    // switch it reconnects immediately (on the new channel), no backoff.
                    let mut reloaded = false;

                    // First-message deadline (spec §7.4): the server pushes an initial
                    // event unconditionally, so an open-but-silent stream is a wedged
                    // peer. Armed only until the first event arrives.
                    let first_deadline = tokio::time::sleep(FIRST_MSG_DEADLINE);
                    tokio::pin!(first_deadline);

                    loop {
                        tokio::select! {
                            biased;
                            _ = gen_token.cancelled() => {
                                tracing::info!(
                                    "WatchServerMeta: channel reinstalled (TLS reload or rotation), reconnecting"
                                );
                                reloaded = true;
                                break;
                            }
                            _ = &mut first_deadline, if stream_server_id.is_none() => {
                                tracing::warn!(
                                    deadline_secs = FIRST_MSG_DEADLINE.as_secs(),
                                    "WatchServerMeta: stream open but no initial event within deadline; \
                                     treating the peer as wedged"
                                );
                                break;
                            }
                            msg = stream.message() => {
                                match msg {
                                    Ok(Some(event)) => {
                                        let mut rotator = self.channels.swap_lock.lock().await;
                                        if !self.channels.is_current(&installed) { reloaded = true; break; }
                                        set_gateway_instance_count(event.gateway_instance_count);

                                        if !event.server_id.is_empty() {
                                            match &stream_server_id {
                                                None => {
                                                    // First event: the positive leader signal.
                                                    // Reset failure accounting for this address.
                                                    backoff = Duration::from_secs(1);
                                                    delivered.insert(addr.clone());
                                                    failures_on_current = 0;
                                                    rotator.note_success();
                                                    stream_server_id = Some(event.server_id.clone());
                                                    self.config_client.set_current_server_id(event.server_id);
                                                }
                                                Some(sid) if *sid != event.server_id => {
                                                    // server_id changed mid-stream — server has reloaded.
                                                    // Update config client with new server_id and reconnect
                                                    // immediately on the SAME address (spec §7.4 H2).
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
                                        // A clean end after delivery, with no switch event, is an
                                        // ACTIVE server-side close: a demoted leader cancelling its
                                        // per-term token, or a controller shutting down gracefully —
                                        // both mean "stop following this peer", so rotate now.
                                        // Assumption this rests on: transport-level teardowns
                                        // (connection reset, trust-root revocation via
                                        // TrustScopedStream, a blackhole) surface as `Err`, never as
                                        // a clean `Ok(None)` — the server sets no max_connection_age
                                        // on this channel, so there is no periodic clean close that
                                        // could trip this arm on a healthy leader.
                                        demoted_close = stream_server_id.is_some();
                                        tracing::warn!(
                                            demoted_close,
                                            "WatchServerMeta stream ended, reconnecting..."
                                        );
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
                        // Intentional switch/reload — skip backoff and reconnect
                        // immediately. Deliberately does NOT mark the link down:
                        // these reconnect on the next channel without a gap worth
                        // paging about.
                        continue;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "WatchServerMeta connection failed, retrying..."
                    );
                }
            }

            // Reached only on an unintentional disconnect (connect failed, an
            // established stream ended/errored, the first-message deadline, or a
            // demoted leader's active close). The gateway keeps serving its
            // last-known config; this mark is what makes that visible, and it clears
            // on the next successful connect above. Scope note: this tracks THIS
            // stream, not overall sync health — see `reasons::CONFSYNC_DISCONNECTED`.
            if !self.channels.is_current(&installed) {
                continue;
            }
            mark_confsync_disconnected();

            if self.single_address {
                // Single-address deployment (K8s): rotation cannot help; keep the
                // previous same-address reconnect-with-backoff behavior verbatim.
                tokio::select! {
                    _ = gen_token.cancelled() => continue,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }

            failures_on_current += 1;
            match rotate_decision(delivered.contains(&addr), failures_on_current, demoted_close) {
                RotateDecision::Dwell => {
                    // Reload-vs-follower protection (spec §7.4 H2): give a healthy
                    // ex-deliverer one dwell to come back before rotating away.
                    tokio::select! {
                        _ = gen_token.cancelled() => continue,
                        _ = tokio::time::sleep(DWELL) => {}
                    }
                }
                RotateDecision::RotateNow => {
                    // The dwell is paid at most ONCE per delivery: rotating away
                    // clears the mark, so a dead ex-leader is fail-fast on every
                    // revisit until it actually delivers again. Without this, a
                    // killed ex-leader costs (1 failed attempt + 3s dwell + 1
                    // failed attempt) on EVERY lap, and the coordinator ping-pongs
                    // long enough to miss the new leader's ready window (observed
                    // in the e2e failover test).
                    //
                    // INVARIANT this depends on: a controller RELOAD reaches this
                    // coordinator as a `switch_event` (same-address reconnect path
                    // above — no failure, no dwell, no rotation), NEVER as a
                    // connection-level failure: the gRPC listener outlives the
                    // CSS, and the server-side meta task waits out the rebuild
                    // window before sending the switch event. If a future change
                    // ever restarts the LISTENER on reload, the meta stream would
                    // Err instead, and this `remove` would turn the ~16-22s
                    // etcd-mode reload into a dwell-less address flap.
                    delivered.remove(&addr);
                    loop {
                        match self.recover_step(installed.clone()).await {
                            Ok(RecoveryOutcome::Installed(_) | RecoveryOutcome::Superseded) => break,
                            Ok(RecoveryOutcome::Retry { error, local }) => {
                                tracing::warn!(component = "grpc_client", error = %error, "conf_sync candidate unavailable");
                                if local {
                                    tokio::select! {
                                        _ = gen_token.cancelled() => break,
                                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::error!(component = "grpc_client", error = %error, "conf_sync channel publication failed");
                                tokio::select! {
                                    _ = gen_token.cancelled() => break,
                                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                                }
                            }
                        }
                    }
                    failures_on_current = 0;
                }
            }
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
            let Some(kind) = ResourceKind::from_kind_name(kind_name) else {
                // Truly unknown kind string — forward compat with newer server
                tracing::warn!(kind = %kind_name, "Unknown resource kind from server, skipping");
                continue;
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

/// The endpoint timeout bounds response headers; this timeout also bounds the
/// entire unary response body. A serving probe is an observation, not a lease.
async fn request_server_info(
    client: &mut ConfigSyncClientService<Channel>,
    timeout: Duration,
) -> Result<ServerInfoResponse, tonic::Status> {
    tokio::time::timeout(timeout, client.get_server_info(ServerInfoRequest {}))
        .await
        .map_err(|_| tonic::Status::deadline_exceeded("Controller server-info probe timed out"))?
        .map(|response| response.into_inner())
}

async fn prepare_client(
    address: &str,
    use_tls: bool,
    timeout: Duration,
    paths: Option<&CertPaths>,
) -> anyhow::Result<ConfigSyncClientService<Channel>> {
    let tls = match paths {
        Some(paths) => {
            let (cert, key, ca) = super::cert_watch::read_and_validate(paths).await?;
            Some(
                ClientTlsConfig::new()
                    .ca_certificate(tonic::transport::Certificate::from_pem(ca))
                    .identity(tonic::transport::Identity::from_pem(cert, key)),
            )
        }
        None => None,
    };
    Ok(configured_client(build_channel(
        &build_grpc_uri(address, use_tls),
        timeout,
        tls,
    )?))
}

/// Every initial, recovery and TLS-reload handle uses the same payload budget.
fn configured_client(channel: Channel) -> ConfigSyncClientService<Channel> {
    ConfigSyncClientService::new(channel).max_decoding_message_size(MAX_MESSAGE_BYTES)
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
#[path = "../../../tests/unit/conf_sync/conf_client/grpc_client_test.rs"]
mod tests;
