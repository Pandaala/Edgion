use super::*;
use crate::routes::udp::UdpPortRouteManager;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV6};
use tokio::sync::oneshot;

type ReceiveResult = std::io::Result<(usize, SocketAddr)>;

struct ControlledReceiver {
    result: Mutex<Option<oneshot::Receiver<ReceiveResult>>>,
    started: Semaphore,
}

impl UpstreamDatagramReceiver for ControlledReceiver {
    async fn recv_from<'a>(&'a self, _buf: &'a mut [u8]) -> ReceiveResult {
        let result = self.result.lock().take().expect("one controlled receive");
        self.started.add_permits(1);
        result.await.expect("controlled receive result")
    }
}

fn controlled_receiver() -> (Arc<ControlledReceiver>, oneshot::Sender<ReceiveResult>) {
    let (send, result) = oneshot::channel();
    (
        Arc::new(ControlledReceiver {
            result: Mutex::new(Some(result)),
            started: Semaphore::new(0),
        }),
        send,
    )
}

fn make_test_listener_key(port: u16) -> ListenerKey {
    ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port), "UDP")
}

async fn make_test_proxy(port: u16) -> Arc<EdgionUdpProxy> {
    Arc::new(EdgionUdpProxy::new(
        port,
        make_test_listener_key(port),
        Arc::new(UdpPortRouteManager::new()),
        crate::config::bind_gateway_config(None),
        UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        CancellationToken::new(),
        TaskTracker::new(),
    ))
}

fn make_test_session(proxy: &EdgionUdpProxy, last_activity: Instant) -> Arc<ClientSession> {
    Arc::new(ClientSession {
        _session_permit: proxy.session_permits.clone().try_acquire_owned().unwrap(),
        upstream_socket: proxy.socket.clone(),
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        lifecycle: Mutex::new(SessionLifecycle {
            last_activity,
            retired: false,
        }),
        session_start: last_activity,
        packets_sent: AtomicU64::new(0),
        packets_received: AtomicU64::new(0),
        bytes_sent: AtomicU64::new(0),
        bytes_received: AtomicU64::new(0),
    })
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
fn session_admission_exhaustion_reports_capacity() {
    let permits = Arc::new(Semaphore::new(1));
    let admitted = acquire_session_permit(&permits).unwrap();
    assert_eq!(permits.available_permits(), 0);
    assert_eq!(
        acquire_session_permit(&permits).unwrap_err(),
        UdpStatus::SessionLimitReached
    );
    drop(admitted);
    assert_eq!(permits.available_permits(), 1);
}

#[test]
fn session_admission_closed_reports_capacity() {
    let permits = Arc::new(Semaphore::new(1));
    permits.close();
    assert_eq!(
        acquire_session_permit(&permits).unwrap_err(),
        UdpStatus::SessionLimitReached
    );
}

#[test]
fn upstream_socket_failure_reports_cause_and_releases_admission() {
    for kind in [
        std::io::ErrorKind::AddrInUse,
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::Other,
    ] {
        let permits = Arc::new(Semaphore::new(1));
        let admitted = acquire_session_permit(&permits).unwrap();
        assert_eq!(permits.available_permits(), 0);
        let result =
            finish_upstream_socket::<()>(admitted, Err(std::io::Error::new(kind, "socket initialization failed")));
        assert_eq!(result.unwrap_err(), UdpStatus::UpstreamSocketError);
        assert_eq!(permits.available_permits(), 1);
        assert!(acquire_session_permit(&permits).is_ok());
    }
}

#[test]
fn upstream_socket_success_retains_admission_until_owner_drops() {
    let permits = Arc::new(Semaphore::new(1));
    let admitted = acquire_session_permit(&permits).unwrap();
    let initialized = finish_upstream_socket(admitted, Ok(())).unwrap();
    assert_eq!(permits.available_permits(), 0);
    assert_eq!(
        acquire_session_permit(&permits).unwrap_err(),
        UdpStatus::SessionLimitReached
    );
    drop(initialized);
    assert_eq!(permits.available_permits(), 1);
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

fn pinned_v4() -> SocketAddr {
    "192.0.2.10:53".parse().unwrap()
}

fn mapped_v4(ip: Ipv4Addr, port: u16) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(ip.to_ipv6_mapped(), port, 0, 0))
}

#[test]
fn pinned_upstream_source_accepts_exact_ip_and_port() {
    assert!(is_pinned_upstream_source(pinned_v4(), pinned_v4()));
}

#[test]
fn pinned_upstream_source_rejects_wrong_port() {
    let src: SocketAddr = "192.0.2.10:54".parse().unwrap();
    assert!(!is_pinned_upstream_source(src, pinned_v4()));
}

#[test]
fn pinned_upstream_source_rejects_wrong_ip() {
    let src: SocketAddr = "192.0.2.11:53".parse().unwrap();
    assert!(!is_pinned_upstream_source(src, pinned_v4()));
}

