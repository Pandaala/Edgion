use crate::plugins::publication::global_publisher;
use crate::plugins::stream::connection_filter_bridge::resolve_route_owner_scope;
use dashmap::{mapref::entry::Entry, DashMap};
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::backends::discovery::{
    get_service_discovery_coordinator, ServicePublishedState, ServiceSectionInvalid, ServiceSectionKey,
    ServiceSelectorGeneration, ServiceUnavailableReason,
};
use crate::backends::{validate_endpoint_backend_addr, L4BackendRejection, L4ResolveError};
use crate::observe::{log_udp, UdpLogEntry};
use crate::routes::common::{
    L4_BACKEND_REF_DENIED_STATUS, L4_BACKEND_UNSUPPORTED_KIND_STATUS, UNSAFE_UPSTREAM_ADDRESS_LOG,
};
use crate::routes::tcp::edgion_tcp::{check_stream_plugins, StreamPluginCheck};
use crate::routes::udp::{UdpPortRouteManager, UdpRouteUnit};
use crate::runtime::server::downstream_connections::{ConnectionCounter, ConnectionSlot};
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

/// Stable UDP failure-status tokens. Plugin denials keep their own
/// `&'static str` reason and do not go through this table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpStatus {
    NoRouteMatched,
    NoRuleAvailable,
    NoBackendSelected,
    RefDenied,
    UnsupportedBackend,
    NoBackendResolved,
    UnsafeUpstreamAddress,
    SessionLimitReached,
    UpstreamSocketError,
    UpstreamSendError,
    DownstreamSendError,
    UpstreamEndpointRemoved,
}

impl UdpStatus {
    /// Stable string label for this status, used as the UDP access log
    /// `status` field. Same contract as `TcpStatus::as_str`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRouteMatched => "NoRouteMatched",
            Self::NoRuleAvailable => "NoRuleAvailable",
            Self::NoBackendSelected => "NoBackendSelected",
            Self::RefDenied => L4_BACKEND_REF_DENIED_STATUS,
            Self::UnsupportedBackend => L4_BACKEND_UNSUPPORTED_KIND_STATUS,
            Self::NoBackendResolved => "NoBackendResolved",
            Self::UnsafeUpstreamAddress => UNSAFE_UPSTREAM_ADDRESS_LOG,
            Self::SessionLimitReached => "SessionLimitReached",
            Self::UpstreamSocketError => "UpstreamSocketError",
            Self::UpstreamSendError => "UpstreamSendError",
            Self::DownstreamSendError => "DownstreamSendError",
            Self::UpstreamEndpointRemoved => "UpstreamEndpointRemoved",
        }
    }
}

fn acquire_session_permit(permits: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, UdpStatus> {
    permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| UdpStatus::SessionLimitReached)
}

/// Keep the admitted permit with the initialized socket, or release it on failure.
fn finish_upstream_socket<T>(
    permit: OwnedSemaphorePermit,
    result: std::io::Result<T>,
) -> Result<(OwnedSemaphorePermit, T), UdpStatus> {
    let socket = result.map_err(|_| UdpStatus::UpstreamSocketError)?;
    Ok((permit, socket))
}

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

/// True when `src` is the session's pinned upstream: canonical IP and port both match.
///
/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) collapses to native IPv4 so a family-matched
/// IPv4 pin still accepts a mapped source. Wrong IP and wrong port fail independently.
/// Unpinned datagrams must not be forwarded, counted, or used to refresh idle time.
fn is_pinned_upstream_source(src: std::net::SocketAddr, pinned: std::net::SocketAddr) -> bool {
    src.ip().to_canonical() == pinned.ip().to_canonical() && src.port() == pinned.port()
}

/// Service section that supplied a session's pinned upstream endpoint.
struct PinnedServiceEndpoint {
    service_key: Box<str>,
    section: ServiceSectionKey,
    /// Set when the address came from a per-session OS lookup of this
    /// hostname (`system`-mode `ExternalName`): no published slot holds it, so
    /// the session stays only while the Service is still a `system`-mode
    /// `ExternalName` of the same hostname with the pinned port.
    per_session_hostname: Option<Box<str>>,
}

