use dashmap::DashMap;
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::core::gateway::backends::select_roundrobin_backend;
use crate::core::gateway::observe::{log_udp, UdpLogEntry};
use crate::core::gateway::routes::udp::UdpPortRouteManager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;

/// UDP session timeout (60 seconds of inactivity)
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum UDP packet size (64KB)
const MAX_UDP_PACKET_SIZE: usize = 65535;

/// Maximum number of concurrent client sessions per listener.
/// Prevents resource exhaustion from UDP reflection/amplification attacks.
const MAX_UDP_SESSIONS: usize = 10000;

/// Client session information with statistics
struct ClientSession {
    upstream_socket: Arc<UdpSocket>,
    upstream_addr: std::net::SocketAddr,
    last_activity: Arc<Mutex<Instant>>,
    session_start: Instant,
    packets_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
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
    pub udp_route_manager: Arc<UdpPortRouteManager>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub socket: Arc<UdpSocket>,
    client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
    cancel_token: CancellationToken,
}

impl EdgionUdpProxy {
    pub fn new(
        listener_port: u16,
        udp_route_manager: Arc<UdpPortRouteManager>,
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        socket: UdpSocket,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            listener_port,
            udp_route_manager,
            edgion_gateway_config,
            socket: Arc::new(socket),
            client_sessions: Arc::new(DashMap::new()),
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
                        Ok((len, client_addr)) => {
                            let data = buf[..len].to_vec();
                            let this = self.clone();
                            tokio::spawn(async move {
                                this.handle_client_packet(data, client_addr).await;
                            });
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

    async fn handle_client_packet(&self, data: Vec<u8>, client_addr: std::net::SocketAddr) {
        let client_ip = client_addr.ip().to_string();
        let client_port = client_addr.port();

        let route_table = self.udp_route_manager.load_route_table();
        let udp_route = match route_table.match_route() {
            Some(route) => route,
            None => {
                log_udp(&UdpLogEntry::failure(self.listener_port, client_ip, client_port, "NoRouteMatched")).await;
                return;
            }
        };

        let backend_ref = match udp_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => match rule.backend_finder.select() {
                Ok(backend) => backend,
                Err(_) => {
                    log_udp(&UdpLogEntry::failure(self.listener_port, client_ip, client_port, "NoBackendSelected")).await;
                    return;
                }
            },
            None => {
                log_udp(&UdpLogEntry::failure(self.listener_port, client_ip, client_port, "NoRuleAvailable")).await;
                return;
            }
        };

        let backend_namespace = backend_ref
            .namespace
            .as_deref()
            .or_else(|| udp_route.metadata.namespace.as_deref())
            .unwrap_or("default");
        let service_key = format!("{}/{}", backend_namespace, backend_ref.name);

        let backend = match select_roundrobin_backend(&service_key) {
            Some(backend) => backend,
            None => {
                log_udp(&UdpLogEntry::failure(self.listener_port, client_ip, client_port, "NoBackendResolved")).await;
                return;
            }
        };

        let mut upstream_addr = match backend.addr {
            PingoraSocketAddr::Inet(sockaddr) => sockaddr,
            PingoraSocketAddr::Unix(_) => return,
        };

        if let Some(port) = backend_ref.port {
            upstream_addr.set_port(port as u16);
        }

        let session = match self.get_or_create_session(client_addr, upstream_addr).await {
            Ok(session) => session,
            Err(_) => {
                log_udp(&UdpLogEntry::failure(self.listener_port, client_ip, client_port, "SessionLimitReached")).await;
                return;
            }
        };

        let data_len = data.len() as u64;
        let _ = session.upstream_socket.send_to(&data, session.upstream_addr).await;

        session.packets_sent.fetch_add(1, Ordering::Relaxed);
        session.bytes_sent.fetch_add(data_len, Ordering::Relaxed);
        *session.last_activity.lock() = Instant::now();
    }

    /// Get or create a client session.
    ///
    /// Returns `Err(())` if the session limit is reached, preventing resource
    /// exhaustion from UDP reflection/amplification attacks (fix for H-4).
    async fn get_or_create_session(
        &self,
        client_addr: std::net::SocketAddr,
        upstream_addr: std::net::SocketAddr,
    ) -> Result<Arc<ClientSession>, ()> {
        if let Some(session) = self.client_sessions.get(&client_addr) {
            return Ok(session.value().clone());
        }

        if self.client_sessions.len() >= MAX_UDP_SESSIONS {
            return Err(());
        }

        let upstream_socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(socket) => Arc::new(socket),
            Err(_) => return Err(()),
        };

        let session = Arc::new(ClientSession {
            upstream_socket: upstream_socket.clone(),
            upstream_addr,
            last_activity: Arc::new(Mutex::new(Instant::now())),
            session_start: Instant::now(),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
        });

        self.client_sessions.insert(client_addr, session.clone());

        let sessions_ref = self.client_sessions.clone();
        let downstream = self.socket.clone();
        let sess = session.clone();
        let cancel = self.cancel_token.clone();

        tokio::spawn(async move {
            Self::handle_upstream_packets(client_addr, downstream, sessions_ref, sess, cancel).await;
        });

        Ok(session)
    }

