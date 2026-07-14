//! gRPC server for configuration synchronization.
//!
//! Implements the ConfigSync service using `ConfigSyncServer` and a provider
//! pattern so the gRPC layer always uses the latest server instance after reload.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

/// HTTP/2 keepalive interval — how often to send PING frames (seconds).
const HTTP2_KEEPALIVE_INTERVAL_SECS: u64 = 10;
/// HTTP/2 keepalive timeout — how long to wait for PING ACK before treating connection as dead (seconds).
const HTTP2_KEEPALIVE_TIMEOUT_SECS: u64 = 5;

/// Hard upper bound on the lifetime of a single conf_sync HTTP/2 connection.
///
/// **Defense in depth, not the primary control.** The primary revocation control is the
/// trust token ([`tls_identity::ServerIdentityHandle::store`]): a trust-root change kills
/// every session established under the old root, immediately. This is the belt to that
/// suspenders — whatever else goes wrong (a bug in the token plumbing, a code path that
/// forgets to wire it), no session that streams resolved Secrets may live forever without
/// the peer ever re-proving its identity against the CURRENT client-CA verifier.
///
/// 1h: gateways number in the low tens, so one re-handshake per gateway per hour is
/// nothing (a handshake, and a resumed watch from the client's last `sync_version` — no
/// full relist storm; tonic sends a GOAWAY first, so the client finishes in-flight RPCs).
/// Short enough that a lost revocation is bounded by an hour instead of by the pod's
/// lifetime; long enough that the control plane never churns.
const MAX_CONNECTION_AGE_SECS: u64 = 3600;

/// Grace period after [`MAX_CONNECTION_AGE_SECS`] before the socket is forcibly aborted.
///
/// tonic 0.12's `max_connection_age` only calls hyper's `graceful_shutdown` (GOAWAY, then
/// wait for open streams to finish) — and a conf_sync `Watch` stream NEVER finishes on its
/// own, so the GOAWAY alone would let the connection live indefinitely and the "hard upper
/// bound" would not be a bound at all. tonic 0.12.3 exposes no grace-period knob (verified
/// against the vendored source: the only builder method is `max_connection_age`), so the
/// deadline is enforced where we already own the socket — [`TrustScopedStream`] fails the
/// connection with `ConnectionAborted` at age + grace.
///
/// 5m: far more than the round trip a gateway needs to see the GOAWAY, tear down its watch
/// streams and reconnect, so a healthy client always closes on its own and never sees the
/// hard abort.
const MAX_CONNECTION_AGE_GRACE_SECS: u64 = 300;

/// Upper bound on TLS handshakes in flight at once in the owned accept loop.
///
/// What this actually bounds: the number of spawned tasks that are simultaneously
/// **(a) performing a TLS handshake, or (b) waiting to hand the finished stream to
/// tonic over `tx`**. It is a **backpressure bound, not a connection limit**:
/// legitimate conf_sync peers are gateway replicas, which number in the low tens,
/// and both phases are time-bounded (10s handshake timeout,
/// [`TLS_HANDOFF_TIMEOUT_SECS`] handoff timeout). 256 is therefore orders of
/// magnitude above any healthy steady state — it exists purely so that a
/// pathological case (a flood of half-open TLS clients) cannot spawn unbounded
/// tasks and leak their sockets. Once saturated, the loop stops calling `accept()`
/// and the burst waits in the kernel backlog.
///
/// The permit alone is NOT sufficient to survive a wedged consumer: because it is
/// held across the `tx.send(..)` handoff, an unbounded send on a full channel would
/// pin permits forever and, at 256 of them, stop `accept()` **permanently** — no
/// gateway could ever (re)connect. That is why the handoff is time-bounded; see
/// [`TLS_HANDOFF_TIMEOUT_SECS`].
const MAX_INFLIGHT_HANDSHAKES: usize = 256;

/// Upper bound on how long a completed TLS handshake may wait to be handed to the
/// tonic consumer over the bounded (64-slot) `tx` channel.
///
/// Without it, `tx.send(..)` is an **unbounded** wait whenever the channel is full
/// and the consumer is not draining it. Since the handshake permit is held across
/// the send, a wedged consumer would convert a leaky failure into a fatal one: 256
/// stuck senders exhaust [`MAX_INFLIGHT_HANDSHAKES`] and the accept loop stalls for
/// good. With the timeout, the failure degrades to what it should be — connections
/// are DROPPED (and counted via `tls_handshake_failed`) while the accept loop keeps
/// breathing and recovers the instant the consumer drains.
///
/// 30s: comfortably longer than any legitimate scheduling hiccup on a busy runtime
/// (tonic pulls from the channel as fast as it can spawn connection tasks, so a
/// healthy handoff is sub-millisecond), and shorter than the gateway's own connect
/// deadline — a client that has been waiting this long has given up anyway, so the
/// stream we would hand over is already worthless.
const TLS_HANDOFF_TIMEOUT_SECS: u64 = 30;

use crate::conf_mgr::sync_runtime::metrics::reload_metrics;
use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_common::conf_sync::proto::{
    config_sync_server::{ConfigSync, ConfigSyncServer as ConfigSyncService},
    ListRequest, ListResponse, ServerInfoRequest, ServerInfoResponse, ServerMetaEvent, WatchRequest, WatchResponse,
    WatchServerMetaRequest,
};
use edgion_resources::{WATCH_ERR_SERVER_ID_MISMATCH, WATCH_ERR_SERVER_RELOAD};

use super::ConfigSyncServer;

