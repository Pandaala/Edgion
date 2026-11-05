//! Federation client: connects to edgion-center and maintains a persistent gRPC stream.
//!
//! Lifecycle:
//! 1. Connect to center address
//! 2. Open bidirectional stream (Sync RPC)
//! 3. Send RegisterRequest as first message
//! 4. Loop: handle CenterMessage (Ping, RegisterAck, HttpProxyRequest, WatchRequest)
//! 5. On error: exponential backoff, reconnect

mod center_watch;
mod proxy;
use center_watch::WatchSession;

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio_stream::StreamExt;
use tokio_util::sync::{CancellationToken, DropGuard};
use tonic::transport::Channel;

use crate::authz::{Authorizer, Verb};
use crate::conf_mgr::ConfMgr;
use crate::config::CenterClientConfig;
use crate::fed_sync::proto::{
    center_message::Payload as CenterPayload, controller_message::Payload as CtrlPayload,
    federation_sync_client::FederationSyncClient, CenterMessage, ControllerMessage, FedWatchEventResponse,
    FedWatchListResponse, Pong, RegisterRequest, StatsReport,
};
use edgion_common::observe::fed_metrics;

/// Minimum connection duration before backoff is reset.
/// Connections shorter than this are considered unstable (e.g., Center crash-loop).
const MIN_STABLE_CONNECTION_SECS: u64 = 30;

/// Maximum concurrent HttpProxy tasks forwarded from Center.
const MAX_CONCURRENT_PROXY: usize = 16;

/// Maximum request-body size accepted from a Center HttpProxy request.
/// Mirrors the 10 MB response cap with a tighter inbound limit so an
/// adversarial Center cannot force the Controller to buffer a huge body
/// before any handler runs.
const MAX_PROXY_REQUEST_BODY: usize = 1024 * 1024;

/// Request headers forwarded from a Center HttpProxy request into the admin
/// router. Everything else (hop-by-hop, `Authorization`, `Cookie`,
/// `X-Forwarded-*`, ...) is dropped: the Center identity is established by mTLS
/// plus the injected RBAC role, never by a Center-supplied request header.
/// `if-match` is forwarded because it carries the CAS precondition for
/// Center-issued writes; DELETE has no request-body fallback for it.
const ALLOWED_PROXY_HEADERS: &[&str] = &["accept", "content-type", "if-match", "user-agent"];

/// Path prefixes the federation proxy may reach on the admin router. Anything
/// outside this set is rejected with 404 before it touches the router, so a
/// compromised channel cannot probe arbitrary admin paths.
const ALLOWED_PROXY_PATH_PREFIXES: &[&str] = &["/api/v1/", "/health", "/ready", "/metrics"];

/// Stats coalescing interval. Controller pushes a StatsReport at most once
/// per `STATS_COALESCE_INTERVAL` when its resource counts change.
const STATS_COALESCE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
enum StatsCollectionMode {
    Skip,
    Snapshot,
    DirtyOnly,
}

fn stats_collection_mode(
    initial: bool,
    ready: bool,
    previous_server_id: Option<&str>,
    current_server_id: Option<&str>,
) -> StatsCollectionMode {
    if !ready || current_server_id.is_none() {
        return StatsCollectionMode::Skip;
    }
    if initial || previous_server_id != current_server_id {
        StatsCollectionMode::Snapshot
    } else {
        StatsCollectionMode::DirtyOnly
    }
}

/// Exponential backoff state for reconnection attempts.
pub struct ReconnectBackoff {
    current: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconnectBackoff {
    pub fn new() -> Self {
        Self { current: 1 }
    }

    pub fn current_secs(&self) -> u64 {
        self.current
    }

    /// Advance to next backoff value (doubles, capped at 60s).
    pub fn next(&mut self) {
        self.current = (self.current * 2).min(60);
    }

    pub fn reset(&mut self) {
        self.current = 1;
    }
}

/// Return the DER bytes of the first certificate in a PEM bundle.
fn first_cert_der_from_pem(pem_bytes: &[u8]) -> Option<Vec<u8>> {
    use x509_parser::pem::Pem;
    let mut it = Pem::iter_from_buffer(pem_bytes);
    it.next().and_then(|r| r.ok()).map(|p| p.contents)
}

/// Map a federation mTLS refusal to its `system_error_state` reason label.
fn refusal_reason(r: edgion_common::conf_sync::config::MtlsRefusal) -> &'static str {
    use edgion_common::conf_sync::config::MtlsRefusal;
    use edgion_common::observe::system_metrics::reasons;
    match r {
        MtlsRefusal::NoTls => reasons::NO_TLS_CONFIGURED,
        MtlsRefusal::SkipTls => reasons::SKIP_TLS,
    }
}