/// True only when the published discovery generation proves the pinned endpoint
/// is gone: the Service is absent, its section port was removed, or the section
/// no longer publishes the address. Indeterminate states keep the session. A
/// per-session resolved address also retires when the Service stops being a
/// `system`-mode `ExternalName` of the same hostname.
fn pinned_endpoint_removed(
    generation: &ServiceSelectorGeneration,
    pinned: &PinnedServiceEndpoint,
    upstream_addr: std::net::SocketAddr,
) -> bool {
    match generation.service(&pinned.service_key) {
        None | Some(ServicePublishedState::Unavailable(ServiceUnavailableReason::MissingService)) => true,
        Some(ServicePublishedState::Unavailable(ServiceUnavailableReason::InvalidEpoch)) => false,
        Some(ServicePublishedState::Available(service)) if pinned.per_session_hostname.is_some() => {
            crate::backends::system_mode_hostname(service) != pinned.per_session_hostname.as_deref()
                || matches!(
                    service.resolve_section(&pinned.section),
                    Err(ServiceSectionInvalid::MissingRequestedPort)
                )
        }
        Some(ServicePublishedState::Available(service)) => match service.resolve_section(&pinned.section) {
            Ok(section) => section.slot_for_addr(upstream_addr).is_none(),
            Err(ServiceSectionInvalid::MissingRequestedPort) => true,
            Err(_) => false,
        },
    }
}

/// Client session information with statistics
struct ClientSession {
    cancel_token: CancellationToken,
    _session_permit: OwnedSemaphorePermit,
    /// Route-stage session count; released when the last session handle drops.
    _route_slot: ConnectionSlot,
    upstream_socket: Arc<UdpSocket>,
    upstream_addr: std::net::SocketAddr,
    pinned_endpoint: PinnedServiceEndpoint,
    lifecycle: Mutex<SessionLifecycle>,
    session_start: Instant,
}

struct SessionLifecycle {
    last_activity: Instant,
    retired: bool,
    packets_sent: u64,
    packets_received: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

trait UpstreamDatagramReceiver: Send + Sync {
    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> impl Future<Output = std::io::Result<(usize, std::net::SocketAddr)>> + Send + 'a;
}

impl UpstreamDatagramReceiver for UdpSocket {
    async fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> std::io::Result<(usize, std::net::SocketAddr)> {
        UdpSocket::recv_from(self, buf).await
    }
}

/// UDP proxy service for Gateway UDP listeners.
///
/// Each instance is bound to a single listener port with its own per-port
/// `UdpPortRouteManager`. Route updates swap the inner `ArcSwap<UdpRouteTable>`
/// so the `Arc<UdpPortRouteManager>` stays stable.
///
/// Process shutdown cancels the main loop and its children. Listener ports are
/// frozen at startup; route hot reload does not add or remove listener tasks.
pub struct EdgionUdpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub udp_route_manager: Arc<UdpPortRouteManager>,
    pub egc: Arc<crate::config::GatewayConfigHandle>,
    pub socket: Arc<UdpSocket>,
    client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
    session_permits: Arc<Semaphore>,
    /// Live sessions admitted on this listener's route stage. Owned by the
    /// listener's single proxy instance, so it survives config reloads.
    route_sessions: Arc<ConnectionCounter>,
    cancel_token: CancellationToken,
    child_tasks: TaskTracker,
}

impl EdgionUdpProxy {
    pub fn new(
        listener_port: u16,
        listener_key: ListenerKey,
        udp_route_manager: Arc<UdpPortRouteManager>,
        egc: Arc<crate::config::GatewayConfigHandle>,
        socket: UdpSocket,
        cancel_token: CancellationToken,
        child_tasks: TaskTracker,
    ) -> Self {
        Self {
            listener_port,
            listener_key,
            udp_route_manager,
            egc,
            socket: Arc::new(socket),
            client_sessions: Arc::new(DashMap::new()),
            session_permits: Arc::new(Semaphore::new(MAX_UDP_SESSIONS)),
            route_sessions: Arc::new(ConnectionCounter::unlimited()),
            cancel_token,
            child_tasks,
        }
    }

