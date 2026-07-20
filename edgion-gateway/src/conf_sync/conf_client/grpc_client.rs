use crate::conf_sync::conf_client::cert_watch::CertPaths;
use crate::conf_sync::conf_client::{ConfigClient, EndpointRotator};
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
    /// Cert file paths for rebuilding the channel on cert rotation. `None` when
    /// the channel is plaintext / `skip_tls` (nothing to reload). The address
    /// deliberately does NOT live here (spec §7.4 H1: a cert reload must rebuild
    /// on the CURRENT address, not the boot-time one).
    client_tls: Option<CertPaths>,
    /// The address the installed channel points at. Written only by
    /// `install_channel` (under `swap_lock`).
    current_addr: ArcSwap<String>,
    /// Rotates through `server_addrs` to find the leader (spec §7.2). A
    /// one-element list never advances — the K8s zero-impact guarantee.
    rotator: Arc<EndpointRotator>,
    /// Whether the channel uses TLS (fixed by the security config at startup).
    use_tls: bool,
    /// Per-request/connect timeout captured for channel rebuilds.
    timeout: Duration,
    /// Serializes channel swaps: cert reload and rotation are two writers to the
    /// same channel slots and must not interleave (spec §7.4 H3).
    swap_lock: tokio::sync::Mutex<()>,
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
        let config_client = Arc::new(ConfigClient::new(client_id.clone(), client_name.clone()));

        // Shuffled per gateway (spec §7.5); the initial channel points at the
        // rotator's current address.
        let rotator = Arc::new(EndpointRotator::new(server_addrs.clone()));
        let initial_addr = rotator.current();

        let tls_config = security.resolve_active_tls();
        let use_tls = tls_config.is_some_and(|t| !t.skip_tls);
        let uri = build_grpc_uri(&initial_addr, use_tls);

        tracing::info!(
            server_addrs = ?server_addrs,
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
            current_addr: ArcSwap::from_pointee(initial_addr),
            rotator,
            use_tls,
            timeout,
            swap_lock: tokio::sync::Mutex::new(()),
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

    /// Build and install a channel for `addr` — the ONE channel-swap primitive
    /// shared by cert reload, rotation, and bootstrap (spec §7.2).
    ///
    /// Rebuilds TLS from the configured cert paths (validated on every call) or
    /// plaintext when none are configured, then installs in the fixed order:
    /// (1) `set_all_grpc_clients` stores the new channel in every per-kind
    /// cache's client slot; (2) store the new handle for `get_server_info` /
    /// the meta stream; (3) `rotate_generation` LAST — and ONLY when the address
    /// actually changed or the caller forces it (a cert reload must recycle
    /// streams on the same address; a same-address rotation no-op must NOT
    /// disturb the 18 per-kind loops — the K8s zero-impact rule, spec §10.5 S6).
    /// The whole body holds `swap_lock` so the two writers (cert reload and
    /// rotation) cannot interleave (spec §7.4 H3). On any error the current
    /// channel is left untouched.
    pub(crate) async fn install_channel(&self, addr: &str, force_generation: bool) -> anyhow::Result<()> {
        let _guard = self.swap_lock.lock().await;
        let addr_changed = addr != self.current_addr.load().as_str();

        let tls_opt = match &self.client_tls {
            Some(paths) => {
                let (cert, key, ca) = crate::conf_sync::conf_client::cert_watch::read_and_validate(paths).await?;
                Some(
                    ClientTlsConfig::new()
                        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
                        .identity(tonic::transport::Identity::from_pem(cert, key)),
                )
            }
            // Plaintext / skip_tls: rotation must still work (spec §7.2) — most
            // non-K8s dev/test deployments run conf_sync in plaintext.
            None => None,
        };
        let uri = build_grpc_uri(addr, self.use_tls);
        let channel = build_channel(&uri, self.timeout, tls_opt)?;
        let new_client = ConfigSyncClientService::new(channel);

        // (1) Point every per-kind cache at the new channel BEFORE cancelling, so
        // a loop that breaks re-clones the new client, never the retired one.
        self.config_client.set_all_grpc_clients(new_client.clone()).await;
        // (2) Install the new handle for get_server_info / WatchServerMeta.
        self.conf_client_handle.store(Arc::new(new_client));
        self.current_addr.store(Arc::new(addr.to_string()));
        // (3) Cancel the old generation LAST → running loops break and reconnect.
        if force_generation || addr_changed {
            self.rotate_generation();
        }
        Ok(())
    }

    /// Advance the rotator and re-point the channel at the next address
    /// (spec §7.2). Always records the failure (for lap/backoff accounting);
    /// with a single configured address the channel is never touched — the K8s
    /// deployment cannot rotate anywhere (spec §10.5 S5).
    pub(crate) async fn rotate(&self) -> anyhow::Result<()> {
        let next = self.rotator.advance();
        if self.rotator.is_single() {
            return Ok(());
        }
        tracing::info!(component = "grpc_client", next_addr = %next, "rotating conf_sync channel");
        self.install_channel(&next, false).await
    }

    /// The rotator (bootstrap and the meta-stream coordinator share it).
    pub(crate) fn rotator(&self) -> Arc<EndpointRotator> {
        self.rotator.clone()
    }

    /// The address the installed channel currently points at (logging/bootstrap).
    pub(crate) fn current_addr(&self) -> String {
        self.current_addr.load_full().as_ref().clone()
    }

    /// Hot-reload the client TLS identity from disk (spec §6 T4): rebuild on the
    /// CURRENT address (spec §7.4 H1 — never the boot-time one) with a forced
    /// generation bump so running streams recycle onto the new TLS identity.
    pub async fn reload_client_tls(self: &Arc<Self>) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.client_tls.is_some(),
            "conf_sync client TLS reload is not configured (plaintext/skip_tls)"
        );
        let addr = self.current_addr.load_full();
        self.install_channel(&addr, true).await
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
            let addr = self.current_addr.load_full().as_ref().clone();
            let gen_token = self.reload_generation.load_full();
            let mut handle = self.conf_client_handle.load().as_ref().clone();
            // A demoted leader actively closes the stream with no switch event
            // (Plan A per-term token); distinguishes "leader gone" from blips.
            let mut demoted_close = false;

            match handle
                .watch_server_meta(WatchServerMetaRequest {
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                })
                .await
            {
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
                                        set_gateway_instance_count(event.gateway_instance_count);

                                        if !event.server_id.is_empty() {
                                            match &stream_server_id {
                                                None => {
                                                    // First event: the positive leader signal.
                                                    // Reset failure accounting for this address.
                                                    backoff = Duration::from_secs(1);
                                                    delivered.insert(addr.clone());
                                                    failures_on_current = 0;
                                                    self.rotator.note_success();
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
            mark_confsync_disconnected();

            if self.rotator.is_single() {
                // Single-address deployment (K8s): rotation cannot help; keep the
                // previous same-address reconnect-with-backoff behavior verbatim.
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }

            failures_on_current += 1;
            match rotate_decision(delivered.contains(&addr), failures_on_current, demoted_close) {
                RotateDecision::Dwell => {
                    // Reload-vs-follower protection (spec §7.4 H2): give a healthy
                    // ex-deliverer one dwell to come back before rotating away.
                    tokio::time::sleep(DWELL).await;
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
                    if let Err(e) = self.rotate().await {
                        tracing::error!(error = %e, "conf_sync rotation failed; retrying current address");
                    }
                    failures_on_current = 0;
                    if self.rotator.completed_full_lap() {
                        // Every address failed since the last success: jittered
                        // backoff so a fleet does not hammer in lockstep (spec §7.5).
                        self.rotator.backoff_after_lap().await;
                    }
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

    /// An explicit `skip_tls` section — the only supported plaintext config, and the
    /// cheapest way to build a client without touching cert files.
    fn skip_tls_security() -> ConfSyncSecurityConfig {
        ConfSyncSecurityConfig {
            tls: Some(ConfSyncTlsConfig {
                name: None,
                cert: String::new(),
                key: String::new(),
                ca: String::new(),
                skip_tls: true,
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn reload_cancels_previous_generation() {
        // Uses a plaintext (skip_tls) client against no server: connect_lazy never dials.
        let sec = skip_tls_security();
        let client = ConfigSyncClient::new(vec!["127.0.0.1:1".into()], "t".into(), Duration::from_secs(1), &sec)
            .await
            .unwrap();
        let client = std::sync::Arc::new(client);
        let old_gen = client.reload_generation.load_full();
        assert!(!old_gen.is_cancelled());
        client.rotate_generation(); // the token-swap half of reload_client_tls, factored out
        assert!(old_gen.is_cancelled());
        assert!(!client.reload_generation.load_full().is_cancelled());
    }

    #[test]
    fn rotation_decision_matrix() {
        use super::{rotate_decision, RotateDecision};
        // never delivered ⇒ rotate immediately (bootstrap / kill -9 at startup).
        assert_eq!(rotate_decision(false, 1, false), RotateDecision::RotateNow);
        // delivered before, first failure ⇒ dwell first (reload flap protection, spec §7.4 H2).
        assert_eq!(rotate_decision(true, 1, false), RotateDecision::Dwell);
        // delivered before, dwell already spent ⇒ rotate.
        assert_eq!(rotate_decision(true, 2, false), RotateDecision::RotateNow);
        // demoted leader actively closed its stream ⇒ rotate regardless of history.
        assert_eq!(rotate_decision(true, 1, true), RotateDecision::RotateNow);
    }

    #[tokio::test]
    async fn install_channel_generation_guard() {
        // Spec §10.5 S6: a same-address install must NOT bump the generation (the
        // 18 per-kind loops stay undisturbed — the K8s no-op); force=true (cert
        // reload) and a changed address must bump it.
        let sec = skip_tls_security();
        let client = ConfigSyncClient::new(
            vec!["127.0.0.1:1".into(), "127.0.0.1:2".into()],
            "t".into(),
            Duration::from_secs(1),
            &sec,
        )
        .await
        .unwrap();
        let current = client.current_addr.load_full().as_ref().clone();

        let g0 = client.reload_generation.load_full();
        client.install_channel(&current, false).await.unwrap();
        assert!(
            !g0.is_cancelled(),
            "same address without force must not bump the generation"
        );

        client.install_channel(&current, true).await.unwrap();
        assert!(g0.is_cancelled(), "force=true (cert reload) must bump the generation");

        let g1 = client.reload_generation.load_full();
        let other = if current == "127.0.0.1:1" {
            "127.0.0.1:2"
        } else {
            "127.0.0.1:1"
        };
        client.install_channel(other, false).await.unwrap();
        assert!(g1.is_cancelled(), "a changed address must bump the generation");
        assert_eq!(client.current_addr.load_full().as_str(), other);
    }

    #[tokio::test]
    async fn rotate_is_noop_for_single_address() {
        // Spec §10.5 S5: with one configured address the rotator never advances
        // and rotate() must not touch the channel or the generation.
        let sec = skip_tls_security();
        let client = ConfigSyncClient::new(vec!["127.0.0.1:1".into()], "t".into(), Duration::from_secs(1), &sec)
            .await
            .unwrap();
        let g0 = client.reload_generation.load_full();
        client.rotate().await.unwrap();
        assert!(!g0.is_cancelled(), "single-address rotate must be a generation no-op");
        assert_eq!(client.current_addr.load_full().as_str(), "127.0.0.1:1");
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
        let Err(err) = ConfigSyncClient::new(
            vec![DUMMY_ADDR.to_string()],
            "test".to_string(),
            Duration::from_secs(1),
            &security,
        )
        .await
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
    async fn omitted_security_fails_closed_instead_of_plaintext() {
        // Fail-close (2026-07-14): an omitted `conf_sync_security` no longer connects in
        // plaintext — it refuses to start. `skip_tls: true` is the only plaintext path.
        let security = ConfSyncSecurityConfig::default();
        let Err(err) = ConfigSyncClient::new(
            vec![DUMMY_ADDR.to_string()],
            "test".to_string(),
            Duration::from_secs(1),
            &security,
        )
        .await
        else {
            panic!("omitted conf_sync_security must fail closed, not connect in plaintext");
        };
        let msg = format!("{err:?}");
        for needle in ["skip_tls", "auto_ca", "Secrets"] {
            assert!(msg.contains(needle), "error must mention {needle:?}, got: {msg}");
        }
    }

    #[tokio::test]
    async fn skip_tls_without_auto_ca_still_connects_plaintext() {
        // The explicit escape hatch is untouched: skip_tls still yields a usable client.
        let security = skip_tls_security();
        let client = ConfigSyncClient::new(
            vec![DUMMY_ADDR.to_string()],
            "test".to_string(),
            Duration::from_secs(1),
            &security,
        )
        .await;
        assert!(
            client.is_ok(),
            "skip_tls must remain the explicit plaintext escape hatch"
        );
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
        let client = ConfigSyncClient::new(
            vec![DUMMY_ADDR.to_string()],
            "test".to_string(),
            Duration::from_secs(1),
            &security,
        )
        .await;
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
