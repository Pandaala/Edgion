use dashmap::{mapref::entry::Entry, DashMap};
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::backends::{resolve_target_port_key, select_roundrobin_backend_key, validate_endpoint_backend_addr};
use crate::observe::{log_udp, UdpLogEntry};
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::tcp::edgion_tcp::{check_stream_plugins, StreamPluginCheck};
use crate::routes::udp::UdpPortRouteManager;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfig;
use edgion_resources::ListenerKey;

/// UDP session idle timeout: a session is reclaimed after 60 seconds without
/// traffic in either direction.
///
/// This is intentionally a fixed constant, NOT operator-configurable — unlike
/// the TCP/TLS L4 relay, whose idle timeout is resolved from
/// `tcpTimeout.idleTimeout` (`resolve_tcp_idle_timeout`). UDP is connectionless
/// soft state with no close handshake, so 60s follows the conventional NAT-style
/// UDP mapping lifetime and bounds resource use per [`MAX_UDP_SESSIONS`]. A
/// `udpTimeout.idleTimeout` knob is deliberately deferred until a concrete need
/// appears; see the timeout-coverage audit (item 4) for the rationale. The value
/// is pinned by a unit test in this module so any change is deliberate.
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum UDP packet size (64KB)
const MAX_UDP_PACKET_SIZE: usize = 65535;

/// Maximum number of concurrent client sessions per listener.
/// Prevents resource exhaustion from UDP reflection/amplification attacks.
const MAX_UDP_SESSIONS: usize = 10000;

/// Pick the wildcard bind address whose family matches the upstream.
///
/// A UDP socket can only `send_to` a destination of the same address family,
/// so an IPv4-only socket sending to an IPv6 backend fails with `EINVAL`.
fn upstream_bind_addr(upstream: std::net::SocketAddr) -> std::net::SocketAddr {
    match upstream {
        std::net::SocketAddr::V4(_) => std::net::SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0)),
        std::net::SocketAddr::V6(_) => std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)),
    }
}

/// Client session information with statistics
struct ClientSession {
    session_permit: Mutex<Option<OwnedSemaphorePermit>>,
    upstream_socket: Arc<UdpSocket>,
    upstream_addr: std::net::SocketAddr,
    last_activity: Arc<Mutex<Instant>>,
    session_start: Instant,
    packets_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
}

impl ClientSession {
    fn release_permit(&self) {
        self.session_permit.lock().take();
    }
}

/// UDP proxy service for Gateway UDP listeners.
///
/// Each instance is bound to a single listener port with its own per-port
/// `UdpPortRouteManager`. Route updates swap the inner `ArcSwap<UdpRouteTable>`
/// so the `Arc<UdpPortRouteManager>` stays stable.
///
/// Lifecycle is managed via `CancellationToken`: when the listener is removed
/// (e.g. during hot-reload), calling `cancel_token.cancel()` stops the main
/// loop, the session cleanup loop, and all upstream listener tasks.
pub struct EdgionUdpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub udp_route_manager: Arc<UdpPortRouteManager>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub socket: Arc<UdpSocket>,
    client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
    session_permits: Arc<Semaphore>,
    cancel_token: CancellationToken,
}

