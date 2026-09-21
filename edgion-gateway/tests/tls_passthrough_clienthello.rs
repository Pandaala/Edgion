//! Socket-level ClientHello regressions using the production ServerApp callback.
//! This isolated Cargo integration test needs an observable raw TCP backend and
//! per-connection access-log capture; it starts no Gateway/Controller processes
//! and does not restart or modify the shared integration topology. Configuration
//! is published directly, so controller sync and listener construction are out
//! of scope. The backend deliberately performs no TLS handshake: rejection must
//! happen in the gateway, not in a backend TLS library.

use async_trait::async_trait;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_gateway::backends::get_service_discovery_coordinator;
use edgion_gateway::link_sys::DataSender;
use edgion_gateway::observe::AccessLogger;
use edgion_gateway::routes::tls::{sni_parser, EdgionTlsPassthroughProxy, GlobalTlsRouteManagers};
use edgion_resources::resources::tls_route::ResolvedTlsListener;
use edgion_resources::{EdgionGatewayConfig, Gateway, GatewayClass, ListenerKey, TLSRoute};
use k8s_openapi::api::{core::v1::Service, discovery::v1::EndpointSlice};
use pingora_core::{apps::ServerApp, connectors::TransportConnector, protocols::l4};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

const HOST: &str = "hello.example.com";
const BUDGET: Duration = Duration::from_secs(3);

struct LogCapture(mpsc::UnboundedSender<String>);

#[async_trait]
impl DataSender<String> for LogCapture {
    async fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn healthy(&self) -> bool {
        !self.0.is_closed()
    }
    async fn send(&self, data: String) -> anyhow::Result<()> {
        self.0.send(data)?;
        Ok(())
    }
    fn name(&self) -> &str {
        "clienthello-test"
    }
}

struct Fixture {
    listener: TcpListener,
    backend: TcpListener,
    proxy: Arc<EdgionTlsPassthroughProxy>,
    logs: mpsc::UnboundedReceiver<String>,
}

impl Fixture {
    fn publish_security(allow_loopback: bool, log_connections: bool) {
        let config: EdgionGatewayConfig = serde_json::from_value(json!({
            "metadata": {"name": "hello-wire"},
            "spec": {"securityProtect": {
                "allowLoopbackUpstream": allow_loopback,
                "tlsProxyLogRecord": log_connections
            }}
        }))
        .unwrap();
        edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler().partial_update(
            HashMap::new(),
            HashMap::from([("hello-wire".into(), Arc::new(config))]),
            Default::default(),
        );
    }

    async fn security_reload(&mut self, hello: &[u8]) {
        // Use the real endpoint guard: this executable must not enable the
        // shared-topology escape hatch that independently permits loopback.
        assert!(!edgion_common::test_mode::is_integration_testing_mode());
        tokio::time::timeout(BUDGET, self.security_reload_inner(hello))
            .await
            .expect("security hot-reload exchange timed out");
        Self::publish_security(true, true);
        self.exchange(hello, true, None).await;
    }

