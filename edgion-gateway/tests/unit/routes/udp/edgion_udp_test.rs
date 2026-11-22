use super::*;
use crate::routes::udp::UdpPortRouteManager;
use edgion_resources::resources::udp_route::{UDPRoute, UDPRouteSpec};
use kube::api::ObjectMeta;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV6};
use tokio::sync::oneshot;

fn route_unit(name: &str) -> Arc<UdpRouteUnit> {
    Arc::new(
        UdpRouteUnit::from_route(Arc::new(crate::test_current_status::processed(UDPRoute {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: UDPRouteSpec {
                current_status: None,
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: None,
                rules: None,
                resolved_listeners: None,
                resolved_listener_attachments: None,
            },
            status: None,
        })))
        .unwrap(),
    )
}

#[test]
fn selected_unit_survives_reload_without_retaining_old_table() {
    let manager = UdpPortRouteManager::new();
    assert!(select_route_unit(&manager).is_none());

    let old_unit = route_unit("a");
    let sibling = route_unit("b");
    let old_unit_weak = Arc::downgrade(&old_unit);
    let sibling_weak = Arc::downgrade(&sibling);
    manager.rebuild(&HashMap::from([
        ("default/a".to_string(), old_unit),
        ("default/b".to_string(), sibling),
    ]));
    let old_table = Arc::downgrade(&manager.load_route_table());
    let selected = select_route_unit(&manager).unwrap();
    assert!(Arc::ptr_eq(&selected, &old_unit_weak.upgrade().unwrap()));

    let new_unit = route_unit("a");
    manager.rebuild(&HashMap::from([("default/a".to_string(), new_unit.clone())]));
    assert!(old_table.upgrade().is_none(), "selection must release the table guard");
    assert!(
        sibling_weak.upgrade().is_none(),
        "unselected units must not be retained"
    );
    assert!(
        old_unit_weak.upgrade().is_some(),
        "the new session keeps its selected generation"
    );
    assert!(Arc::ptr_eq(&select_route_unit(&manager).unwrap(), &new_unit));
    assert!(!Arc::ptr_eq(&selected, &new_unit));
    drop(selected);
    assert!(old_unit_weak.upgrade().is_none());
}

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

const TEST_SERVICE_KEY: &str = "default/udp-backend";
const TEST_SERVICE_PORT: u16 = 8080;