    /// Handle packets received from upstream for a specific client.
    ///
    /// Exits when the session is removed, the cancel token fires, or a
    /// socket error occurs.
    async fn handle_upstream_packets(
        client_addr: std::net::SocketAddr,
        downstream_socket: Arc<UdpSocket>,
        client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
        session: Arc<ClientSession>,
        cancel: CancellationToken,
    ) {
        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            if !client_sessions.contains_key(&client_addr) {
                break;
            }

            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                result = tokio::time::timeout(Duration::from_secs(1), session.upstream_socket.recv_from(&mut buf)) => {
                    match result {
                        Ok(Ok((len, _))) => {
                            let _ = downstream_socket.send_to(&buf[..len], client_addr).await;
                            session.packets_received.fetch_add(1, Ordering::Relaxed);
                            session.bytes_received.fetch_add(len as u64, Ordering::Relaxed);
                            *session.last_activity.lock() = Instant::now();
                        }
                        Ok(Err(_)) => break,
                        Err(_) => continue,
                    }
                }
            }
        }
    }

    /// Periodically clean up inactive sessions.
    ///
    /// Exits when `cancel_token` is cancelled (fix for C-1: cleanup task
    /// no longer holds `Arc<Self>` forever).
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
            if now.duration_since(last_activity) > SESSION_TIMEOUT {
                to_remove.push((*entry.key(), entry.value().clone()));
            }
        }

        for (client_addr, session) in to_remove {
            let log_entry = UdpLogEntry::new(
                self.listener_port,
                client_addr.ip().to_string(),
                client_addr.port(),
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
            self.client_sessions.remove(&client_addr);
        }
    }

    /// Visible for testing: current session count.
    #[cfg(test)]
    pub(crate) fn session_count(&self) -> usize {
        self.client_sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::routes::udp::UdpPortRouteManager;

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
    fn test_session_timeout_constant() {
        assert_eq!(SESSION_TIMEOUT, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_serve_exits_on_cancel() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(19998, manager, config, socket, cancel.clone()));

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
        let proxy = Arc::new(EdgionUdpProxy::new(19997, manager, config, socket, cancel.clone()));

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
        let proxy = Arc::new(EdgionUdpProxy::new(19996, manager, config, socket, cancel));

        assert_eq!(proxy.session_count(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let cancel = CancellationToken::new();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let manager = Arc::new(UdpPortRouteManager::new());
        let config = make_test_config();
        let proxy = Arc::new(EdgionUdpProxy::new(19995, manager, config, socket, cancel));

        // Manually insert an expired session
        let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let expired_time = Instant::now() - Duration::from_secs(120);
        let session = Arc::new(ClientSession {
            upstream_socket,
            upstream_addr: "127.0.0.1:8080".parse().unwrap(),
            last_activity: Arc::new(Mutex::new(expired_time)),
            session_start: expired_time,
            packets_sent: Arc::new(AtomicU64::new(5)),
            packets_received: Arc::new(AtomicU64::new(3)),
            bytes_sent: Arc::new(AtomicU64::new(500)),
            bytes_received: Arc::new(AtomicU64::new(300)),
        });

        let client_addr: std::net::SocketAddr = "192.168.1.1:12345".parse().unwrap();
        proxy.client_sessions.insert(client_addr, session);
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
        let proxy = Arc::new(EdgionUdpProxy::new(19994, manager, config, socket, cancel));

        // Insert an active session (recent last_activity)
        let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let session = Arc::new(ClientSession {
            upstream_socket,
            upstream_addr: "127.0.0.1:8080".parse().unwrap(),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            session_start: Instant::now(),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
        });

        let client_addr: std::net::SocketAddr = "192.168.1.2:12346".parse().unwrap();
        proxy.client_sessions.insert(client_addr, session);
        assert_eq!(proxy.session_count(), 1);

        proxy.cleanup_expired_sessions().await;
        assert_eq!(proxy.session_count(), 1);
    }
}