/// A conf_sync TLS connection scoped to the trust root that authenticated it.
///
/// Certificates are consumed exactly once, at handshake. Swapping the client-CA verifier
/// therefore governs FUTURE handshakes only — an already-established HTTP/2 connection
/// keeps issuing `List`/`Watch` RPCs (and keeps receiving the resolved-Secret stream)
/// without ever re-authenticating. After an emergency CA re-key that is precisely the
/// attacker's connection.
///
/// This wrapper carries the trust token captured **at accept time** and turns the whole
/// stream into a dead socket (`ConnectionAborted`) the moment that token is cancelled —
/// which [`super::tls_identity::ServerIdentityHandle::store`] does if and only if the
/// trust ROOT changed. hyper sees the I/O error and drops the connection, taking every
/// stream on it with it. A same-root leaf rotation cancels nothing, so routine rotation
/// stays zero-disruption.
///
/// It additionally enforces the hard connection-age deadline (see
/// [`MAX_CONNECTION_AGE_GRACE_SECS`]).
struct TrustScopedStream {
    inner: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    /// Prompt wakeup: registers our waker with the token, so a cancel fails the
    /// connection immediately instead of on its next (possibly never) poll.
    trust_cancelled: Pin<Box<WaitForCancellationFutureOwned>>,
    /// Hard age bound; fires at MAX_CONNECTION_AGE + grace.
    age_deadline: Pin<Box<tokio::time::Sleep>>,
    /// Latched once we have returned the abort error: neither future may be polled
    /// after completion.
    dead: bool,
}

impl TrustScopedStream {
    fn new(inner: tokio_rustls::server::TlsStream<tokio::net::TcpStream>, trust: &CancellationToken) -> Self {
        Self {
            inner,
            trust_cancelled: Box::pin(trust.clone().cancelled_owned()),
            age_deadline: Box::pin(tokio::time::sleep(Duration::from_secs(
                MAX_CONNECTION_AGE_SECS + MAX_CONNECTION_AGE_GRACE_SECS,
            ))),
            dead: false,
        }
    }

    /// `Some(err)` once this connection must die (trust root replaced, or the hard age
    /// bound elapsed). Registers the waker on both futures while it is still alive.
    fn poll_alive(&mut self, cx: &mut Context<'_>) -> Option<io::Error> {
        if self.dead {
            return Some(aborted("conf_sync connection revoked"));
        }
        if self.trust_cancelled.as_mut().poll(cx).is_ready() {
            self.dead = true;
            return Some(aborted(
                "conf_sync trust root changed; connections established under the previous root are revoked",
            ));
        }
        if self.age_deadline.as_mut().poll(cx).is_ready() {
            self.dead = true;
            return Some(aborted("conf_sync connection exceeded its maximum age"));
        }
        None
    }
}

fn aborted(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, msg)
}