fn test_service(ports: &[u16]) -> Arc<k8s_openapi::api::core::v1::Service> {
    use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    Arc::new(Service {
        metadata: ObjectMeta {
            namespace: Some("default".to_owned()),
            name: Some("udp-backend".to_owned()),
            uid: Some("udp-backend-uid".to_owned()),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            ports: Some(
                ports
                    .iter()
                    .map(|port| ServicePort {
                        port: i32::from(*port),
                        target_port: Some(IntOrString::Int(i32::from(*port))),
                        protocol: Some("UDP".to_owned()),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn test_slice(addresses: &[&str]) -> Arc<k8s_openapi::api::discovery::v1::EndpointSlice> {
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    Arc::new(EndpointSlice {
        metadata: ObjectMeta {
            namespace: Some("default".to_owned()),
            name: Some("udp-backend-slice".to_owned()),
            uid: Some("udp-backend-slice-uid".to_owned()),
            labels: Some(std::collections::BTreeMap::from([(
                SERVICE_NAME.to_owned(),
                "udp-backend".to_owned(),
            )])),
            ..Default::default()
        },
        address_type: "IPv4".to_owned(),
        endpoints: addresses
            .iter()
            .map(|address| Endpoint {
                addresses: vec![(*address).to_owned()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect(),
        ports: Some(vec![EndpointPort {
            port: Some(i32::from(TEST_SERVICE_PORT)),
            protocol: Some("UDP".to_owned()),
            ..Default::default()
        }]),
    })
}

/// Publish the test Service and EndpointSlice into an isolated coordinator.
fn test_generation(
    service_ports: Option<&[u16]>,
    addresses: Option<&[&str]>,
) -> Arc<crate::backends::discovery::ServiceSelectorGeneration> {
    let coordinator = crate::backends::discovery::ServiceDiscoveryCoordinator::new();
    if let Some(ports) = service_ports {
        coordinator.replace_services(&HashMap::from([(TEST_SERVICE_KEY.to_owned(), test_service(ports))]));
    }
    if let Some(addresses) = addresses {
        coordinator.replace_endpoint_slices(&HashMap::from([(
            "default/udp-backend-slice".to_owned(),
            test_slice(addresses),
        )]));
    }
    coordinator.load_generation()
}

fn test_pinned_endpoint() -> PinnedServiceEndpoint {
    PinnedServiceEndpoint {
        service_key: TEST_SERVICE_KEY.into(),
        section: ServiceSectionKey::Number(std::num::NonZeroU16::new(TEST_SERVICE_PORT).unwrap()),
        per_session_hostname: None,
    }
}

fn make_test_session(proxy: &EdgionUdpProxy, last_activity: Instant) -> Arc<ClientSession> {
    Arc::new(ClientSession {
        cancel_token: proxy.cancel_token.child_token(),
        _route_slot: proxy.route_sessions.acquire().0,
        _session_permit: proxy.session_permits.clone().try_acquire_owned().unwrap(),
        upstream_socket: proxy.socket.clone(),
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        pinned_endpoint: test_pinned_endpoint(),
        lifecycle: Mutex::new(SessionLifecycle {
            last_activity,
            retired: false,
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
        }),
        session_start: last_activity,
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
            cancel_token: proxy.cancel_token.child_token(),
            _route_slot: proxy.route_sessions.acquire().0,
            _session_permit: proxy.session_permits.clone().try_acquire_owned().unwrap(),
            upstream_socket: proxy.socket.clone(),
            upstream_addr: "192.0.2.2:8080".parse().unwrap(),
            pinned_endpoint: test_pinned_endpoint(),
            lifecycle: Mutex::new(SessionLifecycle {
                last_activity: Instant::now(),
                retired: false,
                packets_sent: 5,
                packets_received: 3,
                bytes_sent: 500,
                bytes_received: 300,
            }),
            session_start: Instant::now(),
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
        cancel_token: proxy.cancel_token.child_token(),
        _route_slot: proxy.route_sessions.acquire().0,
        _session_permit: proxy
            .session_permits
            .clone()
            .try_acquire_owned()
            .expect("test session permit"),
        upstream_socket,
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        pinned_endpoint: test_pinned_endpoint(),
        lifecycle: Mutex::new(SessionLifecycle {
            last_activity: expired_time,
            retired: false,
            packets_sent: 5,
            packets_received: 3,
            bytes_sent: 500,
            bytes_received: 300,
        }),
        session_start: expired_time,
    });

    let direct_peer_addr: std::net::SocketAddr = "192.168.1.1:12345".parse().unwrap();
    proxy.client_sessions.insert(direct_peer_addr, session);
    assert_eq!(proxy.session_count(), 1);

    assert_eq!(proxy.route_sessions.active(), 1);

    // Keep the pinned endpoint published so only idleness can retire the session.
    proxy
        .cleanup_sessions(test_generation(Some(&[TEST_SERVICE_PORT]), Some(&["127.0.0.1"])))
        .await;
    assert_eq!(proxy.session_count(), 0);
    // The map held the only session handle, so retirement releases its slot.
    assert_eq!(proxy.route_sessions.active(), 0);
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
        cancel_token: proxy.cancel_token.child_token(),
        _route_slot: proxy.route_sessions.acquire().0,
        _session_permit: proxy
            .session_permits
            .clone()
            .try_acquire_owned()
            .expect("test session permit"),
        upstream_socket,
        upstream_addr: "127.0.0.1:8080".parse().unwrap(),
        pinned_endpoint: test_pinned_endpoint(),
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

    let direct_peer_addr: std::net::SocketAddr = "192.168.1.2:12346".parse().unwrap();
    proxy.client_sessions.insert(direct_peer_addr, session);
    assert_eq!(proxy.session_count(), 1);

    proxy
        .cleanup_sessions(test_generation(Some(&[TEST_SERVICE_PORT]), Some(&["127.0.0.1"])))
        .await;
    assert_eq!(proxy.session_count(), 1);
    assert_eq!(proxy.route_sessions.active(), 1);
}

#[tokio::test]
async fn existing_session_lookup_releases_the_new_route_slot() {
    let proxy = make_test_proxy(19992).await;
    let peer: SocketAddr = "192.0.2.7:12345".parse().unwrap();
    proxy
        .client_sessions
        .insert(peer, make_test_session(&proxy, Instant::now()));
    assert_eq!(proxy.route_sessions.active(), 1);

    let (slot, ordinal) = proxy.route_sessions.acquire();
    assert_eq!(ordinal, 2);
    let mut children = JoinSet::new();
    let session = proxy
        .get_or_create_session(
            peer,
            "127.0.0.1:8080".parse().unwrap(),
            test_pinned_endpoint(),
            slot,
            &mut children,
        )
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&session, &proxy.client_sessions.get(&peer).unwrap()));
    // The existing session keeps its own slot; the caller's slot is released.
    assert_eq!(proxy.route_sessions.active(), 1);
}

#[tokio::test]
async fn expiry_candidate_refreshed_before_retirement_stays_active() {
    let proxy = make_test_proxy(19993).await;
    let peer: SocketAddr = "192.0.2.1:12345".parse().unwrap();
    let session = make_test_session(&proxy, Instant::now() - Duration::from_secs(120));
    proxy.client_sessions.insert(peer, session.clone());
    let candidate_time = Instant::now();

    let before_error = session.lifecycle.lock().last_activity;
    EdgionUdpProxy::record_upstream_send_result(
        Err(std::io::Error::other("controlled send failure")),
        session.clone(),
        proxy.listener_port,
        peer,
    )
    .await;
    assert_eq!(session.lifecycle.lock().last_activity, before_error);
    assert_eq!(session.lifecycle.lock().packets_sent, 0);
    assert!(!session.lifecycle.lock().retired);
    assert!(Arc::ptr_eq(proxy.client_sessions.get(&peer).unwrap().value(), &session));

    EdgionUdpProxy::record_upstream_send_result(Ok(3), session.clone(), proxy.listener_port, peer).await;
    assert!(!EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &session,
        Some(candidate_time),
    ));
    assert_eq!(proxy.session_count(), 1);
    assert!(!session.lifecycle.lock().retired);
    assert_eq!(session.lifecycle.lock().packets_sent, 1);
}

#[tokio::test(start_paused = true)]
async fn retired_generation_cannot_remove_replacement_or_keep_receiver_alive() {
    let proxy = make_test_proxy(19992).await;
    let peer: SocketAddr = "192.0.2.2:12346".parse().unwrap();
    let old = make_test_session(&proxy, Instant::now() - Duration::from_secs(120));
    proxy.client_sessions.insert(peer, old.clone());
    let (receiver, _send) = controlled_receiver();
    let handle = tokio::spawn(EdgionUdpProxy::handle_upstream_packets(
        peer,
        proxy.socket.clone(),
        receiver.clone(),
        proxy.client_sessions.clone(),
        old.clone(),
        proxy.listener_port,
    ));
    receiver.started.acquire().await.unwrap().forget();
    let before_retirement = tokio::time::Instant::now();
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

    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("retirement should wake the pending receiver")
        .unwrap();
    assert_eq!(tokio::time::Instant::now(), before_retirement);
    assert!(!replacement.cancel_token.is_cancelled());
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
    let (receiver, _send) = controlled_receiver();
    let handle = tokio::spawn(EdgionUdpProxy::handle_upstream_packets(
        peer,
        proxy.socket.clone(),
        receiver.clone(),
        proxy.client_sessions.clone(),
        session.clone(),
        proxy.listener_port,
    ));
    receiver.started.acquire().await.unwrap().forget();
    proxy.cancel_token.cancel();
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

#[tokio::test]
async fn policy_ancestor_refusal_preserves_selected_weight_and_skips_resolution() {
    use crate::backends::policy::{
        get_global_backend_tls_policy_store, resolve_l4_policy_backend, GLOBAL_TLS_STORE_TEST_LOCK,
    };
    let _guard = GLOBAL_TLS_STORE_TEST_LOCK.lock().await;
    let policy: edgion_resources::BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"namespace":"capacity-udp","name":"limited"},
        "spec":{"targetRefs":[{"group":"","kind":"Service","name":"limited"}],
        "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"},
        "preparedPrecedence":{"creationTimestamp":null},"resolvedTargetClass":"service",
        "preparedAncestorAdmission":{"mode":"scoped","gateways":["capacity-udp/represented"]}}
    }))
    .unwrap();
    let mut policy = policy;
    policy.spec.use_system_ca = true;
    assert!(edgion_resources::resources::common::policy_candidates::prepared_backend_tls_target(&policy).is_some());
    policy.spec.prepared_ancestor_admission = Some(
        edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Scoped(vec![
            "capacity-udp/represented".into(),
        ]),
    );
    let store = get_global_backend_tls_policy_store();
    store.replace_all(std::collections::HashMap::from([(
        "capacity-udp/limited".into(),
        Arc::new(policy),
    )]));
    let route = serde_json::from_value(serde_json::json!({
        "metadata":{"namespace":"capacity-udp","name":"mixed"},
        "spec":{"rules":[{"backendRefs":[{"name":"limited","port":80,"weight":1},{"name":"plain","port":80,"weight":1}]}]}
    })).unwrap();
    let unit = crate::routes::udp::UdpRouteUnit::from_route(Arc::new(route)).unwrap();
    let calls = std::cell::Cell::new(0);
    let result = resolve_l4_policy_backend(unit.select_backend().unwrap(), Some("capacity-udp/new"), |_| {
        calls.set(calls.get() + 1);
        Ok::<_, ()>(())
    });
    assert!(matches!(
        result,
        Err(crate::backends::L4ResolveError::Rejected(
            crate::backends::L4BackendRejection::Denied
        ))
    ));
    assert_eq!(calls.get(), 0);
    assert_eq!(unit.select_backend().unwrap().service().unwrap().name(), "plain");
    let result = resolve_l4_policy_backend(unit.select_backend().unwrap(), Some("capacity-udp/represented"), |_| {
        calls.set(calls.get() + 1);
        Ok::<_, ()>(())
    });
    assert!(result.is_ok());
    assert_eq!(calls.get(), 1);
    store.replace_all(std::collections::HashMap::new());
}

#[tokio::test]
async fn session_accounting_wraps_and_freezes_after_retirement() {
    let proxy = make_test_proxy(19991).await;
    let peer: SocketAddr = "192.0.2.10:12345".parse().unwrap();
    let session = make_test_session(&proxy, Instant::now());
    {
        let mut lifecycle = session.lifecycle.lock();
        lifecycle.packets_sent = u64::MAX;
        lifecycle.packets_received = u64::MAX;
        lifecycle.bytes_sent = u64::MAX;
        lifecycle.bytes_received = u64::MAX;
    }
    proxy.client_sessions.insert(peer, session.clone());

    EdgionUdpProxy::record_upstream_send_result(Ok(3), session.clone(), proxy.listener_port, peer).await;
    EdgionUdpProxy::record_downstream_send_result(Ok(5), &session, proxy.listener_port, peer).await;
    assert!(EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &session,
        None
    ));
    let activity = session.lifecycle.lock().last_activity;
    EdgionUdpProxy::record_upstream_send_result(Ok(7), session.clone(), proxy.listener_port, peer).await;
    EdgionUdpProxy::record_downstream_send_result(Ok(9), &session, proxy.listener_port, peer).await;
    assert_eq!(session.lifecycle.lock().last_activity, activity);
    let log = EdgionUdpProxy::session_log_entry(proxy.listener_port, peer, &session);
    let retired = Arc::downgrade(&session);
    drop(session);
    assert!(retired.upgrade().is_none());
    assert_eq!(proxy.session_permits.available_permits(), MAX_UDP_SESSIONS);
    assert_eq!((log.packets_sent, log.packets_received), (0, 0));
    assert_eq!((log.bytes_sent, log.bytes_received), (2, 4));
}

#[test]
fn pinned_endpoint_removal_requires_published_proof() {
    let pinned = test_pinned_endpoint();
    let upstream: SocketAddr = "10.0.0.1:8080".parse().unwrap();
    let port = [TEST_SERVICE_PORT];

    let live = test_generation(Some(&port), Some(&["10.0.0.1", "10.0.0.2"]));
    assert!(!pinned_endpoint_removed(&live, &pinned, upstream));

    let rotated = test_generation(Some(&port), Some(&["10.0.0.2"]));
    assert!(pinned_endpoint_removed(&rotated, &pinned, upstream));

    let port_removed = test_generation(Some(&[9090]), Some(&["10.0.0.1"]));
    assert!(pinned_endpoint_removed(&port_removed, &pinned, upstream));

    let service_deleted = test_generation(None, Some(&["10.0.0.1"]));
    assert!(pinned_endpoint_removed(&service_deleted, &pinned, upstream));

    let absent = test_generation(None, None);
    assert!(pinned_endpoint_removed(&absent, &pinned, upstream));
}

#[test]
fn invalid_section_is_indeterminate_and_keeps_session() {
    use k8s_openapi::api::discovery::v1::EndpointPort;
    // Two unnamed slice ports make the section ambiguous: it is invalid
    // without proving the pinned endpoint was removed.
    let mut slice = (*test_slice(&["10.0.0.1"])).clone();
    slice.ports.as_mut().unwrap().push(EndpointPort {
        port: Some(9090),
        protocol: Some("UDP".to_owned()),
        ..Default::default()
    });
    let coordinator = crate::backends::discovery::ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        TEST_SERVICE_KEY.to_owned(),
        test_service(&[TEST_SERVICE_PORT]),
    )]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/udp-backend-slice".to_owned(),
        Arc::new(slice),
    )]));
    let generation = coordinator.load_generation();
    let pinned = test_pinned_endpoint();

    let ServicePublishedState::Available(service) = generation.service(TEST_SERVICE_KEY).unwrap() else {
        panic!("service should be published");
    };
    assert!(matches!(
        service.resolve_section(&pinned.section),
        Err(ServiceSectionInvalid::AmbiguousEndpointPort)
    ));
    assert!(!pinned_endpoint_removed(
        &generation,
        &pinned,
        "10.0.0.1:8080".parse().unwrap()
    ));
}