    /// Main service loop — receives packets from clients and handles them.
    ///
    /// Exits when `cancel_token` is cancelled or the socket errors out.
    /// On exit, joins cleanup and receiver tasks before taking terminal session
    /// statistics. Dropping the serve future aborts its owned child tasks.
    pub async fn serve(self: Arc<Self>) -> anyhow::Result<()> {
        let _cancel_on_drop = self.cancel_token.clone().drop_guard();
        let mut children = JoinSet::new();
        let cleanup_self = self.clone();
        children.spawn(self.child_tasks.track_future(async move {
            cleanup_self.session_cleanup_loop().await;
        }));

        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];
        let mut child_error = None;

        loop {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => break,
                result = children.join_next(), if !children.is_empty() => {
                    if let Some(Err(error)) = result {
                        child_error = Some(error);
                        break;
                    }
                }
                result = self.socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, direct_peer_addr)) => {
                            tokio::select! {
                                biased;
                                _ = self.cancel_token.cancelled() => break,
                                _ = self.handle_client_packet(&buf[..len], direct_peer_addr, &mut children) => {}
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        self.cancel_token.cancel();
        drop(buf);
        while let Some(result) = children.join_next().await {
            if let Err(error) = result {
                child_error.get_or_insert(error);
            }
        }
        self.child_tasks.close();
        self.child_tasks.wait().await;
        // No producer or cleanup task can mutate statistics or retire sessions
        // now. Expiry/recreation races during serving are handled separately.
        let peers: Vec<_> = self.client_sessions.iter().map(|entry| *entry.key()).collect();
        for peer in peers {
            if let Some((_, session)) = self.client_sessions.remove(&peer) {
                let log_entry = Self::session_log_entry(self.listener_port, peer, &session);
                drop(session);
                log_udp(&log_entry).await;
            }
        }
        if let Some(error) = child_error {
            return Err(error.into());
        }
        Ok(())
    }

    async fn handle_client_packet(
        &self,
        data: &[u8],
        direct_peer_addr: std::net::SocketAddr,
        children: &mut JoinSet<()>,
    ) {
        // Fast path: an established session already has a pinned upstream and a
        // dedicated upstream socket, so the route pipeline below (ArcSwap load,
        // route match, backend selection, round-robin, SSRF validation) is pure
        // redundant work — its result is discarded for existing sessions anyway,
        // since the send always targets the session's stored `upstream_addr`.
        // Route/policy changes take effect when the session expires
        // (SESSION_TIMEOUT); removal of the pinned endpoint retires the session
        // in the cleanup loop. Either way the next packet creates a new one. Clone the Arc out so the
        // DashMap read guard is dropped before the `.await` below.
        if let Some(session) = self.client_sessions.get(&direct_peer_addr).map(|s| s.value().clone()) {
            let send_result = session.upstream_socket.send_to(data, session.upstream_addr).await;
            Self::record_upstream_send_result(send_result, session, self.listener_port, direct_peer_addr).await;
            return;
        }

        let gateway_config = self.egc.load();
        let direct_peer_ip = crate::ctx::canonicalize_ip(direct_peer_addr.ip());
        let direct_peer_port = direct_peer_addr.port();
        let Some(unit) = select_route_unit(&self.udp_route_manager) else {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                UdpStatus::NoRouteMatched.as_str(),
            ))
            .await;
            return;
        };
        let udp_route = unit.route();

        // Require at least one rule. The compiled runtime (backend_finder,
        // stream-plugin store key) lives on the unit and is built from the
        // first rule, so a route with no rules cannot serve traffic.
        if udp_route.spec.rules.as_ref().and_then(|rules| rules.first()).is_none() {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                UdpStatus::NoRuleAvailable.as_str(),
            ))
            .await;
            return;
        }

        // Established sessions returned above; only new sessions acquire policy.
        let publication = global_publisher().snapshot();
        let Some((owner, scope)) = resolve_route_owner_scope(
            &publication,
            &self.listener_key,
            unit.resolved_parent_refs(),
            "",
            unit.route().metadata.namespace.as_deref().unwrap_or("default"),
            "UDPRoute",
        ) else {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                "plugin policy unavailable",
            ))
            .await;
            return;
        };
        let resource = unit.stream_plugin_store_key().and_then(|key| scope.stream(key));
        drop(scope);
        drop(publication);
        // Counted from here; moved into the new session, or dropped on any
        // earlier exit.
        let (route_slot, route_ordinal) = self.route_sessions.acquire();
        if let StreamPluginCheck::Deny(reason) = check_stream_plugins(
            unit.stream_plugin_store_key(),
            resource.as_deref(),
            &direct_peer_ip,
            self.listener_port,
            route_ordinal,
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

        drop(resource);

        let Ok(selected) = unit.select_backend() else {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                UdpStatus::NoBackendSelected.as_str(),
            ))
            .await;
            return;
        };
        let prep = crate::backends::prepare_l4_service(
            selected,
            Some(owner.gateway_key()),
            &gateway_config,
            crate::backends::L4Transport::Datagram,
            &[],
        )
        .await;
        let (pinned_endpoint, upstream_addr) =
            match crate::backends::policy::resolve_l4_policy_backend(selected, Some(owner.gateway_key()), |target| {
                prep.resolve(target)
            }) {
                Ok((target, backend)) => {
                    let PingoraSocketAddr::Inet(addr) = &backend.inner().addr else {
                        return;
                    };
                    let pinned = PinnedServiceEndpoint {
                        service_key: target.key().into(),
                        section: target.section().clone(),
                        per_session_hostname: prep.direct_hostname().map(Box::from),
                    };
                    (pinned, *addr)
                }
                Err(error) => {
                    let status = match error {
                        L4ResolveError::Rejected(L4BackendRejection::Denied) => UdpStatus::RefDenied,
                        L4ResolveError::Rejected(L4BackendRejection::Invalid) => UdpStatus::UnsupportedBackend,
                        L4ResolveError::Unavailable(_) => UdpStatus::NoBackendResolved,
                    };
                    log_udp(&UdpLogEntry::failure(
                        self.listener_port,
                        direct_peer_ip,
                        direct_peer_port,
                        status.as_str(),
                    ))
                    .await;
                    return;
                }
            };

        if validate_endpoint_backend_addr(
            &PingoraSocketAddr::Inet(upstream_addr),
            gateway_config.allow_loopback_upstream,
        )
        .is_err()
        {
            log_udp(&UdpLogEntry::failure(
                self.listener_port,
                direct_peer_ip,
                direct_peer_port,
                UdpStatus::UnsafeUpstreamAddress.as_str(),
            ))
            .await;
            return;
        }

        let session = match self
            .get_or_create_session(direct_peer_addr, upstream_addr, pinned_endpoint, route_slot, children)
            .await
        {
            Ok(session) => session,
            Err(status) => {
                log_udp(&UdpLogEntry::failure(
                    self.listener_port,
                    direct_peer_ip,
                    direct_peer_port,
                    status.as_str(),
                ))
                .await;
                return;
            }
        };

        let send_result = session.upstream_socket.send_to(data, session.upstream_addr).await;
        Self::record_upstream_send_result(send_result, session, self.listener_port, direct_peer_addr).await;
    }

    /// Get or create a client session.
    ///
    /// Admission failure returns `SessionLimitReached`; upstream socket
    /// initialization failure returns `UpstreamSocketError` and releases admission.
    async fn get_or_create_session(
        &self,
        direct_peer_addr: std::net::SocketAddr,
        upstream_addr: std::net::SocketAddr,
        pinned_endpoint: PinnedServiceEndpoint,
        route_slot: ConnectionSlot,
        children: &mut JoinSet<()>,
    ) -> Result<Arc<ClientSession>, UdpStatus> {
        if let Some(session) = self.client_sessions.get(&direct_peer_addr) {
            return Ok(session.value().clone());
        }

        let session_permit = match acquire_session_permit(&self.session_permits) {
            Ok(permit) => permit,
            Err(status) => {
                if let Some(session) = self.client_sessions.get(&direct_peer_addr) {
                    return Ok(session.value().clone());
                }
                return Err(status);
            }
        };

        // Bind the upstream socket to the address family of the resolved upstream.
        // An IPv4-only socket cannot send to an IPv6 backend (send_to fails with
        // EINVAL), which would silently drop every packet for the session.
        let socket_result = UdpSocket::bind(upstream_bind_addr(upstream_addr)).await;
        let (session_permit, upstream_socket) = finish_upstream_socket(session_permit, socket_result)?;
        let upstream_socket = Arc::new(upstream_socket);

        let session = Arc::new(ClientSession {
            cancel_token: self.cancel_token.child_token(),
            _session_permit: session_permit,
            _route_slot: route_slot,
            upstream_socket,
            upstream_addr,
            pinned_endpoint,
            lifecycle: Mutex::new(SessionLifecycle {
                last_activity: Instant::now(),
                retired: false,
                packets_sent: 0,
                packets_received: 0,
                bytes_sent: 0,
                bytes_received: 0,
            }),
            session_start: Instant::now(),
        });

        let session = match self.client_sessions.entry(direct_peer_addr) {
            Entry::Occupied(existing) => return Ok(existing.get().clone()),
            Entry::Vacant(vacant) => vacant.insert(session).clone(),
        };

        let sessions_ref = self.client_sessions.clone();
        let downstream = self.socket.clone();
        let sess = session.clone();
        let upstream_receiver = session.upstream_socket.clone();
        let listener_port = self.listener_port;

        children.spawn(self.child_tasks.track_future(async move {
            Self::handle_upstream_packets(
                direct_peer_addr,
                downstream,
                upstream_receiver,
                sessions_ref,
                sess,
                listener_port,
            )
            .await;
        }));

        Ok(session)
    }

    /// Handle packets received from upstream for a specific client.
    ///
    /// Exits when the session is removed, the cancel token fires, or a
    /// socket error occurs.
    async fn handle_upstream_packets<R: UpstreamDatagramReceiver>(
        direct_peer_addr: std::net::SocketAddr,
        downstream_socket: Arc<UdpSocket>,
        upstream_receiver: Arc<R>,
        client_sessions: Arc<DashMap<std::net::SocketAddr, Arc<ClientSession>>>,
        session: Arc<ClientSession>,
        listener_port: u16,
    ) {
        let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE];

        loop {
            tokio::select! {
                biased;
                _ = session.cancel_token.cancelled() => break,
                result = upstream_receiver.recv_from(&mut buf) => {
                    match result {
                        Ok((len, src)) => {
                            if session.lifecycle.lock().retired {
                                break;
                            }
                            if !is_pinned_upstream_source(src, session.upstream_addr) {
                                continue;
                            }
                            let send_result = tokio::select! {
                                biased;
                                _ = session.cancel_token.cancelled() => break,
                                result = downstream_socket.send_to(&buf[..len], direct_peer_addr) => result,
                            };
                            Self::record_downstream_send_result(
                                send_result,
                                &session,
                                listener_port,
                                direct_peer_addr,
                            )
                            .await;
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        if Self::retire_session(&client_sessions, direct_peer_addr, &session, None) {
            let log_entry = Self::session_log_entry(listener_port, direct_peer_addr, &session);
            drop(buf);
            drop(upstream_receiver);
            drop(session);
            log_udp(&log_entry).await;
        }
    }

    async fn record_upstream_send_result(
        result: std::io::Result<usize>,
        session: Arc<ClientSession>,
        listener_port: u16,
        direct_peer_addr: std::net::SocketAddr,
    ) {
        match result {
            Ok(bytes_sent) => {
                let mut lifecycle = session.lifecycle.lock();
                if !lifecycle.retired {
                    lifecycle.packets_sent = lifecycle.packets_sent.wrapping_add(1);
                    lifecycle.bytes_sent = lifecycle.bytes_sent.wrapping_add(bytes_sent as u64);
                    lifecycle.last_activity = Instant::now();
                }
            }
            Err(_) => {
                drop(session);
                log_udp(&UdpLogEntry::failure(
                    listener_port,
                    crate::ctx::canonicalize_ip(direct_peer_addr.ip()),
                    direct_peer_addr.port(),
                    UdpStatus::UpstreamSendError.as_str(),
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
                let mut lifecycle = session.lifecycle.lock();
                if !lifecycle.retired {
                    lifecycle.packets_received = lifecycle.packets_received.wrapping_add(1);
                    lifecycle.bytes_received = lifecycle.bytes_received.wrapping_add(bytes_sent as u64);
                    lifecycle.last_activity = Instant::now();
                }
            }
            Err(_) => {
                log_udp(&UdpLogEntry::failure(
                    listener_port,
                    crate::ctx::canonicalize_ip(direct_peer_addr.ip()),
                    direct_peer_addr.port(),
                    UdpStatus::DownstreamSendError.as_str(),
                ))
                .await;
            }
        }
    }

    /// Periodically clean up inactive sessions and sessions whose pinned
    /// upstream endpoint was removed from service discovery.
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
        self.cleanup_sessions(get_service_discovery_coordinator().load_generation())
            .await;
    }

    async fn cleanup_sessions(&self, generation: Arc<ServiceSelectorGeneration>) {
        let now = Instant::now();
        let to_remove = Self::collect_retirement_candidates(&self.client_sessions, &generation, now);
        drop(generation);

        for (direct_peer_addr, session, reason) in to_remove {
            // Idle candidates recheck activity at removal; a removed endpoint retires unconditionally.
            let expiry_check = matches!(reason, RetireReason::Idle).then_some(now);
            if Self::retire_session(&self.client_sessions, direct_peer_addr, &session, expiry_check) {
                let mut log_entry = Self::session_log_entry(self.listener_port, direct_peer_addr, &session);
                if matches!(reason, RetireReason::EndpointRemoved) {
                    log_entry.status = Some(UdpStatus::UpstreamEndpointRemoved.as_str());
                }
                drop(session);
                log_udp(&log_entry).await;
            }
        }
    }

    fn collect_retirement_candidates(
        client_sessions: &DashMap<std::net::SocketAddr, Arc<ClientSession>>,
        generation: &ServiceSelectorGeneration,
        now: Instant,
    ) -> Vec<(std::net::SocketAddr, Arc<ClientSession>, RetireReason)> {
        let mut candidates = Vec::new();
        for entry in client_sessions.iter() {
            let session = entry.value();
            let reason = if pinned_endpoint_removed(generation, &session.pinned_endpoint, session.upstream_addr) {
                RetireReason::EndpointRemoved
            } else if now.saturating_duration_since(session.lifecycle.lock().last_activity) > SESSION_TIMEOUT {
                RetireReason::Idle
            } else {
                continue;
            };
            candidates.push((*entry.key(), session.clone(), reason));
        }
        candidates
    }

    /// Retire only this generation while holding the map entry and lifecycle lock.
    /// A send that refreshed activity after collection wins over expiration.
    fn retire_session(
        client_sessions: &DashMap<std::net::SocketAddr, Arc<ClientSession>>,
        direct_peer_addr: std::net::SocketAddr,
        session: &Arc<ClientSession>,
        expiry_check: Option<Instant>,
    ) -> bool {
        let Entry::Occupied(occupied) = client_sessions.entry(direct_peer_addr) else {
            return false;
        };
        if !Arc::ptr_eq(occupied.get(), session) {
            return false;
        }
        let mut lifecycle = session.lifecycle.lock();
        if lifecycle.retired
            || expiry_check.is_some_and(|now| now.saturating_duration_since(lifecycle.last_activity) <= SESSION_TIMEOUT)
        {
            return false;
        }
        lifecycle.retired = true;
        drop(lifecycle);
        occupied.remove();
        session.cancel_token.cancel();
        true
    }

    fn session_log_entry(
        listener_port: u16,
        direct_peer_addr: std::net::SocketAddr,
        session: &ClientSession,
    ) -> UdpLogEntry {
        let (packets_sent, packets_received, bytes_sent, bytes_received) = {
            let lifecycle = session.lifecycle.lock();
            (
                lifecycle.packets_sent,
                lifecycle.packets_received,
                lifecycle.bytes_sent,
                lifecycle.bytes_received,
            )
        };
        UdpLogEntry::new(
            listener_port,
            crate::ctx::canonicalize_ip(direct_peer_addr.ip()),
            direct_peer_addr.port(),
            Some(session.upstream_addr.to_string()),
            session.session_start,
        )
        .with_stats(packets_sent, packets_received, bytes_sent, bytes_received)
    }

    /// Visible for testing: current session count.
    #[cfg(test)]
    pub(crate) fn session_count(&self) -> usize {
        self.client_sessions.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetireReason {
    Idle,
    EndpointRemoved,
}

// Retain only the selected unit across admission I/O, not the full route table.
fn select_route_unit(manager: &UdpPortRouteManager) -> Option<Arc<UdpRouteUnit>> {
    manager.load_route_table().match_route()
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/udp/edgion_udp_test.rs"]
mod tests;