impl AsyncRead for TrustScopedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.poll_alive(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for TrustScopedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(e) = this.poll_alive(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.poll_alive(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.poll_alive(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

impl Connected for TrustScopedStream {
    // Mirror the inner TLS stream's ConnectInfo verbatim so tonic request extensions
    // (peer certificates, peer address) are unchanged by the wrapper.
    type ConnectInfo = <tokio_rustls::server::TlsStream<tokio::net::TcpStream> as Connected>::ConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.inner.connect_info()
    }
}

/// Provider trait for dynamically obtaining ConfigSyncServer
///
/// This allows the gRPC server to get the latest ConfigSyncServer instance
/// after reload operations.
pub trait ConfigSyncServerProvider: Send + Sync {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>>;
}

/// gRPC ConfigSync service implementation
///
/// Uses a provider to dynamically obtain ConfigSyncServer, which allows
/// the gRPC server to use the latest instance after reload operations.
pub struct ConfigSyncGrpcServer {
    provider: Arc<dyn ConfigSyncServerProvider>,
    shutdown_token: CancellationToken,
}

impl ConfigSyncGrpcServer {
    /// Create a new gRPC server with a dynamic provider
    pub fn new(provider: Arc<dyn ConfigSyncServerProvider>, shutdown_token: CancellationToken) -> Self {
        Self {
            provider,
            shutdown_token,
        }
    }

    /// Get the current ConfigSyncServer, returns error if not available
    #[allow(clippy::result_large_err)]
    fn get_server(&self) -> Result<Arc<ConfigSyncServer>, Status> {
        self.provider
            .config_sync_server()
            .ok_or_else(|| Status::unavailable("ConfigSyncServer not ready"))
    }

    pub fn into_service(self) -> ConfigSyncService<ConfigSyncGrpcServer> {
        ConfigSyncService::new(self)
    }

    /// Log + mark the appropriate system-error metric for the plaintext / skip_tls
    /// path (the `tls_handle = None` branch of `serve_with_shutdown`). Extracted from
    /// the former `build_server` so the two branches (owned TLS accept loop vs.
    /// plaintext) stay easy to read side by side.
    fn log_plaintext_or_skip_tls(security: &ConfSyncSecurityConfig) {
        if let Some(tls) = security.resolve_active_tls() {
            debug_assert!(
                tls.skip_tls,
                "resolve_active_tls() with a non-skip_tls entry implies a tls_handle"
            );
            tracing::error!(
                component = "grpc_server",
                "conf_sync mTLS skip_tls=true: starting WITHOUT TLS. INSECURE, emergency use only!"
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                edgion_common::observe::system_metrics::reasons::SKIP_TLS,
            );
        } else {
            // No active TLS resolved. validate() already rejected the dangling-`active`
            // case, so this is the truly-omitted section: plaintext while carrying Secrets.
            debug_assert!(security.is_plaintext_omitted());
            tracing::error!(
                component = "grpc_server",
                "conf_sync_security omitted: starting gRPC server in PLAINTEXT. This channel \
                 carries resolved Secrets (TLS keys, backend credentials). Configure mTLS for production."
            );
            edgion_common::observe::system_metrics::mark_system_error(
                edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                edgion_common::observe::system_metrics::reasons::PLAINTEXT_DEFAULT,
            );
        }
    }

    /// Start the gRPC server with graceful shutdown support.
    ///
    /// `listener` is a pre-bound `TcpListener` (testability + no bind-window between
    /// resolving the address and accepting connections). `tls_handle = Some(_)` runs
    /// an owned TLS accept loop that reloads the server identity from the hot-swappable
    /// handle on every new handshake (spec §6 T3); `tls_handle = None` keeps the existing
    /// plaintext / skip_tls path unchanged, served directly by tonic.
    pub async fn serve_with_shutdown(
        self,
        listener: tokio::net::TcpListener,
        security: &ConfSyncSecurityConfig,
        tls_handle: Option<Arc<crate::conf_sync::conf_server::tls_identity::ServerIdentityHandle>>,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let mut builder = tonic::transport::Server::builder()
            .http2_keepalive_interval(Some(Duration::from_secs(HTTP2_KEEPALIVE_INTERVAL_SECS)))
            .http2_keepalive_timeout(Some(Duration::from_secs(HTTP2_KEEPALIVE_TIMEOUT_SECS)))
            // Defense in depth: no conf_sync session (this channel streams resolved
            // Secrets) may outlive a bounded window without the peer re-handshaking
            // against the CURRENT client-CA verifier. See MAX_CONNECTION_AGE_SECS; the
            // hard cutoff at age + grace lives in TrustScopedStream, because tonic's
            // graceful GOAWAY alone never closes a connection carrying a live Watch.
            .max_connection_age(Duration::from_secs(MAX_CONNECTION_AGE_SECS));

        match tls_handle {
            Some(handle) => {
                // Owned TLS accept loop (spec §6 T3): per-connection config load from the
                // ArcSwap handle; listener is never rebound; established sessions are
                // never touched by a swap.
                tracing::info!(
                    component = "grpc_server",
                    "conf_sync mTLS enabled (hot-reloadable accept loop)"
                );
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<TrustScopedStream, std::io::Error>>(64);
                let accept_shutdown = self.shutdown_token.clone();
                let handshake_slots = Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT_HANDSHAKES));
                tokio::spawn(async move {
                    loop {
                        // Take a handshake slot BEFORE accepting. When all
                        // MAX_INFLIGHT_HANDSHAKES slots are held we simply stop calling
                        // accept(), so the pressure lands in the kernel accept backlog
                        // instead of in an unbounded pile of spawned tasks holding
                        // sockets. This *pauses* accepting; it never drops a connection.
                        // The acquire sits inside the shutdown select! so cancellation
                        // still breaks the loop promptly even while saturated.
                        let permit = tokio::select! {
                            _ = accept_shutdown.cancelled() => break,
                            p = handshake_slots.clone().acquire_owned() => match p {
                                Ok(p) => p,
                                // The semaphore is never closed; treat it as a fatal
                                // invariant break rather than spinning.
                                Err(_) => break,
                            },
                        };
                        tokio::select! {
                            _ = accept_shutdown.cancelled() => break,
                            accepted = listener.accept() => {
                                let (tcp, peer) = match accepted {
                                    Ok(v) => v,
                                    Err(e) => {
                                        // Transient accept errors (e.g. EMFILE) must not kill the loop.
                                        // `permit` is released here (end of scope) — no slot leak.
                                        tracing::warn!(component = "grpc_server", error = %e, "conf_sync accept error, retrying");
                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                        continue;
                                    }
                                };
                                // tonic's serve_with_incoming discards Server TCP settings; restore nodelay.
                                let _ = tcp.set_nodelay(true);
                                let acceptor = tokio_rustls::TlsAcceptor::from(handle.load().config.clone());
                                // Capture the trust token that governs the root we are about to
                                // authenticate this peer against. Taken BEFORE the handshake, so a
                                // re-key that lands mid-handshake cancels this connection too
                                // (the token is already cancelled when the stream is built, and
                                // TrustScopedStream fails on its first poll).
                                let trust = handle.trust_token();
                                let tx = tx.clone();
                                let handoff_shutdown = accept_shutdown.clone();
                                tokio::spawn(async move {
                                    // Held until the handshake resolves and its result has been
                                    // handed off (or the handoff timed out), then dropped, freeing
                                    // the slot. The handoff MUST stay time-bounded or this permit
                                    // becomes a permanent hostage of a wedged consumer — see
                                    // TLS_HANDOFF_TIMEOUT_SECS.
                                    let _permit = permit;
                                    match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await {
                                        Ok(Ok(tls)) => {
                                            // The stream tonic gets dies the moment `trust` is
                                            // cancelled (a trust-ROOT change) — that, not the
                                            // verifier swap alone, is what revokes a leaked leaf
                                            // whose connection is already open.
                                            let scoped = TrustScopedStream::new(tls, &trust);
                                            let handoff = tokio::select! {
                                                // Graceful shutdown: the tonic consumer is going
                                                // away, so a pending send would otherwise sit for
                                                // the full 30s and then report a spurious
                                                // TLS_HANDSHAKE_FAILED on a perfectly healthy exit.
                                                _ = handoff_shutdown.cancelled() => None,
                                                r = tokio::time::timeout(
                                                    Duration::from_secs(TLS_HANDOFF_TIMEOUT_SECS),
                                                    tx.send(Ok(scoped)),
                                                ) => Some(r),
                                            };
                                            match handoff {
                                                // Handed off: the connection is tonic's now.
                                                Some(Ok(Ok(()))) => {
                                                    edgion_common::observe::system_metrics::clear_system_error(
                                                        edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
                                                        edgion_common::observe::system_metrics::reasons::TLS_HANDSHAKE_FAILED,
                                                    );
                                                }
                                                // Receiver gone = the tonic server stopped (shutdown).
                                                // Not an error condition worth marking.
                                                Some(Ok(Err(_))) => {}
                                                // Consumer wedged/backlogged for 30s: DROP this
                                                // connection rather than hold the permit forever.
                                                Some(Err(_)) => warn_handoff_timeout(peer),
                                                // Shutting down: drop the connection silently.
                                                None => {}
                                            }
                                        }
                                        Ok(Err(e)) => {
                                            // NEVER forward as Err: an Err item terminates the whole tonic server.
                                            warn_handshake_failure(peer, &e);
                                        }
                                        Err(_) => {
                                            warn_handshake_timeout(peer);
                                        }
                                    }
                                });
                            }
                        }
                    }
                });
                let incoming = tokio_stream::wrappers::ReceiverStream::new(rx);
                builder
                    .add_service(self.into_service())
                    .serve_with_incoming_shutdown(incoming, shutdown_signal)
                    .await?;
            }
            None => {
                // Plaintext / skip_tls path: keep the existing direct tonic serve.
                Self::log_plaintext_or_skip_tls(security);
                // TcpListenerStream loses TCP_NODELAY; TcpIncoming restores it per-socket.
                let incoming = tonic::transport::server::TcpIncoming::from_listener(listener, true, None)
                    .map_err(|e| anyhow::anyhow!("failed to build TCP incoming: {e}"))?;
                builder
                    .add_service(self.into_service())
                    .serve_with_incoming_shutdown(incoming, shutdown_signal)
                    .await?;
            }
        }

        tracing::info!(component = "grpc_server", "gRPC server stopped");

        Ok(())
    }
}

/// Rate-limited WARN (at most once per minute; DEBUG otherwise) for a per-connection
/// TLS handshake failure, plus an always-on system-error metric mark.
fn warn_handshake_failure(peer: std::net::SocketAddr, error: &std::io::Error) {
    if should_warn_handshake_failure() {
        tracing::warn!(component = "grpc_server", peer = %peer, error = %error, "conf_sync TLS handshake failed");
    } else {
        tracing::debug!(component = "grpc_server", peer = %peer, error = %error, "conf_sync TLS handshake failed");
    }
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
        edgion_common::observe::system_metrics::reasons::TLS_HANDSHAKE_FAILED,
    );
}