/// Runs the federation client loop. Should be spawned as a background task.
/// Exits only when `shutdown` is triggered.
pub async fn run(
    config: CenterClientConfig,
    mut shutdown: watch::Receiver<bool>,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
    authorizer: Arc<Authorizer>,
) {
    if let Err(e) = config.security.validate() {
        tracing::error!(
            component = "fed_client",
            error = %e,
            "Invalid federation security config, federation client will not start"
        );
        return;
    }

    // Federation requires mTLS. On a non-mTLS channel (no TLS block, or
    // skip_tls=true) DEGRADE: log + mark a metric + do not start the client.
    // Federation is auxiliary to the data plane, so we never crash here.
    let tls_cfg = match config.security.resolve_mtls_or_refuse() {
        Ok(t) => t,
        Err(refusal) => {
            tracing::error!(
                component = "fed_client",
                ?refusal,
                "Federation requires mTLS; client will not start"
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::FED_SYNC_CLIENT,
                refusal_reason(refusal),
            );
            return;
        }
    };

    // Startup self-check: our own client cert's SPIFFE URI SAN path must match
    // our configured cluster/name. mTLS is guaranteed here (gate above), so this
    // always runs. On mismatch DEGRADE (log + metric + do not federate).
    match std::fs::read(tls_cfg.cert_path()) {
        Ok(pem_bytes) => {
            let ok = first_cert_der_from_pem(&pem_bytes)
                .and_then(|d| crate::fed_sync::spiffe::extract_single_spiffe_uri(&d).ok())
                .and_then(|u| crate::fed_sync::spiffe::parse_controller_spiffe(&u))
                .map(|p| crate::fed_sync::spiffe::path_matches(&p, &config.cluster, &config.name))
                .unwrap_or(false);
            if !ok {
                tracing::error!(
                    component = "fed_client",
                    cluster = %config.cluster,
                    name = %config.name,
                    "Client cert SPIFFE SAN does not match cluster/name; federation client will not start"
                );
                edgion_common::observe::system_metrics::mark_system_error(
                    edgion_common::observe::system_metrics::components::FED_SYNC_CLIENT,
                    edgion_common::observe::system_metrics::reasons::PEER_IDENTITY_SELF_CHECK,
                );
                return;
            }
        }
        Err(e) => {
            tracing::error!(
                component = "fed_client",
                error = %e,
                "Cannot read client cert for self-check; federation client will not start"
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::FED_SYNC_CLIENT,
                edgion_common::observe::system_metrics::reasons::PEER_IDENTITY_SELF_CHECK,
            );
            return;
        }
    }

    // Wait for ConfigSyncServer to be ready before connecting to Center.
    // This prevents the tight NotReady error loop where Center sends
    // FedWatchRequest before the Controller's caches are initialized.
    tracing::info!(
        component = "fed_client",
        "Waiting for ConfigSyncServer to be ready before connecting to Center"
    );
    let ready_wait_start = std::time::Instant::now();
    loop {
        if conf_mgr.is_ready() {
            let elapsed = std::time::Instant::now()
                .saturating_duration_since(ready_wait_start)
                .as_secs_f64();
            fed_metrics::record_ready_gate_wait(elapsed);
            tracing::info!(
                component = "fed_client",
                elapsed_secs = elapsed,
                "ConfigSyncServer is ready, proceeding to connect"
            );
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown during ready-wait, exiting");
                return;
            }
        }
    }

    let mut backoff = ReconnectBackoff::new();

    loop {
        let connect_start = std::time::Instant::now();
        let Some(result) = until_shutdown(
            &mut shutdown,
            connect_and_run(&config, admin_router.clone(), conf_mgr.clone(), authorizer.clone()),
        )
        .await
        else {
            tracing::info!(component = "fed_client", "Shutdown signal received, exiting");
            return;
        };
        match result {
            Ok(()) => {
                // Only reset backoff if connection was alive long enough.
                // Prevents rapid reconnection when Center crash-loops
                // (connects briefly then dies), which would cause repeated
                // TLS handshakes + full resource serialization.
                if std::time::Instant::now().saturating_duration_since(connect_start)
                    > Duration::from_secs(MIN_STABLE_CONNECTION_SECS)
                {
                    backoff.reset();
                }
                tracing::info!(component = "fed_client", "Stream ended cleanly, reconnecting");
            }
            Err(e) => {
                tracing::warn!(
                    component = "fed_client",
                    error = %e,
                    retry_secs = backoff.current_secs(),
                    "Connection failed, will retry"
                );
            }
        }

        let wait = Duration::from_secs(backoff.current_secs());
        backoff.next();

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown during backoff, exiting");
                return;
            }
        }
    }
}