#[tokio::test]
async fn cleanup_retires_active_session_whose_endpoint_was_removed() {
    let proxy = make_test_proxy(19987).await;
    let removed_peer: SocketAddr = "192.0.2.10:12345".parse().unwrap();
    let removed = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(removed_peer, removed.clone());

    // The pinned 127.0.0.1:8080 endpoint is gone; another endpoint remains.
    proxy
        .cleanup_sessions(test_generation(Some(&[TEST_SERVICE_PORT]), Some(&["127.0.0.2"])))
        .await;

    assert_eq!(proxy.session_count(), 0);
    assert!(removed.lifecycle.lock().retired);
}

#[tokio::test]
async fn endpoint_removal_candidate_does_not_retire_replacement_generation() {
    let proxy = make_test_proxy(19986).await;
    let peer: SocketAddr = "192.0.2.11:12345".parse().unwrap();
    let old = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(peer, old.clone());
    let generation = test_generation(Some(&[TEST_SERVICE_PORT]), Some(&["127.0.0.2"]));
    let candidates = EdgionUdpProxy::collect_retirement_candidates(&proxy.client_sessions, &generation, Instant::now());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].2, RetireReason::EndpointRemoved);

    let replacement = make_test_session(&proxy, Instant::now());
    proxy.client_sessions.insert(peer, replacement.clone());
    assert!(!EdgionUdpProxy::retire_session(
        &proxy.client_sessions,
        peer,
        &candidates[0].1,
        None
    ));
    assert!(Arc::ptr_eq(
        proxy.client_sessions.get(&peer).unwrap().value(),
        &replacement
    ));
    assert!(!replacement.lifecycle.lock().retired);
}