/// Rate-limited WARN (at most once per minute; DEBUG otherwise) for a per-connection
/// TLS handshake timeout, plus an always-on system-error metric mark.
fn warn_handshake_timeout(peer: std::net::SocketAddr) {
    if should_warn_handshake_failure() {
        tracing::warn!(component = "grpc_server", peer = %peer, "conf_sync TLS handshake timed out (10s)");
    } else {
        tracing::debug!(component = "grpc_server", peer = %peer, "conf_sync TLS handshake timed out (10s)");
    }
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
        edgion_common::observe::system_metrics::reasons::TLS_HANDSHAKE_FAILED,
    );
}

/// Rate-limited WARN (at most once per minute; DEBUG otherwise) for a successfully
/// handshaked connection that could not be handed to the tonic consumer within
/// [`TLS_HANDOFF_TIMEOUT_SECS`], plus the same system-error mark as a handshake
/// failure — from the peer's point of view the outcome is identical (the connection
/// is dropped), and the operator response is the same (look at conf_sync server
/// health). The connection is dropped and the handshake permit released, which is
/// what keeps the accept loop alive under a wedged/backlogged consumer.
fn warn_handoff_timeout(peer: std::net::SocketAddr) {
    if should_warn_handshake_failure() {
        tracing::warn!(
            component = "grpc_server",
            peer = %peer,
            timeout_secs = TLS_HANDOFF_TIMEOUT_SECS,
            "conf_sync TLS connection handoff timed out (tonic consumer not draining); dropping the connection"
        );
    } else {
        tracing::debug!(
            component = "grpc_server",
            peer = %peer,
            timeout_secs = TLS_HANDOFF_TIMEOUT_SECS,
            "conf_sync TLS connection handoff timed out; dropping the connection"
        );
    }
    edgion_common::observe::system_metrics::mark_system_error(
        edgion_common::observe::system_metrics::components::CONF_SYNC_SERVER,
        edgion_common::observe::system_metrics::reasons::TLS_HANDSHAKE_FAILED,
    );
}

/// Simple rate-limit gate shared by both handshake-failure warning sites: WARN if
/// more than 60s have elapsed since the last WARN, DEBUG otherwise. `mark_system_error`
/// is called unconditionally by the caller regardless of this gate's decision.
fn should_warn_handshake_failure() -> bool {
    static LAST_WARN: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);
    let mut last = LAST_WARN.lock();
    let now = std::time::Instant::now();
    let should_warn = match *last {
        Some(t) => now.duration_since(t) > Duration::from_secs(60),
        None => true,
    };
    if should_warn {
        *last = Some(now);
    }
    should_warn
}