impl EdgionUdpProxy {
    pub fn new(
        listener_port: u16,
        listener_key: ListenerKey,
        udp_route_manager: Arc<UdpPortRouteManager>,
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        socket: UdpSocket,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            listener_port,
            listener_key,
            udp_route_manager,
            edgion_gateway_config,
            socket: Arc::new(socket),
            client_sessions: Arc::new(DashMap::new()),
            session_permits: Arc::new(Semaphore::new(MAX_UDP_SESSIONS)),
            cancel_token,
        }
    }

    /// Main service loop — receives packets from clients and handles them.
    ///
    /// Exits when `cancel_token` is cancelled or the socket errors out.
    /// On exit, cancels the token so that `session_cleanup_loop` and all
    /// per-session `handle_upstream_packets` tasks also terminate.
    pub async fn serve(self: Arc<Self>) {
        let cleanup_self = self.clone();
        tokio::spawn(async move {
            cleanup_self.session_cleanup_loop().await;
        });

        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => break,
                result = self.socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, direct_peer_addr)) => {
                            tokio::select! {
                                biased;
                                _ = self.cancel_token.cancelled() => break,
                                _ = self.handle_client_packet(&buf[..len], direct_peer_addr) => {}
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        // Ensure all child tasks (cleanup loop, upstream listeners) exit
        // when the main loop terminates for any reason.
        self.cancel_token.cancel();
    }

    async fn handle_client_packet(&self, data: &[u8], direct_peer_addr: std::net::SocketAddr) {
        let direct_peer_ip = crate::ctx::canonicalize_ip(direct_peer_addr.ip());
        let direct_peer_port = direct_peer_addr.port();

        // Fast path: an established session already has a pinned upstream and a
        // dedicated upstream socket, so the route pipeline below (ArcSwap load,
        // route match, backend selection, round-robin, SSRF validation) is pure
        // redundant work — its result is discarded for existing sessions anyway,
        // since the send always targets the session's stored `upstream_addr`.
        // Route/backend changes take effect when the session expires
        // (SESSION_TIMEOUT) and a new one is created. Clone the Arc out so the
        // DashMap read guard is dropped before the `.await` below.
        if let Some(session) = self.client_sessions.get(&direct_peer_addr).map(|s| s.value().clone()) {
            let send_result = session.upstream_socket.send_to(data, session.upstream_addr).await;
            Self::record_upstream_send_result(
                send_result,
                &session,
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
            )
            .await;
            return;
        }

        let route_table = self.udp_route_manager.load_route_table();
        let unit = match route_table.match_route() {
            Some(unit) => unit,
            None => {
                log_udp(&UdpLogEntry::failure(
                    self.listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    "NoRouteMatched",
                ))
                .await;
                return;
            }
        };
        let udp_route = &unit.route;

        // Require at least one rule. The compiled runtime (backend_finder,
        // stream-plugin store key) lives on the unit and is built from the
        // first rule, so a route with no rules cannot serve traffic.
        if udp_route.spec.rules.as_ref().and_then(|rules| rules.first()).is_none() {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                "NoRuleAvailable",
            ))
            .await;
            return;
        }

        // Stream plugin enforcement (dynamic lookup from StreamPluginStore, fail-close).
        // Mirrors the TCPRoute path: a packet whose rule references an
        // EdgionStreamPlugins resource is dropped unless the policy admits it.
        // Only packets that reach the pipeline are checked — the fast path above
        // returns first for established sessions, so the check runs once at
        // session creation (like TCP's per-connection check). A denied client is
        // dropped before get_or_create_session and therefore never establishes a
        // session. A rule with no annotation has store_key None, so the check
        // returns Allow immediately and the non-annotated path pays only one
        // Option check.
        if let StreamPluginCheck::Deny(reason) = check_stream_plugins(
            unit.stream_plugin_store_key.as_deref(),
            &direct_peer_ip,
            self.listener_port,
        )
        .await
        {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                reason,
            ))
            .await;
            return;
        }

        let backend_ref = match unit.backend_finder.select() {
            Ok(backend) => backend,
            Err(_) => {
                log_udp(&UdpLogEntry::failure(
                    self.listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    "NoBackendSelected",
                ))
                .await;
                return;
            }
        };

        let route_ns = udp_route.metadata.namespace.as_deref().unwrap_or("default");
        let backend_ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
        let service_key = format!("{}/{}", backend_ns, backend_ref.name);

        let backend = match select_roundrobin_backend_key(&service_key) {
            Some(backend) => backend,
            None => {
                log_udp(&UdpLogEntry::failure(
                    self.listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    "NoBackendResolved",
                ))
                .await;
                return;
            }
        };

        let mut upstream_pingora_addr = backend.inner().addr.clone();
        resolve_target_port_key(&mut upstream_pingora_addr, backend_ref.port, &service_key);
        let upstream_addr = match upstream_pingora_addr {
            PingoraSocketAddr::Inet(sockaddr) => sockaddr,
            PingoraSocketAddr::Unix(_) => return,
        };
        if validate_endpoint_backend_addr(&PingoraSocketAddr::Inet(upstream_addr), self.allow_loopback_upstream())
            .is_err()
        {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                UNSAFE_UPSTREAM_ADDRESS_LOG,
            ))
            .await;
            return;
        }

        let session = match self.get_or_create_session(direct_peer_addr, upstream_addr).await {
            Ok(session) => session,
            Err(_) => {
                log_udp(&UdpLogEntry::failure(
                    self.listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    "SessionLimitReached",
                ))
                .await;
                return;
            }
        };

        let send_result = session.upstream_socket.send_to(data, session.upstream_addr).await;
        Self::record_upstream_send_result(
            send_result,
            &session,
            self.listener_port,
            direct_peer_ip,
            direct_peer_port,
        )
        .await;
    }

    /// Get or create a client session.
    ///
    /// Returns `Err(())` if the session limit is reached, preventing resource
    /// exhaustion from UDP reflection/amplification attacks.
    async fn get_or_create_session(
        &self,
        direct_peer_addr: std::net::SocketAddr,
        upstream_addr: std::net::SocketAddr,
    ) -> Result<Arc<ClientSession>, ()> {
        if let Some(session) = self.client_sessions.get(&direct_peer_addr) {
            return Ok(session.value().clone());
        }

        let session_permit = match self.session_permits.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                if let Some(session) = self.client_sessions.get(&direct_peer_addr) {
                    return Ok(session.value().clone());
                }
                return Err(());
            }
        };

        // Bind the upstream socket to the address family of the resolved upstream.
        // An IPv4-only socket cannot send to an IPv6 backend (send_to fails with
        // EINVAL), which would silently drop every packet for the session.
        let upstream_socket = match UdpSocket::bind(upstream_bind_addr(upstream_addr)).await {
            Ok(socket) => Arc::new(socket),
            Err(_) => return Err(()),
        };

        let session = Arc::new(ClientSession {
            session_permit: Mutex::new(Some(session_permit)),
            upstream_socket: upstream_socket.clone(),
            upstream_addr,
            last_activity: Arc::new(Mutex::new(Instant::now())),
            session_start: Instant::now(),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
        });

        let session = match self.client_sessions.entry(direct_peer_addr) {
            Entry::Occupied(existing) => return Ok(existing.get().clone()),
            Entry::Vacant(vacant) => vacant.insert(session).clone(),
        };

        let sessions_ref = self.client_sessions.clone();
        let downstream = self.socket.clone();
        let sess = session.clone();
        let cancel = self.cancel_token.clone();
        let listener_port = self.listener_port;

        tokio::spawn(async move {
            Self::handle_upstream_packets(direct_peer_addr, downstream, sessions_ref, sess, cancel, listener_port)
                .await;
        });

        Ok(session)
    }

    /// Handle packets received from upstream for a specific client.
    ///
    /// Exits when the session is removed, the cancel token fires, or a
    /// socket error occurs.
    async fn handle_upstream_packets(
        direct_peer_addr: std::net::SocketAddr,
        downstream_socket: Arc<UdpSocket>,
        client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
        session: Arc<ClientSession>,
        cancel: CancellationToken,
        listener_port: u16,
    ) {
        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            if !client_sessions.contains_key(&direct_peer_addr) {
                break;
            }

            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                result = tokio::time::timeout(Duration::from_secs(1), session.upstream_socket.recv_from(&mut buf)) => {
                    match result {
                        Ok(Ok((len, _))) => {
                            let send_result = downstream_socket.send_to(&buf[..len], direct_peer_addr).await;
                            Self::record_downstream_send_result(
                                send_result,
                                &session,
                                listener_port,
                                direct_peer_addr,
                            )
                            .await;
                        }
                        Ok(Err(_)) => break,
                        Err(_) => continue,
                    }
                }
            }
        }
    }

    async fn record_upstream_send_result(
        result: std::io::Result<usize>,
        session: &ClientSession,
        listener_port: u16,
        direct_peer_ip: String,
        direct_peer_port: u16,
    ) {
        match result {
            Ok(bytes_sent) => {
                session.packets_sent.fetch_add(1, Ordering::Relaxed);
                session.bytes_sent.fetch_add(bytes_sent as u64, Ordering::Relaxed);
                *session.last_activity.lock() = Instant::now();
            }
            Err(_) => {
                log_udp(&UdpLogEntry::failure(
                    listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    "UpstreamSendError",
                ))
                .await;
            }
        }
    }

    async fn record_downstream_send_result(
        result: std::io::Result<usize>,
        session: &ClientSession,
        listener_port: u16,
        direct_peer_addr: std::net::SocketAddr,
    ) {
        match result {
            Ok(bytes_sent) => {
                session.packets_received.fetch_add(1, Ordering::Relaxed);
                session.bytes_received.fetch_add(bytes_sent as u64, Ordering::Relaxed);
                *session.last_activity.lock() = Instant::now();
            }
            Err(_) => {
                log_udp(&UdpLogEntry::failure(
                    listener_port,
                    crate::ctx::canonicalize_ip(direct_peer_addr.ip()),
                    direct_peer_addr.port(),
                    "DownstreamSendError",
                ))
                .await;
            }
        }
    }

    /// Periodically clean up inactive sessions.
    ///
    /// Exits when `cancel_token` is cancelled.
    async fn session_cleanup_loop(&self) {
        loop {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    self.cleanup_expired_sessions().await;
                }
            }
        }
    }

    async fn cleanup_expired_sessions(&self) {
        let now = Instant::now();
        let mut to_remove = Vec::new();

        for entry in self.client_sessions.iter() {
            let last_activity = *entry.value().last_activity.lock();
            if now.saturating_duration_since(last_activity) > SESSION_TIMEOUT {
                to_remove.push((*entry.key(), entry.value().clone()));
            }
        }

        for (direct_peer_addr, session) in to_remove {
            let log_entry = UdpLogEntry::new(
                self.listener_port,
                crate::ctx::canonicalize_ip(direct_peer_addr.ip()),
                direct_peer_addr.port(),
                Some(session.upstream_addr.to_string()),
                session.session_start,
            )
            .with_stats(
                session.packets_sent.load(Ordering::Relaxed),
                session.packets_received.load(Ordering::Relaxed),
                session.bytes_sent.load(Ordering::Relaxed),
                session.bytes_received.load(Ordering::Relaxed),
            );

            log_udp(&log_entry).await;
            if let Some((_, removed)) = self.client_sessions.remove(&direct_peer_addr) {
                removed.release_permit();
            }
        }
    }

    /// Visible for testing: current session count.
    #[cfg(test)]
    pub(crate) fn session_count(&self) -> usize {
        self.client_sessions.len()
    }

    fn allow_loopback_upstream(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .map(|s| s.allow_loopback_upstream)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::udp::UdpPortRouteManager;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn make_test_listener_key(port: u16) -> ListenerKey {
        ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port), "UDP")
    }

    fn make_test_config() -> Arc<EdgionGatewayConfig> {
        let json = serde_json::json!({
            "apiVersion": "edgion.io/v1alpha1",
            "kind": "EdgionGatewayConfig",
            "metadata": { "name": "test-config" },
            "spec": {}
        });
        Arc::new(serde_json::from_value(json).expect("test config"))
    }

    #[test]
    fn test_cancel_token_propagation() {
        let cancel = CancellationToken::new();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn test_session_limit_constant() {
        assert_eq!(MAX_UDP_SESSIONS, 10000);
    }

    #[test]
    fn test_upstream_bind_addr_matches_family() {
        // IPv4 upstream → IPv4 wildcard bind.
        let v4: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(upstream_bind_addr(v4).is_ipv4());
        // IPv6 upstream → IPv6 wildcard bind, so send_to does not fail with EINVAL.
        let v6: SocketAddr = "[2001:db8::1]:8080".parse().unwrap();
        assert!(upstream_bind_addr(v6).is_ipv6());
    }

    #[test]
    fn test_session_timeout_constant() {
        assert_eq!(SESSION_TIMEOUT, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_serve_exits_on_cancel() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(
            19998,
            make_test_listener_key(19998),
            manager,
            config,
            socket,
            cancel.clone(),
        ));

        let handle = tokio::spawn({
            let proxy = proxy.clone();
            async move { proxy.serve().await }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("serve() should exit after cancel")
            .expect("serve() should not panic");
    }

    #[tokio::test]
    async fn test_cleanup_loop_exits_on_cancel() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(
            19997,
            make_test_listener_key(19997),
            manager,
            config,
            socket,
            cancel.clone(),
        ));

        let handle = tokio::spawn({
            let proxy = proxy.clone();
            async move { proxy.session_cleanup_loop().await }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("cleanup_loop should exit after cancel")
            .expect("cleanup_loop should not panic");
    }

    #[tokio::test]
    async fn test_session_creation_and_limit() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(
            19996,
            make_test_listener_key(19996),
            manager,
            config,
            socket,
            cancel,
        ));

        assert_eq!(proxy.session_count(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(
            19995,
            make_test_listener_key(19995),
            manager,
            config,
            socket,
            cancel,
        ));

        // Manually insert an expired session
        let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let expired_time = Instant::now() - Duration::from_secs(120);
        let session = Arc::new(ClientSession {
            session_permit: Mutex::new(Some(
                proxy
                    .session_permits
                    .clone()
                    .try_acquire_owned()
                    .expect("test session permit"),
            )),
            upstream_socket,
            upstream_addr: "127.0.0.1:8080".parse().unwrap(),
            last_activity: Arc::new(Mutex::new(expired_time)),
            session_start: expired_time,
            packets_sent: Arc::new(AtomicU64::new(5)),
            packets_received: Arc::new(AtomicU64::new(3)),
            bytes_sent: Arc::new(AtomicU64::new(500)),
            bytes_received: Arc::new(AtomicU64::new(300)),
        });

        let direct_peer_addr: std::net::SocketAddr = "192.168.1.1:12345".parse().unwrap();
        proxy.client_sessions.insert(direct_peer_addr, session);
        assert_eq!(proxy.session_count(), 1);

        proxy.cleanup_expired_sessions().await;
        assert_eq!(proxy.session_count(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_keeps_active_sessions() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(
            19994,
            make_test_listener_key(19994),
            manager,
            config,
            socket,
            cancel,
        ));

        // Insert an active session (recent last_activity)
        let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let session = Arc::new(ClientSession {
            session_permit: Mutex::new(Some(
                proxy
                    .session_permits
                    .clone()
                    .try_acquire_owned()
                    .expect("test session permit"),
            )),
            upstream_socket,
            upstream_addr: "127.0.0.1:8080".parse().unwrap(),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            session_start: Instant::now(),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
        });

        let direct_peer_addr: std::net::SocketAddr = "192.168.1.2:12346".parse().unwrap();
        proxy.client_sessions.insert(direct_peer_addr, session);
        assert_eq!(proxy.session_count(), 1);

        proxy.cleanup_expired_sessions().await;
        assert_eq!(proxy.session_count(), 1);
    }
}