#[test]
fn pinned_upstream_source_accepts_ipv4_mapped_ipv6_same_host_and_port() {
    let src = mapped_v4(Ipv4Addr::new(192, 0, 2, 10), 53);
    assert!(is_pinned_upstream_source(src, pinned_v4()));
    assert!(is_pinned_upstream_source(pinned_v4(), src));
}

#[test]
fn pinned_upstream_source_rejects_ipv4_mapped_wrong_port_and_wrong_ip() {
    let wrong_port = mapped_v4(Ipv4Addr::new(192, 0, 2, 10), 54);
    let wrong_ip = mapped_v4(Ipv4Addr::new(192, 0, 2, 11), 53);
    assert!(!is_pinned_upstream_source(wrong_port, pinned_v4()));
    assert!(!is_pinned_upstream_source(wrong_ip, pinned_v4()));
}

#[test]
fn pinned_upstream_source_rejects_native_ipv6_mismatch() {
    let pinned: SocketAddr = "[2001:db8::10]:53".parse().unwrap();
    let wrong_ip: SocketAddr = "[2001:db8::11]:53".parse().unwrap();
    let wrong_port: SocketAddr = "[2001:db8::10]:54".parse().unwrap();
    assert!(is_pinned_upstream_source(pinned, pinned));
    assert!(!is_pinned_upstream_source(wrong_ip, pinned));
    assert!(!is_pinned_upstream_source(wrong_port, pinned));
}

#[test]
fn pinned_upstream_source_does_not_treat_ipv4_compatible_as_mapped() {
    let compatible: SocketAddr = "[::192.0.2.10]:53".parse().unwrap();
    assert!(!is_pinned_upstream_source(compatible, pinned_v4()));
}

#[tokio::test]
async fn test_serve_exits_on_cancel() {
    let cancel = CancellationToken::new();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let manager = Arc::new(UdpPortRouteManager::new());
    let config = crate::config::bind_gateway_config(None);
    let proxy = Arc::new(EdgionUdpProxy::new(
        19998,
        make_test_listener_key(19998),
        manager,
        config,
        socket,
        cancel.clone(),
        TaskTracker::new(),
    ));

    // Use the existing test socket as a synthetic session resource; no
    // datagrams or additional network endpoint are needed for shutdown.
    let peer: SocketAddr = "192.0.2.1:12345".parse().unwrap();
    proxy.client_sessions.insert(
        peer,
        Arc::new(ClientSession {
            _session_permit: proxy.session_permits.clone().try_acquire_owned().unwrap(),
            upstream_socket: proxy.socket.clone(),
            upstream_addr: "192.0.2.2:8080".parse().unwrap(),
            lifecycle: Mutex::new(SessionLifecycle {
                last_activity: Instant::now(),
                retired: false,
            }),
            session_start: Instant::now(),
            packets_sent: AtomicU64::new(5),
            packets_received: AtomicU64::new(3),
            bytes_sent: AtomicU64::new(500),
            bytes_received: AtomicU64::new(300),
        }),
    );

    let handle = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.serve().await }
    });

    // Cancellation before the first poll must still reap the cleanup task
    // and finalize sessions; this requires no scheduling delay.
    cancel.cancel();

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("serve() should exit after cancel")
        .expect("serve() should not panic")
        .expect("child tasks should exit successfully");
    assert!(proxy.child_tasks.is_closed());
    assert!(proxy.child_tasks.is_empty());
    assert_eq!(proxy.session_count(), 0);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS);
    assert_eq!(Arc::strong_count(&proxy.socket), 1);
}