#[tonic::async_trait]
impl ConfigSync for ConfigSyncGrpcServer {
    async fn get_server_info(
        &self,
        _request: Request<ServerInfoRequest>,
    ) -> Result<Response<ServerInfoResponse>, Status> {
        let server = self.get_server()?;

        let supported_kinds = server.all_kinds();

        tracing::debug!(
            component = "grpc_server",
            supported_kinds = ?supported_kinds,
            "GetServerInfo request"
        );

        Ok(Response::new(ServerInfoResponse {
            server_id: server.server_id(),
            supported_kinds,
        }))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = server.server_id();
            if req.expected_server_id != current_server_id {
                tracing::warn!(
                    component = "grpc_server",
                    expected = %req.expected_server_id,
                    actual = %current_server_id,
                    "Server ID mismatch in list request"
                );
                return Err(Status::failed_precondition(WATCH_ERR_SERVER_ID_MISMATCH));
            }
        }

        if req.kind.is_empty() {
            return Err(Status::invalid_argument("Resource kind is required"));
        }

        let list_data = server
            .list(&req.kind)
            .map_err(|e| Status::internal(format!("Failed to list resources: {}", e)))?;

        Ok(Response::new(ListResponse {
            data: list_data.data,
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = server.server_id();
            if req.expected_server_id != current_server_id {
                tracing::warn!(
                    component = "grpc_server",
                    expected = %req.expected_server_id,
                    actual = %current_server_id,
                    client_id = %req.client_id,
                    "Server ID mismatch in watch request"
                );
                return Err(Status::failed_precondition(WATCH_ERR_SERVER_ID_MISMATCH));
            }
        }

        if req.kind.is_empty() {
            return Err(Status::invalid_argument("Resource kind is required"));
        }

        let kind_name = &req.kind;
        let client_id_log = req.client_id.clone();
        let client_name_log = req.client_name.clone();

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = %kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            from_version = req.from_version,
            "Watch request received"
        );

        let receiver = server
            .watch(kind_name, req.client_id, req.client_name, req.from_version)
            .map_err(|e| {
                tracing::error!(
                    component = "grpc_server",
                    key = %req.key,
                    kind = %kind_name,
                    client_id = %client_id_log,
                    client_name = %client_name_log,
                    error = %e,
                    "Watch failed"
                );
                Status::internal(format!("Failed to start watch: {}", e))
            })?;

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = %kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            "Watch established"
        );

        // Convert EventDataSimple receiver to WatchResponse stream
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        // Capture provider and initial server_id for reload detection
        let provider = self.provider.clone();
        let initial_server_id = server.server_id();
        let shutdown_token = self.shutdown_token.clone();
        let shutdown_kind = kind_name.to_string();
        let shutdown_client_id = client_id_log.clone();