    async fn security_reload_inner(&mut self, hello: &[u8]) {
        Self::publish_security(true, true);
        let mut old_client = TcpStream::connect(self.listener.local_addr().unwrap()).await.unwrap();
        let (old_downstream, _) = self.listener.accept().await.unwrap();
        let (_shutdown_tx, shutdown) = watch::channel(false);
        let proxy = Arc::clone(&self.proxy);
        let old_process = proxy.process_new(Box::new(l4::stream::Stream::from(old_downstream)), &shutdown);
        let traffic = async {
            old_client.write_all(hello).await.unwrap();
            let (mut old_backend, _) = tokio::time::timeout(Duration::from_secs(1), self.backend.accept())
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "initial allowed generation did not dial backend: {error}; log={:?}",
                        self.logs.try_recv()
                    )
                })
                .unwrap();
            let mut received = vec![0; hello.len()];
            tokio::time::timeout(Duration::from_secs(1), old_backend.read_exact(&mut received))
                .await
                .expect("initial ClientHello was not relayed")
                .unwrap();
            assert_eq!(received, hello);

            for log_enabled in [true, false] {
                Self::publish_security(false, log_enabled);
                let mut new_client = TcpStream::connect(self.listener.local_addr().unwrap()).await.unwrap();
                let (new_downstream, _) = self.listener.accept().await.unwrap();
                let proxy = Arc::clone(&self.proxy);
                let new_process = proxy.process_new(Box::new(l4::stream::Stream::from(new_downstream)), &shutdown);
                let new_traffic = async {
                    new_client.write_all(hello).await.unwrap();
                    let mut reply = [0];
                    match new_client.read(&mut reply).await {
                        Ok(0) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                        result => panic!("expected loopback rejection and socket closure: {result:?}"),
                    }
                };
                let (returned, ()) =
                    tokio::time::timeout(Duration::from_secs(1), async { tokio::join!(new_process, new_traffic) })
                        .await
                        .expect("new denied generation did not close");
                assert!(returned.is_none());
                if log_enabled {
                    let log: Value =
                        serde_json::from_str(&self.logs.try_recv().expect("missing rejection log")).unwrap();
                    assert_eq!(log["err_log"], "unsafe upstream address");
                    assert_eq!(log["sni"], HOST);
                }
                assert!(self.logs.try_recv().is_err(), "unexpected connection log");
                // The production future completed; any forbidden dial would
                // now be in the backlog, not delayed by configuration sync.
                assert!(
                    tokio::time::timeout(Duration::from_millis(25), self.backend.accept())
                        .await
                        .is_err(),
                    "denied generation dialed the loopback backend"
                );
            }

            // The established stream keeps the old policy after revocation.
            old_client.write_all(b"after-update").await.unwrap();
            let mut payload = [0; 12];
            tokio::time::timeout(Duration::from_secs(1), old_backend.read_exact(&mut payload))
                .await
                .expect("old generation stopped forwarding after update")
                .unwrap();
            assert_eq!(&payload, b"after-update");
            old_backend.write_all(&payload).await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), old_client.read_exact(&mut payload))
                .await
                .expect("old generation stopped returning backend bytes")
                .unwrap();
            assert_eq!(&payload, b"after-update");
            old_client.shutdown().await.unwrap();
            let mut extra = Vec::new();
            tokio::time::timeout(Duration::from_secs(1), old_backend.read_to_end(&mut extra))
                .await
                .expect("old client shutdown did not reach backend")
                .unwrap();
            assert!(extra.is_empty());
            old_backend.shutdown().await.unwrap();
        };
        let (returned, ()) = tokio::join!(old_process, traffic);
        assert!(returned.is_none());
        let log: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(1), self.logs.recv())
                .await
                .expect("old generation did not retain enabled logging")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(log["sni"], HOST);
        assert_eq!(log["upstream_info"].as_array().unwrap().len(), 1);
        assert_eq!(log["upstream_info"][0]["bytes_sent"], hello.len() + 12);
        assert!(self.logs.try_recv().is_err(), "duplicate captured-generation log");
    }

    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = backend.local_addr().unwrap().port();
        let key = ListenerKey::new(addr, "TLS");
        let service: Service = serde_json::from_value(json!({
            "metadata": {"namespace": "hello-wire", "name": "backend"},
            "spec": {"ports": [{"port": port, "targetPort": port, "protocol": "TCP"}]}
        }))
        .unwrap();
        let slice: EndpointSlice = serde_json::from_value(json!({
            "metadata": {"namespace": "hello-wire", "name": "backend-slice",
                "labels": {"kubernetes.io/service-name": "backend"}},
            "addressType": "IPv4",
            "ports": [{"port": port, "protocol": "TCP"}],
            "endpoints": [{"addresses": ["127.0.0.1"], "conditions": {"ready": true}}]
        }))
        .unwrap();
        let discovery = get_service_discovery_coordinator();
        discovery.replace_services(&HashMap::from([("hello-wire/backend".into(), Arc::new(service))]));
        discovery.replace_endpoint_slices(&HashMap::from([("hello-wire/backend-slice".into(), Arc::new(slice))]));

        let class: GatewayClass = serde_json::from_value(json!({
            "metadata": {"name": "hello-wire"},
            "spec": {"controllerName": "edgion.io/gateway-controller"}
        }))
        .unwrap();
        edgion_gateway::config::gateway_class::create_gateway_class_handler()
            .full_set(&HashMap::from([("hello-wire".into(), Arc::new(class))]));
        let gateway: Gateway = serde_json::from_value(json!({
            "metadata": {"namespace": "hello-wire", "name": "gateway"},
            "spec": {"gatewayClassName": "hello-wire",
                "addresses": [{"type": "IPAddress", "value": addr.ip().to_string()}],
                "listeners": [{"name": "tls", "port": addr.port(), "protocol": "TLS",
                    "hostname": HOST, "tls": {"mode": "Passthrough"}}]}
        }))
        .unwrap();
        edgion_gateway::runtime::create_gateway_handler()
            .full_set(&HashMap::from([("hello-wire/gateway".into(), Arc::new(gateway))]));

        let mut route: TLSRoute = serde_json::from_value(json!({
            "metadata": {"namespace": "hello-wire", "name": "route"},
            "spec": {"parentRefs": [{"name": "gateway", "sectionName": "tls"}], "hostnames": [HOST], "rules": [{"backendRefs": [{"name": "backend", "port": port}]}]}
        }))
        .unwrap();
        route.spec.resolved_status_controller = Some("edgion.io/gateway-controller".into());
        route.spec.resolved_listeners = Some(vec![ResolvedTlsListener {
            listener_key: key.clone(),
            gateway_namespace: "hello-wire".into(),
            gateway_name: "gateway".into(),
            listener_name: "tls".into(),
            hostnames: vec![HOST.to_string()],
        }]);
        route.spec.resolved_status_controller = Some("edgion.io/gateway-controller".into());
        let routes = GlobalTlsRouteManagers::new();
        routes.full_set(&HashMap::from([("hello-wire/route".into(), Arc::new(route))]));
        let config: EdgionGatewayConfig = serde_json::from_value(json!({
            "metadata": {"name": "hello-wire"},
            "spec": {"securityProtect": {"allowLoopbackUpstream": true}}
        }))
        .unwrap();
        let (sender, logs) = mpsc::unbounded_channel();
        let mut logger = AccessLogger::new();
        logger.register(Box::new(LogCapture(sender)));
        edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler().partial_update(
            HashMap::from([("hello-wire".into(), Arc::new(config))]),
            HashMap::new(),
            Default::default(),
        );
        let proxy = Arc::new(EdgionTlsPassthroughProxy {
            listener_port: addr.port(),
            listener_key: key.clone(),
            tls_route_manager: routes.get_or_create_listener_manager(&key),
            access_logger: Arc::new(logger),
            egc: edgion_gateway::config::bind_gateway_config(Some("hello-wire")),
            connector: TransportConnector::new(None),
        });
        Self {
            listener,
            backend,
            proxy,
            logs,
        }
    }

    async fn exchange(&mut self, bytes: &[u8], accepted: bool, split: Option<usize>) {
        // A timed-out exchange is always a failure, including rejected inputs.
        tokio::time::timeout(BUDGET, self.exchange_inner(bytes, accepted, split))
            .await
            .expect("connection did not finish within the test budget");
    }

    async fn exchange_inner(&mut self, bytes: &[u8], accepted: bool, split: Option<usize>) {
        let mut client = TcpStream::connect(self.listener.local_addr().unwrap()).await.unwrap();
        let (downstream, _) = self.listener.accept().await.unwrap();
        let (_shutdown_tx, shutdown) = watch::channel(false);
        let proxy = Arc::clone(&self.proxy);
        // Keep the production future scoped to this exchange: cancellation or
        // an assertion failure drops its sockets instead of leaking a task.
        let process = proxy.process_new(Box::new(l4::stream::Stream::from(downstream)), &shutdown);
        let traffic = async {
            if let Some(at) = split {
                client.write_all(&bytes[..at]).await.unwrap();
                let mut probe = [0];
                assert!(
                    tokio::time::timeout(Duration::from_millis(25), client.read(&mut probe))
                        .await
                        .is_err(),
                    "an incomplete outer record must remain open awaiting the suffix"
                );
                assert!(
                    tokio::time::timeout(Duration::from_millis(25), self.backend.accept())
                        .await
                        .is_err(),
                    "incomplete ClientHello reached the backend"
                );
                client.write_all(&bytes[at..]).await.unwrap();
            } else {
                client.write_all(bytes).await.unwrap();
            }
            if accepted {
                let (mut upstream, _) = self.backend.accept().await.unwrap();
                let mut received = vec![0; bytes.len()];
                upstream.read_exact(&mut received).await.unwrap();
                assert_eq!(received, bytes, "initial bytes changed or were truncated");
                upstream.write_all(b"backend-observed").await.unwrap();
                let mut reply = [0; 16];
                client.read_exact(&mut reply).await.unwrap();
                assert_eq!(&reply, b"backend-observed");
                client.shutdown().await.unwrap();
                let mut extra = Vec::new();
                upstream.read_to_end(&mut extra).await.unwrap();
                assert!(extra.is_empty(), "initial bytes were replayed more than once");
                upstream.shutdown().await.unwrap();
            } else {
                let mut reply = [0];
                match client.read(&mut reply).await {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    result => panic!("expected parser rejection and socket closure: {result:?}"),
                }
            }
        };
        let (returned, ()) = tokio::join!(process, traffic);
        assert!(returned.is_none());
        let log: Value = serde_json::from_str(&self.logs.recv().await.unwrap()).unwrap();
        if accepted {
            assert_eq!(log["sni"], HOST);
            assert_eq!(log["upstream_info"].as_array().unwrap().len(), 1);
            assert_eq!(log["upstream_info"][0]["bytes_sent"], bytes.len());
        } else {
            assert_eq!(log["err_log"], "no SNI in ClientHello", "must reject in the parser");
            assert!(log["sni"].is_null());
            assert_eq!(log["upstream_info"], json!([]));
        }
        // process_new has finished, so it cannot initiate a later connection.
        // A mistakenly connected backend would still be in the accept backlog.
        assert!(
            tokio::time::timeout(Duration::from_millis(25), self.backend.accept())
                .await
                .is_err(),
            "unexpected backend connection"
        );
        assert!(self.logs.try_recv().is_err(), "duplicate connection log");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This test executable owns its discovery singleton; no other test in
        // this binary publishes resources or runs concurrently with this matrix.
        let discovery = get_service_discovery_coordinator();
        discovery.replace_endpoint_slices(&HashMap::new());
        discovery.replace_services(&HashMap::new());
        edgion_gateway::runtime::create_gateway_handler().full_set(&HashMap::new());
        edgion_gateway::config::gateway_class::create_gateway_class_handler().full_set(&HashMap::new());
        edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler().full_set(&HashMap::new());
    }
}

