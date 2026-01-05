use dashmap::DashMap;
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

use crate::core::backends::endpoint_slice::get_roundrobin_store;
use crate::core::observe::{log_udp, UdpLogEntry};
use crate::core::routes::udp_routes::GatewayUdpRoutes;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;

/// UDP session timeout (60 seconds of inactivity)
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum UDP packet size (64KB)
const MAX_UDP_PACKET_SIZE: usize = 65535;

/// Client session information with statistics
struct ClientSession {
    upstream_socket: Arc<UdpSocket>,
    upstream_addr: std::net::SocketAddr,
    last_activity: Arc<Mutex<Instant>>,
    session_start: Instant,
    // Atomic statistics for thread-safe updates
    packets_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
}

/// UDP proxy service
pub struct EdgionUdp {
    pub gateway_name: String,
    pub gateway_namespace: Option<String>,
    pub listener_name: String, // Listener name (sectionName in UDPRoute)
    pub listener_port: u16,
    pub gateway_udp_routes: Arc<GatewayUdpRoutes>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub socket: Arc<UdpSocket>,
    /// Client address -> session mapping
    /// Each client gets its own upstream socket for proper NAT-like behavior
    client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
}

impl EdgionUdp {
    /// Create a new UDP proxy service
    pub fn new(
        gateway_name: String,
        gateway_namespace: Option<String>,
        listener_name: String,
        listener_port: u16,
        gateway_udp_routes: Arc<GatewayUdpRoutes>,
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        socket: UdpSocket,
    ) -> Self {
        Self {
            gateway_name,
            gateway_namespace,
            listener_name,
            listener_port,
            gateway_udp_routes,
            edgion_gateway_config,
            socket: Arc::new(socket),
            client_sessions: Arc::new(DashMap::new()),
        }
    }

    /// Main service loop - receives packets from clients and handles them
    pub async fn serve(self: Arc<Self>) {
        // Spawn session cleanup task
        let cleanup_self = self.clone();
        tokio::spawn(async move {
            cleanup_self.session_cleanup_loop().await;
        });

        // Main packet receiving loop
        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            match self.socket.recv_from(&mut buf).await {
                Ok((len, client_addr)) => {
                    let data = buf[..len].to_vec();
                    let this = self.clone();

                    // Handle packet asynchronously
                    tokio::spawn(async move {
                        this.handle_client_packet(data, client_addr).await;
                    });
                }
                Err(_) => {
                    // Socket error, break the loop
                    break;
                }
            }
        }
    }

    /// Handle a packet received from a client
    async fn handle_client_packet(&self, data: Vec<u8>, client_addr: std::net::SocketAddr) {
        // 1. Match UDPRoute by listener_name and port
        let udp_route = match self
            .gateway_udp_routes
            .match_route(&self.listener_name, self.listener_port)
        {
            Some(route) => route,
            None => return, // No logging for dropped packets
        };

        // 2. Select backend
        let backend_ref = match udp_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => match rule.backend_finder.select() {
                Ok(backend) => backend,
                Err(_) => return,
            },
            None => return,
        };

        // 3. Build service key for EndpointSlice lookup
        let backend_namespace = backend_ref
            .namespace
            .as_deref()
            .or_else(|| udp_route.metadata.namespace.as_deref())
            .unwrap_or("default");
        let service_key = format!("{}/{}", backend_namespace, backend_ref.name);

        // 4. Resolve backend address via EndpointSlice
        let ep_store = get_roundrobin_store();
        let backend = match ep_store.select_peer(&service_key, b"", 256) {
            Some(backend) => backend,
            None => return,
        };

        // Convert Pingora SocketAddr to std::net::SocketAddr
        let mut upstream_addr = match backend.addr {
            PingoraSocketAddr::Inet(sockaddr) => sockaddr,
            PingoraSocketAddr::Unix(_) => return, // Unix sockets not supported for UDP
        };

        // Override port if specified in backend_ref
        if let Some(port) = backend_ref.port {
            upstream_addr.set_port(port as u16);
        }

        // 5. Get or create client session
        let session = match self.get_or_create_session(client_addr, upstream_addr).await {
            Ok(session) => session,
            Err(_) => return,
        };

        // 6. Forward packet to upstream
        let data_len = data.len() as u64;
        let _ = session.upstream_socket.send_to(&data, session.upstream_addr).await;

        // 7. Update statistics
        session.packets_sent.fetch_add(1, Ordering::Relaxed);
        session.bytes_sent.fetch_add(data_len, Ordering::Relaxed);

        // 8. Update last activity
        *session.last_activity.lock() = Instant::now();
    }

    /// Get or create a client session
    async fn get_or_create_session(
        &self,
        client_addr: std::net::SocketAddr,
        upstream_addr: std::net::SocketAddr,
    ) -> Result<Arc<ClientSession>, ()> {
        // Check if session already exists
        if let Some(session) = self.client_sessions.get(&client_addr) {
            // Update last activity
            return Ok(session.value().clone());
        }

        // Create new upstream socket for this client
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

        // Store session
        self.client_sessions.insert(client_addr, session.clone());

        // Spawn upstream listener for this session
        let client_sessions = self.client_sessions.clone();
        let downstream_socket = self.socket.clone();
        let last_activity = session.last_activity.clone();
        let packets_received = session.packets_received.clone();
        let bytes_received = session.bytes_received.clone();
        tokio::spawn(async move {
            Self::handle_upstream_packets_static(
                client_addr,
                upstream_socket,
                downstream_socket,
                client_sessions,
                last_activity,
                packets_received,
                bytes_received,
            )
            .await;
        });

        Ok(session)
    }

    /// Handle packets received from upstream for a specific client (static method to avoid lifetime issues)
    async fn handle_upstream_packets_static(
        client_addr: std::net::SocketAddr,
        upstream_socket: Arc<UdpSocket>,
        downstream_socket: Arc<UdpSocket>,
        client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
        last_activity: Arc<Mutex<Instant>>,
        packets_received: Arc<AtomicU64>,
        bytes_received: Arc<AtomicU64>,
    ) {
        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            // Check if session still exists
            if !client_sessions.contains_key(&client_addr) {
                // Session has been cleaned up, stop listening
                break;
            }

            // Set a timeout to check session existence periodically
            match tokio::time::timeout(Duration::from_secs(1), upstream_socket.recv_from(&mut buf)).await {
                Ok(Ok((len, _))) => {
                    // Forward packet back to client
                    let _ = downstream_socket.send_to(&buf[..len], client_addr).await;

                    // Update statistics
                    packets_received.fetch_add(1, Ordering::Relaxed);
                    bytes_received.fetch_add(len as u64, Ordering::Relaxed);

                    // Update last activity
                    *last_activity.lock() = Instant::now();
                }
                Ok(Err(_)) => {
                    // Socket error, break the loop
                    break;
                }
                Err(_) => {
                    // Timeout, continue checking
                    continue;
                }
            }
        }
    }

    /// Periodically clean up inactive sessions
    async fn session_cleanup_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let now = Instant::now();
            let mut to_remove = Vec::new();

            // Find inactive sessions
            for entry in self.client_sessions.iter() {
                let last_activity = *entry.value().last_activity.lock();
                if now.duration_since(last_activity) > SESSION_TIMEOUT {
                    to_remove.push((*entry.key(), entry.value().clone()));
                }
            }

            // Remove inactive sessions and log them
            for (client_addr, session) in to_remove {
                // Log session before removal
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

                // Remove session
                self.client_sessions.remove(&client_addr);
            }
        }
    }
}