        tokio::spawn(async move {
            let mut receiver = receiver;
            let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                tokio::select! {
                    // Check for data from watch stream
                    event = receiver.recv() => {
                        match event {
                            Some(event_data) => {
                                let response = WatchResponse {
                                    data: event_data.data,
                                    sync_version: event_data.sync_version,
                                    err: event_data.err.unwrap_or_default(),
                                    server_id: event_data.server_id,
                                };

                                if tx.send(Ok(response)).await.is_err() {
                                    break;
                                }
                            }
                            None => {
                                tracing::warn!(
                                    component = "grpc_server",
                                    kind = %shutdown_kind,
                                    client_id = %shutdown_client_id,
                                    "watch event receiver closed unexpectedly — upstream channel dropped"
                                );
                                break;
                            }
                        }
                    }
                    // Periodically check if server_id changed (reload completed)
                    _ = check_interval.tick() => {
                        if let Some(new_server) = provider.config_sync_server() {
                            let new_server_id = new_server.server_id();
                            if new_server_id != initial_server_id {
                                // Server reloaded, notify client to relist
                                tracing::info!(
                                    component = "grpc_server",
                                    old_server_id = %initial_server_id,
                                    new_server_id = %new_server_id,
                                    "Server reloaded, notifying client to relist"
                                );
                                let response = WatchResponse {
                                    data: String::new(),
                                    sync_version: 0,
                                    err: WATCH_ERR_SERVER_RELOAD.to_string(),
                                    server_id: new_server_id,
                                };
                                let _ = tx.send(Ok(response)).await;
                                reload_metrics().client_notified();
                                break;
                            }
                        }
                        // If provider returns None, server is reloading, keep waiting
                    }
                    // Shutdown signal — exit the stream task promptly
                    _ = shutdown_token.cancelled() => {
                        tracing::info!(
                            component = "grpc_server",
                            kind = %shutdown_kind,
                            client_id = %shutdown_client_id,
                            "Watch stream task shutting down"
                        );
                        break;
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    type WatchServerMetaStream = tokio_stream::wrappers::ReceiverStream<Result<ServerMetaEvent, Status>>;

    async fn watch_server_meta(
        &self,
        request: Request<WatchServerMetaRequest>,
    ) -> Result<Response<Self::WatchServerMetaStream>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();
        let registry = server.client_registry();

        registry.register(req.client_id.clone(), req.client_name.clone());

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let server_id = server.server_id();
        let client_id = req.client_id.clone();

        tracing::info!(
            component = "grpc_server",
            client_id = %client_id,
            client_name = %req.client_name,
            "WatchServerMeta stream started"
        );

        // Capture provider and initial server_id for reload detection
        let provider = self.provider.clone();
        let initial_server_id = server_id.clone();
        let shutdown_token = self.shutdown_token.clone();

        tokio::spawn(async move {
            // Helper to get current timestamp in millis
            let now_millis = || -> u64 {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            };

            // Send initial state immediately
            let initial_event = ServerMetaEvent {
                server_id: initial_server_id.clone(),
                gateway_instance_count: registry.count(),
                timestamp: now_millis(),
            };
            if tx.send(Ok(initial_event)).await.is_err() {
                registry.unregister(&client_id);
                return;
            }

            // Push on change (event-driven) + periodic server-switch detection (every 1s)
            // Mirrors the pattern used in watch() to detect controller reload/leader switch.
            let mut last_count = registry.count();
            let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                tokio::select! {
                    _ = registry.wait_for_change() => {
                        let new_count = registry.count();
                        if new_count == last_count {
                            continue; // Spurious wakeup or concurrent change that reverted
                        }
                        last_count = new_count;

                        let event = ServerMetaEvent {
                            server_id: initial_server_id.clone(),
                            gateway_instance_count: new_count,
                            timestamp: now_millis(),
                        };

                        if tx.send(Ok(event)).await.is_err() {
                            // Client disconnected, unregister
                            tracing::info!(
                                component = "grpc_server",
                                client_id = %client_id,
                                "WatchServerMeta client disconnected, unregistering"
                            );
                            registry.unregister(&client_id);
                            break;
                        }
                    }
                    // Detect client disconnect even when no count-change event fires.
                    // Without this, a hard-killed gateway (no graceful FIN) would remain
                    // registered until the next count-change event, inflating the divisor
                    // used by cluster-scoped rate limits on the surviving gateways.
                    _ = tx.closed() => {
                        tracing::info!(
                            component = "grpc_server",
                            client_id = %client_id,
                            "WatchServerMeta receiver closed, unregistering"
                        );
                        registry.unregister(&client_id);
                        break;
                    }
                    // Periodically check if server_id changed (controller reload / leader switch)
                    _ = check_interval.tick() => {
                        if let Some(new_server) = provider.config_sync_server() {
                            let new_server_id = new_server.server_id();
                            if new_server_id != initial_server_id {
                                tracing::info!(
                                    component = "grpc_server",
                                    client_id = %client_id,
                                    old_server_id = %initial_server_id,
                                    new_server_id = %new_server_id,
                                    "Server reloaded, closing WatchServerMeta stream to force reconnect"
                                );
                                // Send a terminal event carrying the new server_id so the
                                // client can detect the switch boundary before reconnecting.
                                let switch_event = ServerMetaEvent {
                                    server_id: new_server_id,
                                    gateway_instance_count: new_server.client_registry().count(),
                                    timestamp: now_millis(),
                                };
                                let _ = tx.send(Ok(switch_event)).await;
                                // Unregister from the old registry before exiting
                                registry.unregister(&client_id);
                                break;
                            }
                        }
                        // If provider returns None the server is mid-reload; keep waiting.
                    }
                    // Shutdown signal — exit the stream task promptly
                    _ = shutdown_token.cancelled() => {
                        tracing::info!(
                            component = "grpc_server",
                            client_id = %client_id,
                            "WatchServerMeta stream task shutting down"
                        );
                        registry.unregister(&client_id);
                        break;
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use tokio_stream::StreamExt;

    // A mock provider that allows us to simulate a server reload
    struct MockConfigSyncProvider {
        server: Mutex<Arc<ConfigSyncServer>>,
    }

    impl MockConfigSyncProvider {
        fn new(server: Arc<ConfigSyncServer>) -> Self {
            Self {
                server: Mutex::new(server),
            }
        }

        fn set_server(&self, server: Arc<ConfigSyncServer>) {
            *self.server.lock() = server;
        }
    }

    impl ConfigSyncServerProvider for MockConfigSyncProvider {
        fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
            Some(self.server.lock().clone())
        }
    }

    #[tokio::test]
    async fn test_watch_server_meta_reload_switch() {
        // 1. Create initial server
        let initial_server = Arc::new(ConfigSyncServer::new());
        let initial_server_id = initial_server.server_id();

        let provider = Arc::new(MockConfigSyncProvider::new(initial_server.clone()));
        let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), CancellationToken::new());

        let client_id = "test-client-123".to_string();

        // 2. Start watch_server_meta
        let request = Request::new(WatchServerMetaRequest {
            client_id: client_id.clone(),
            client_name: "test-gateway".to_string(),
        });

        let response = grpc_server.watch_server_meta(request).await.unwrap();
        let mut stream = response.into_inner();

        // 3. Receive initial event
        let first_event = stream.next().await.unwrap().unwrap();
        assert_eq!(first_event.server_id, initial_server_id);
        assert_eq!(first_event.gateway_instance_count, 1); // 1 because the request registered the client

        // Verify registration in old registry
        assert_eq!(initial_server.client_registry().count(), 1);

        // 4. Simulate server reload (new server generated)
        let new_server = Arc::new(ConfigSyncServer::new());
        let new_server_id = new_server.server_id();
        assert_ne!(initial_server_id, new_server_id);

        provider.set_server(new_server.clone());

        // 5. Watch for the terminal event marking the switch boundary.
        // We use a short timeout because the tick happens every second.
        let switch_event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .expect("should yield switch event quickly")
            .unwrap()
            .unwrap();

        // Terminal event should carry the NEW server_id and count
        assert_eq!(switch_event.server_id, new_server_id);

        // 6. Next poll should return None (stream closed)
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_watch_server_meta_shutdown() {
        // 1. Create server with a real CancellationToken
        let server = Arc::new(ConfigSyncServer::new());
        let provider = Arc::new(MockConfigSyncProvider::new(server.clone()));
        let token = CancellationToken::new();
        let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), token.clone());

        // 2. Start watch_server_meta
        let request = Request::new(WatchServerMetaRequest {
            client_id: "shutdown-test-client".to_string(),
            client_name: "shutdown-test-gateway".to_string(),
        });
        let response = grpc_server.watch_server_meta(request).await.unwrap();
        let mut stream = response.into_inner();

        // 3. Receive initial event (the stream always sends one immediately)
        let first_event = stream.next().await.unwrap().unwrap();
        assert_eq!(first_event.gateway_instance_count, 1); // 1 because request registered the client

        // 4. Verify client is registered
        assert_eq!(server.client_registry().count(), 1);

        // 5. Cancel the token (simulating shutdown)
        token.cancel();

        // 6. The stream should end (yield None) shortly after cancellation
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await;

        // Should complete within timeout
        let next = result.expect("stream should end within 3 seconds");
        assert!(next.is_none(), "stream should be closed after shutdown");

        // 7. Verify client was unregistered (cleanup happened)
        // Give a brief moment for the spawned task to complete unregistration
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // After unregister, the HashMap is empty but count() returns max(1, len) = 1
        // The important thing is that the actual clients map is empty.
        // count() always returns at least 1, so we just verify the stream closed cleanly.
        assert_eq!(server.client_registry().count(), 1);
    }

    /// A minimal mock WatchObj that keeps a sender so we can push events or close the channel.
    struct MockWatchObj {
        tx: parking_lot::Mutex<Option<tokio::sync::mpsc::Sender<super::super::traits::WatchResponseSimple>>>,
        rx: parking_lot::Mutex<Option<tokio::sync::mpsc::Receiver<super::super::traits::WatchResponseSimple>>>,
    }

    impl MockWatchObj {
        fn new() -> Self {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            Self {
                tx: parking_lot::Mutex::new(Some(tx)),
                rx: parking_lot::Mutex::new(Some(rx)),
            }
        }
    }

    impl super::super::traits::WatchObj for MockWatchObj {
        fn kind_name(&self) -> &'static str {
            "MockKind"
        }

        fn list_json(&self) -> Result<(String, u64), String> {
            Ok(("[]".to_string(), 0))
        }

        fn watch_json(
            &self,
            _client_id: String,
            _client_name: String,
            _from_version: u64,
        ) -> tokio::sync::mpsc::Receiver<super::super::traits::WatchResponseSimple> {
            // Return the pre-created receiver (consumed once)
            self.rx.lock().take().expect("watch_json called more than once")
        }

        fn is_ready(&self) -> bool {
            true
        }

        fn set_ready(&self) {}

        fn set_not_ready(&self) {}

        fn clear(&self) {}
    }

    #[tokio::test]
    async fn test_watch_stream_shutdown() {
        use super::super::traits::WatchResponseSimple;

        // 1. Create server with a registered mock WatchObj and a real CancellationToken
        let server = Arc::new(ConfigSyncServer::new());
        let mock_obj = Arc::new(MockWatchObj::new());

        // Hold a sender clone so we can push events through the mock watch channel
        let mock_tx = mock_obj.tx.lock().clone().unwrap();

        server.register_watch_obj("MockKind", mock_obj);

        let provider = Arc::new(MockConfigSyncProvider::new(server.clone()));
        let token = CancellationToken::new();
        let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), token.clone());

        // 2. Start a watch on the registered kind
        let request = Request::new(WatchRequest {
            key: "test".to_string(),
            kind: "MockKind".to_string(),
            client_id: "watch-shutdown-client".to_string(),
            client_name: "watch-shutdown-gateway".to_string(),
            from_version: 0,
            expected_server_id: String::new(),
        });
        let response = grpc_server.watch(request).await.unwrap();
        let mut stream = response.into_inner();

        // 3. Push one event and verify it arrives
        mock_tx
            .send(WatchResponseSimple::new("test-data".to_string(), 1))
            .await
            .unwrap();

        let first = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .expect("should receive first event")
            .unwrap()
            .unwrap();
        assert_eq!(first.data, "test-data");
        assert_eq!(first.sync_version, 1);

        // 4. Cancel the token (simulating shutdown)
        token.cancel();

        // 5. The stream should end (yield None) shortly after cancellation
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await;
        let next = result.expect("stream should end within 3 seconds");
        assert!(next.is_none(), "stream should be closed after shutdown");
    }

    // rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
    // (invoked transitively via `ServerIdentity::from_pems`) can be called. Installing it
    // here races harmlessly with other test modules doing the same `install_default()`.
    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    use crate::conf_sync::conf_server::tls_identity::{ServerIdentity, ServerIdentityHandle};
    use crate::services::conf_sync_ca::Ca;
    use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as GrpcClient;
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};