#[tokio::test]
async fn test_cleanup_loop_exits_on_cancel() {
    let cancel = CancellationToken::new();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let manager = Arc::new(UdpPortRouteManager::new());
    let config = crate::config::bind_gateway_config(None);
    let proxy = Arc::new(EdgionUdpProxy::new(
        19997,
        make_test_listener_key(19997),
        manager,
        config,
        socket,
        cancel.clone(),
        TaskTracker::new(),
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
    let config = crate::config::bind_gateway_config(None);
    let proxy = Arc::new(EdgionUdpProxy::new(
        19996,
        make_test_listener_key(19996),
        manager,
        config,
        socket,
        cancel,
        TaskTracker::new(),
    ));

    assert_eq!(proxy.session_count(), 0);
}

#[tokio::test]
async fn test_cleanup_expired_sessions() {
    let cancel = CancellationToken::new();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let manager = Arc::new(UdpPortRouteManager::new());
    let config = crate::config::bind_gateway_config(None);
    let proxy = Arc::new(EdgionUdpProxy::new(
        19995,
        make_test_listener_key(19995),
        manager,
        config,
        socket,
        cancel,
        TaskTracker::new(),
    ));

    // Manually insert an expired session
    let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let expired_time = Instant::now() - Duration::from_secs(120);
    let session = Arc::new(ClientSession {
        _session_permit: proxy
            .session_permits
            .clone()
            .try_acquire_owned()
            .expect("test session permit"),
        upstream_socket,
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        lifecycle: Mutex::new(SessionLifecycle {
            last_activity: expired_time,
            retired: false,
        }),
        session_start: expired_time,
        packets_sent: AtomicU64::new(5),
        packets_received: AtomicU64::new(3),
        bytes_sent: AtomicU64::new(500),
        bytes_received: AtomicU64::new(300),
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
    let config = crate::config::bind_gateway_config(None);
    let proxy = Arc::new(EdgionUdpProxy::new(
        19994,
        make_test_listener_key(19994),
        manager,
        config,
        socket,
        cancel,
        TaskTracker::new(),
    ));

    // Insert an active session (recent last_activity)
    let upstream_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let session = Arc::new(ClientSession {
        _session_permit: proxy
            .session_permits
            .clone()
            .try_acquire_owned()
            .expect("test session permit"),
        upstream_socket,
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        lifecycle: Mutex::new(SessionLifecycle {
            last_activity: Instant::now(),
            retired: false,
        }),
        session_start: Instant::now(),
        packets_sent: AtomicU64::new(0),
        packets_received: AtomicU64::new(0),
        bytes_sent: AtomicU64::new(0),
        bytes_received: AtomicU64::new(0),
    });

    let direct_peer_addr: std::net::SocketAddr = "192.168.1.2:12346".parse().unwrap();
    proxy.client_sessions.insert(direct_peer_addr, session);
    assert_eq!(proxy.session_count(), 1);

    proxy.cleanup_expired_sessions().await;
    assert_eq!(proxy.session_count(), 1);
}

#[tokio::test]
async fn expiry_candidate_refreshed_before_retirement_stays_active() {
    let proxy = make_test_proxy(19993).await;
    let peer: SocketAddr = "192.0.2.1:12345".parse().unwrap();
    let session = make_test_session(&proxy, Instant::now() - Duration::from_secs(120));
    proxy.client_sessions.insert(peer, session.clone());
    let candidate_time = Instant::now();

    EdgionUdpProxy::record_upstream_send_result(Ok(3), &session, proxy.listener_port, peer).await;
    assert!(!EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &session,
        Some(candidate_time),
    ));
    assert_eq!(proxy.session_count(), 1);
    assert!(!session.lifecycle.lock().retired);
    assert_eq!(session.packets_sent.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn retired_generation_cannot_remove_replacement_or_keep_receiver_alive() {
    let proxy = make_test_proxy(19992).await;
    let peer: SocketAddr = "192.0.2.2:12346".parse().unwrap();
    let old = make_test_session(&proxy, Instant::now() - Duration::from_secs(120));
    proxy.client_sessions.insert(peer, old.clone());
    assert!(EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &old,
        Some(Instant::now()),
    ));
    let replacement = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(peer, replacement.clone());
    assert!(!EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &old,
        None,
    ));

    tokio::time::timeout(
        Duration::from_secs(1),
        EdgionUdpProxy::handle_upstream_packets(
            peer,
            proxy.socket.clone(),
            proxy.socket.clone(),
            proxy.client_sessions.clone(),
            old.clone(),
            proxy.cancel_token.clone(),
            proxy.listener_port,
        ),
    )
    .await
    .expect("retired receiver should exit despite replacement");
    assert!(Arc::ptr_eq(
        proxy.client_sessions.get(&peer).unwrap().value(),
        &replacement
    ));
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS - 2);
    drop(old);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS - 1);
}

#[tokio::test]
async fn receiver_cancellation_retires_session_and_releases_permit() {
    let proxy = make_test_proxy(19991).await;
    let peer: SocketAddr = "192.0.2.3:12347".parse().unwrap();
    let session = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(peer, session.clone());
    let cancel = CancellationToken::new();
    let (receiver, _send) = controlled_receiver();
    let handle = tokio::spawn(EdgionUdpProxy::handle_upstream_packets(
        peer,
        proxy.socket.clone(),
        receiver.clone(),
        proxy.client_sessions.clone(),
        session.clone(),
        cancel.clone(),
        proxy.listener_port,
    ));
    receiver.started.acquire().await.unwrap().forget();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("pending receiver should stop on cancellation")
        .unwrap();
    assert_eq!(proxy.session_count(), 0);
    assert!(session.lifecycle.lock().retired);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS - 1);
    drop(session);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS);
}

#[tokio::test]
async fn receiver_error_retires_session_and_releases_permit() {
    let proxy = make_test_proxy(19990).await;
    let peer: SocketAddr = "192.0.2.4:12348".parse().unwrap();
    let session = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(peer, session.clone());
    let (receiver, send) = controlled_receiver();
    let handle = tokio::spawn(EdgionUdpProxy::handle_upstream_packets(
        peer,
        proxy.socket.clone(),
        receiver.clone(),
        proxy.client_sessions.clone(),
        session.clone(),
        proxy.cancel_token.clone(),
        proxy.listener_port,
    ));
    receiver.started.acquire().await.unwrap().forget();
    send.send(Err(std::io::Error::other("controlled receive failure")))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("receiver error should stop the task")
        .unwrap();
    assert_eq!(proxy.session_count(), 0);
    assert!(session.lifecycle.lock().retired);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS - 1);
    drop(session);
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS);
}