/// Own the connection future so shutdown drops its session guards before returning.
async fn until_shutdown(
    shutdown: &mut watch::Receiver<bool>,
    connection: impl std::future::Future<Output = Result<()>>,
) -> Option<Result<()>> {
    tokio::select! {
        _ = shutdown.changed() => None,
        result = connection => Some(result),
    }
}

async fn connect_and_run(
    config: &CenterClientConfig,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
    authorizer: Arc<Authorizer>,
) -> Result<()> {
    // run()'s resolve_mtls_or_refuse gate guarantees mTLS before we get here.
    let tls_cfg = config
        .security
        .resolve_mtls_or_refuse()
        .expect("run() gate guarantees mTLS before connect_and_run");

    // Strip any scheme; federation is always https.
    let addr = config
        .address
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let uri = format!("https://{}", addr);

    let endpoint = Channel::from_shared(uri)?;
    let (client_tls, _) = edgion_common::grpc_tls::load_client_tls(tls_cfg).await?;
    let endpoint = endpoint
        .tls_config(client_tls)
        .map_err(|e| anyhow::anyhow!("Failed to configure federation client TLS: {}", e))?;

    // Cap TCP connection establishment; without this the OS default (~75-130 s on Linux)
    // blocks the reconnect loop on NAT black-holes. Mirrors conf_sync grpc_client.rs.
    let channel = endpoint.connect_timeout(Duration::from_secs(10)).connect().await?;

    let mut client = FederationSyncClient::new(channel)
        .max_decoding_message_size(proxy::MAX_MESSAGE_BYTES)
        .max_encoding_message_size(proxy::MAX_MESSAGE_BYTES);

    let (tx, rx) = tokio::sync::mpsc::channel::<ControllerMessage>(32);
    let tx = Arc::new(tx);

    // Send RegisterRequest as first message
    let register = ControllerMessage {
        payload: Some(CtrlPayload::Register(RegisterRequest {
            controller_id: config.controller_id(),
            cluster: config.cluster.clone(),
            env: config.env.clone(),
            tag: config.tag.clone(),
        })),
    };
    tx.send(register).await?;

    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let inbound = client.sync(outbound).await?.into_inner();

    fed_metrics::record_connection_event(
        fed_metrics::labels::role::CONTROLLER,
        fed_metrics::labels::event::CONNECTED,
    );
    fed_metrics::set_connections_active(fed_metrics::labels::role::CONTROLLER, 1);
    let session_started_at = std::time::Instant::now();

    tracing::info!(
        component = "fed_client",
        controller_id = %config.controller_id(),
        "Connected and registered with center"
    );

    run_session(inbound, tx, admin_router, conf_mgr, authorizer, session_started_at).await
}