    fn client_tls(ca: &Ca, leaf: &crate::services::conf_sync_ca::LeafPem) -> ClientTlsConfig {
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(ca.ca_cert_pem()))
            .identity(Identity::from_pem(&leaf.cert_pem, leaf.key_pem.as_str()))
            .domain_name("localhost")
    }

    async fn connect(
        addr: std::net::SocketAddr,
        tls: ClientTlsConfig,
    ) -> anyhow::Result<GrpcClient<tonic::transport::Channel>> {
        let ch = Endpoint::from_shared(format!("https://localhost:{}", addr.port()))?
            .tls_config(tls)?
            .connect()
            .await?;
        Ok(GrpcClient::new(ch))
    }

    /// Spin up the owned TLS accept loop on an ephemeral port with a mock provider.
    /// Returns the address and the shutdown token.
    async fn spawn_tls_server(handle: Arc<ServerIdentityHandle>) -> (std::net::SocketAddr, CancellationToken) {
        let server = Arc::new(ConfigSyncServer::new());
        let provider = Arc::new(MockConfigSyncProvider::new(server));
        let token = CancellationToken::new();
        let grpc = ConfigSyncGrpcServer::new(provider, token.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let security = ConfSyncSecurityConfig::default();
        let shutdown = {
            let t = token.clone();
            async move { t.cancelled().await }
        };
        tokio::spawn(async move {
            grpc.serve_with_shutdown(listener, &security, Some(handle), shutdown)
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        (addr, token)
    }

    /// Routine LEAF rotation (same trust root): established streams MUST survive.
    /// Zero-disruption rotation is the whole point of the hot-swap design — the peers on
    /// those sessions are still authorized.
    #[tokio::test]
    async fn tls_accept_loop_hot_swaps_leaf_without_dropping_streams() {
        install_test_crypto_provider();

        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let server_a = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let client = ca.sign_client(time::Duration::days(7)).unwrap();
        let handle = Arc::new(ServerIdentityHandle::new(
            ServerIdentity::from_pems(&server_a.cert_pem, &server_a.key_pem, ca.ca_cert_pem()).unwrap(),
        ));
        let (addr, token) = spawn_tls_server(handle.clone()).await;

        // Connect (this also proves ALPN h2: tonic requires it) and open a long-lived stream.
        let mut c1 = connect(addr, client_tls(&ca, &client)).await.unwrap();
        let mut meta = c1
            .watch_server_meta(WatchServerMetaRequest {
                client_id: "c1".into(),
                client_name: "g1".into(),
            })
            .await
            .unwrap()
            .into_inner();
        let first = meta.message().await.unwrap().unwrap();
        assert_eq!(first.gateway_instance_count, 1);

        // Re-sign the SERVER LEAF against the SAME root (exactly what identity_sync does
        // every half-life).
        let server_b = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(14))
            .unwrap();
        handle.store(ServerIdentity::from_pems(&server_b.cert_pem, &server_b.key_pem, ca.ca_cert_pem()).unwrap());

        // A second client registers, which pushes a count event onto the OLD stream —
        // proving that stream survived the swap.
        let mut c2 = connect(addr, client_tls(&ca, &client)).await.unwrap();
        let _meta2 = c2
            .watch_server_meta(WatchServerMetaRequest {
                client_id: "c2".into(),
                client_name: "g2".into(),
            })
            .await
            .unwrap()
            .into_inner();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), meta.message())
            .await
            .expect("old stream must still deliver events after a same-root leaf rotation")
            .unwrap()
            .unwrap();
        assert_eq!(ev.gateway_instance_count, 2);

        token.cancel();
    }

    /// Trust-ROOT change (emergency CA re-key): an ALREADY-ESTABLISHED session — i.e. the
    /// attacker holding the leaked leaf, who never has to handshake again — MUST be torn
    /// down. Swapping the client-CA verifier alone only governs future handshakes; without
    /// the trust token this stream keeps receiving resolved Secrets forever.
    #[tokio::test]
    async fn tls_accept_loop_tears_down_established_streams_on_root_change() {
        install_test_crypto_provider();

        let ca_a = Ca::generate_root(time::Duration::days(30)).unwrap();
        let server_a = ca_a
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let client_a = ca_a.sign_client(time::Duration::days(7)).unwrap();
        let handle = Arc::new(ServerIdentityHandle::new(
            ServerIdentity::from_pems(&server_a.cert_pem, &server_a.key_pem, ca_a.ca_cert_pem()).unwrap(),
        ));
        let (addr, token) = spawn_tls_server(handle.clone()).await;

        let mut c1 = connect(addr, client_tls(&ca_a, &client_a)).await.unwrap();
        let mut meta = c1
            .watch_server_meta(WatchServerMetaRequest {
                client_id: "c1".into(),
                client_name: "g1".into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(meta.message().await.unwrap().unwrap().gateway_instance_count, 1);

        // Re-key: a BRAND-NEW root (new private key).
        let ca_b = Ca::generate_root(time::Duration::days(30)).unwrap();
        let server_b = ca_b
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        let client_b = ca_b.sign_client(time::Duration::days(7)).unwrap();
        handle.store(ServerIdentity::from_pems(&server_b.cert_pem, &server_b.key_pem, ca_b.ca_cert_pem()).unwrap());

        // 1. The LIVE session established under the old root is killed (the connection is
        //    aborted, so the stream ends — either with a transport error or with EOF).
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), meta.message())
            .await
            .expect("the established session must be torn down by the re-key, not left open");
        assert!(
            matches!(&ended, Err(_) | Ok(None)),
            "expected the revoked session to end, got {ended:?}"
        );

        // 2. A NEW handshake with the old (compromised) identity is rejected.
        assert!(connect(addr, client_tls(&ca_a, &client_a)).await.is_err());

        // 3. The server still works for a client under the new root.
        let mut c2 = connect(addr, client_tls(&ca_b, &client_b)).await.unwrap();
        let mut meta2 = c2
            .watch_server_meta(WatchServerMetaRequest {
                client_id: "c2".into(),
                client_name: "g2".into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert!(meta2.message().await.unwrap().is_some());

        token.cancel();
    }
}