#[test]
fn per_session_resolved_address_retires_when_the_system_mode_target_changes() {
    use edgion_resources::constants::annotations::edgion::DNS_RESOLUTION_MODE;
    fn external(hostname: &str, mode: &str, port: u16) -> Arc<k8s_openapi::api::core::v1::Service> {
        let mut service = (*test_service(&[port])).clone();
        let spec = service.spec.as_mut().unwrap();
        spec.type_ = Some("ExternalName".to_owned());
        spec.external_name = Some(hostname.to_owned());
        service.metadata.annotations = Some(std::collections::BTreeMap::from([(
            DNS_RESOLUTION_MODE.to_owned(),
            mode.to_owned(),
        )]));
        Arc::new(service)
    }
    fn generation(
        service: Arc<k8s_openapi::api::core::v1::Service>,
    ) -> Arc<crate::backends::discovery::ServiceSelectorGeneration> {
        let coordinator = crate::backends::discovery::ServiceDiscoveryCoordinator::new();
        coordinator.replace_services(&HashMap::from([(TEST_SERVICE_KEY.to_owned(), service)]));
        coordinator.load_generation()
    }
    let mut pinned = test_pinned_endpoint();
    pinned.per_session_hostname = Some("udp.example.com".into());
    let upstream: SocketAddr = "192.0.2.10:8080".parse().unwrap();

    let same = generation(external("udp.example.com", "system", TEST_SERVICE_PORT));
    assert!(
        !pinned_endpoint_removed(&same, &pinned, upstream),
        "no published slot is required for a per-session address"
    );
    for changed in [
        external("other.example.com", "system", TEST_SERVICE_PORT),
        external("udp.example.com", "gateway", TEST_SERVICE_PORT),
        external("udp.example.com", "system", 9090),
        test_service(&[TEST_SERVICE_PORT]),
    ] {
        assert!(pinned_endpoint_removed(&generation(changed), &pinned, upstream));
    }
    assert!(pinned_endpoint_removed(&test_generation(None, None), &pinned, upstream));
}