/// Established-session state is owned by this future, including cancellation on drop.
async fn run_session(
    mut inbound: impl tokio_stream::Stream<Item = std::result::Result<CenterMessage, tonic::Status>> + Unpin,
    tx: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
    authorizer: Arc<Authorizer>,
    session_started_at: std::time::Instant,
) -> Result<()> {
    // Spawn stats coalescing task: every STATS_COALESCE_INTERVAL, ask the
    // processor registry whether any kind has changed since last tick. If so,
    // push a StatsReport over the same fed_sync stream. Cancelled at session
    // teardown; descendants release asynchronously without delaying shutdown.
    let stats_cancel = CancellationToken::new();
    let _stats_guard = stats_cancel.clone().drop_guard();
    {
        let conf_mgr = conf_mgr.clone();
        let tx = tx.clone();
        let cancel = stats_cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(STATS_COALESCE_INTERVAL);
            // First tick fires immediately. Force a full snapshot for every
            // federation session: a restarted Center has an empty aggregator
            // even when this long-running Controller has no newly dirty resources.
            let mut initial = true;
            let mut last_server_id: Option<String> = None;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Do not publish transient empty/partial counts while a
                        // reload is rebuilding the processor registry. A new
                        // ConfigSyncServer ID marks reload completion and forces
                        // a full snapshot on the existing federation stream.
                        let current_css = conf_mgr.config_sync_server();
                        let current_server_id = current_css.as_ref().map(|server| server.server_id());
                        let mode = stats_collection_mode(
                            initial,
                            conf_mgr.is_ready(),
                            last_server_id.as_deref(),
                            current_server_id.as_deref(),
                        );
                        let report = match mode {
                            StatsCollectionMode::Skip => None,
                            StatsCollectionMode::Snapshot => Some(conf_mgr.collect_stats_snapshot()),
                            StatsCollectionMode::DirtyOnly => conf_mgr.collect_stats_if_dirty(),
                        };
                        // Reload can start after the pre-check but before (or
                        // during) collection. Only publish and commit the
                        // generation after verifying that the exact CSS Arc is
                        // still current and the rebuilt registry remains ready.
                        let report = report.filter(|_| {
                            conf_mgr.is_ready()
                                && current_css.as_ref().is_some_and(|observed| {
                                    conf_mgr
                                        .config_sync_server()
                                        .as_ref()
                                        .is_some_and(|current| Arc::ptr_eq(observed, current))
                                })
                        });
                        if report.is_some() {
                            initial = false;
                            last_server_id = current_server_id;
                        }
                        if let Some((per_kind, total)) = report {
                            let msg = ControllerMessage {
                                payload: Some(CtrlPayload::StatsReport(StatsReport { per_kind, total })),
                            };
                            // Best effort: if the channel is closed (session
                            // ending), drop the report -- the next session will
                            // emit a fresh one with stats_dirty=true.
                            tokio::select! {
                                _ = cancel.cancelled() => break,
                                result = tx.send(msg) => {
                                    if result.is_err() { break; }
                                }
                            }
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // Owns cancellation on drop for the active watch forwarding task, if any.
    // A new WatchRequest cancels the previous one before starting a new task.
    let mut watch_session = WatchSession::default();
    let session_cancel = watch_session.cancelled.clone();

    // Limit concurrent HttpProxy tasks to prevent Center from overwhelming Controller.
    let proxy_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PROXY));

    let result: Result<()> = loop {
        tokio::select! {
            _ = session_cancel.cancelled() => break Err(anyhow::anyhow!("Center watch delivery failed")),
            msg = inbound.next() => {
                match msg {
                    Some(Err(e)) => break Err(e.into()),
                    None => {
                        tracing::info!(component = "fed_client", "Center closed stream");
                        break Ok(());
                    }
                    Some(Ok(CenterMessage { payload: Some(payload) })) => {
                        let handled = tokio::select! {
                            biased;
                            _ = session_cancel.cancelled() => break Err(anyhow::anyhow!("Center watch delivery failed")),
                            result = handle_center_message(
                                payload, tx.clone(), admin_router.clone(), conf_mgr.clone(),
                                &mut watch_session, proxy_semaphore.clone(), authorizer.clone(),
                            ) => result,
                        };
                        if let Err(error) = handled { break Err(error); }
                    }
                    Some(Ok(_)) => {} // empty payload, ignore
                }
            }
        }
    };

    // Cleanup: cancel any active watch forwarding task on disconnect.
    // Without this, old forwarding tasks linger until the gRPC stream's
    // internal channels detect the disconnect asynchronously.
    drop(watch_session);
    // Stop the stats coalescing task tied to this session. A new session
    // will start a fresh one, and `stats_dirty=true` initial state ensures
    // the next register emits an up-to-date report.
    stats_cancel.cancel();

    // Record session end metrics. This runs whether the loop broke with
    // Ok(()) (clean close) or Err(..) (stream error); both count as a
    // disconnect event on the Controller side.
    fed_metrics::record_connection_event(
        fed_metrics::labels::role::CONTROLLER,
        fed_metrics::labels::event::DISCONNECTED,
    );
    fed_metrics::record_connection_duration(
        fed_metrics::labels::role::CONTROLLER,
        std::time::Instant::now()
            .saturating_duration_since(session_started_at)
            .as_secs_f64(),
    );
    fed_metrics::set_connections_active(fed_metrics::labels::role::CONTROLLER, 0);

    result
}

async fn handle_center_message(
    payload: CenterPayload,
    tx: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
    watch_session: &mut WatchSession,
    proxy_semaphore: Arc<Semaphore>,
    authorizer: Arc<Authorizer>,
) -> Result<()> {
    match payload {
        CenterPayload::RegisterAck(ack) => {
            tracing::info!(
                component = "fed_client",
                session_id = %ack.session_id,
                "Registered with center"
            );
        }

        CenterPayload::Ping(ping) => {
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::Pong(Pong {
                    timestamp: ping.timestamp,
                })),
            })
            .await?;
        }

        CenterPayload::HttpProxy(proxy_req) => {
            let deadline = tokio::time::Instant::now() + proxy::PROXY_TIMEOUT;
            let Ok(permit) = proxy_semaphore.clone().try_acquire_owned() else {
                let response = proxy::error(proxy_req.request_id, 429, "Proxy capacity exhausted");
                proxy::send(&tx, response, deadline).await;
                return Ok(());
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let request_id = proxy_req.request_id.clone();
                let response = match tokio::time::timeout_at(deadline, proxy::execute(admin_router, proxy_req)).await {
                    Ok(response) => response,
                    Err(_) => proxy::error(
                        request_id,
                        504,
                        "Proxy deadline exceeded; mutation outcome may be unknown",
                    ),
                };
                proxy::send(&tx, response, deadline).await;
            });
        }

        CenterPayload::WatchRequest(req) => {
            watch_session.start(req, tx, conf_mgr, authorizer);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/fed_client/mod_test.rs"]
mod tests;