fn client_hello(version: &'static rustls::SupportedProtocolVersion, host: &str) -> Vec<u8> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let mut client = rustls::ClientConnection::new(Arc::new(config), host.to_owned().try_into().unwrap()).unwrap();
    let mut bytes = Vec::new();
    client.write_tls(&mut bytes).unwrap();
    assert_eq!(
        read_u16(&bytes, 3) + 5,
        bytes.len(),
        "fixture must be one complete record"
    );
    bytes
}

fn read_u16(bytes: &[u8], at: usize) -> usize {
    u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize
}

fn write_length(bytes: &mut [u8], at: usize, width: usize, value: usize) {
    let value = u32::try_from(value).unwrap().to_be_bytes();
    bytes[at..at + width].copy_from_slice(&value[4 - width..]);
}

// Locate fields independently of the gateway parser and rustls extension order.
fn extension_offsets(bytes: &[u8]) -> (usize, usize) {
    let mut pos = 9 + 34;
    pos += 1 + bytes[pos] as usize;
    pos += 2 + read_u16(bytes, pos);
    pos += 1 + bytes[pos] as usize;
    let vector = pos;
    pos += 2;
    while pos < bytes.len() {
        if read_u16(bytes, pos) == 0 {
            return (vector, pos);
        }
        pos += 4 + read_u16(bytes, pos + 2);
    }
    panic!("rustls fixture has no SNI");
}

#[tokio::test]
async fn complete_clienthello_nested_lengths_reject_before_backend_dial() {
    let mut fixture = Fixture::new().await;
    fixture
        .security_reload(&client_hello(&rustls::version::TLS13, HOST))
        .await;
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let original = client_hello(version, HOST);
        fixture.exchange(&original, true, None).await;
        let (vector, sni) = extension_offsets(&original);
        for (field, offset, width, actual) in [
            ("handshake body", 6, 3, original.len() - 9),
            ("extensions vector", vector, 2, read_u16(&original, vector)),
            ("SNI extension", sni + 2, 2, read_u16(&original, sni + 2)),
            ("ServerNameList", sni + 4, 2, read_u16(&original, sni + 4)),
            ("name entry", sni + 7, 2, read_u16(&original, sni + 7)),
        ] {
            for declared in [0, actual - 1, actual + 1, (1 << (8 * width)) - 1] {
                let mut malformed = original.clone();
                write_length(&mut malformed, offset, width, declared);
                assert_eq!(&malformed[..5], &original[..5]);
                assert_eq!(
                    sni_parser::client_hello_record_complete(&malformed),
                    sni_parser::RecordCompleteness::Complete {
                        record_end: malformed.len()
                    }
                );
                eprintln!("{version:?}: {field}, declared={declared}");
                fixture.exchange(&malformed, false, None).await;
            }
        }
        // A valid control after the negatives rules out a dead fixture/backend.
        fixture.exchange(&original, true, Some(3)).await;
        fixture.exchange(&original, true, Some(original.len() - 1)).await;

        let mut extended = original.clone();
        extended.extend_from_slice(&[0xfa, 0xfa, 0, 2, 0xaa, 0xbb]);
        let len = extended.len();
        write_length(&mut extended, 3, 2, len - 5);
        write_length(&mut extended, 6, 3, len - 9);
        write_length(&mut extended, vector, 2, read_u16(&original, vector) + 6);
        fixture.exchange(&extended, true, None).await;

        // Raw byte-preservation control, not a successful TLS early-data claim.
        extended.extend_from_slice(&[0x17, 3, 3, 0, 3, 0xaa, 0xbb, 0xcc]);
        fixture.exchange(&extended, true, None).await;

        // Mutate the wire name explicitly: TLS libraries may normalize input.
        let mut uppercase = original.clone();
        uppercase[sni + 9..sni + 9 + HOST.len()].make_ascii_uppercase();
        fixture.exchange(&uppercase, true, None).await;
    }
}
