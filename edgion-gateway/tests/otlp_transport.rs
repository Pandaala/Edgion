//! Real OTLP transport and lifecycle checks against owned TCP/TLS/DNS peers.
//! Run: cargo test -p edgion-gateway --features test-util --test otlp_transport

use parking_lot::Mutex;
use std::future::{poll_fn, Future};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use edgion_common::dns::ResolverSpec;
use edgion_gateway::link_sys::providers::otlp::OtlpLinkClient;
use edgion_gateway::link_sys::LogMeta;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpClientConfig};
use edgion_resources::resources::link_sys::LinkSys;
use k8s_openapi::{api::core::v1::Secret, ByteString};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

trait AsyncIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AsyncIo for T {}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Mode {
    Success,
    StallHeaders,
    StallBody,
    HostileHeaders,
    HostileTrailers,
    HostileBody,
}

const REMOTE_MESSAGE: &str = "private-collector-status-sentinel";
const REMOTE_BODY: &[u8] = b"private-collector-body-sentinel";

#[derive(Default)]
struct Observations {
    accepts: AtomicUsize,
    requests: AtomicUsize,
    records: AtomicUsize,
    resets: AtomicUsize,
    closes: AtomicUsize,
    tls_failures: AtomicUsize,
    hostile_responses: AtomicUsize,
    snis: Mutex<Vec<String>>,
    metadata: Mutex<Vec<Option<String>>>,
    bodies: Mutex<Vec<Vec<u8>>>,
}
struct Collector {
    address: SocketAddr,
    observations: Arc<Observations>,
    mode: Arc<AtomicUsize>,
    authorization: Arc<Mutex<Option<String>>>,
    disconnect: CancellationToken,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Collector {
    async fn start(bind: Ipv4Addr, mode: Mode, tls: Option<Arc<rustls::ServerConfig>>) -> Self {
        let listener = TcpListener::bind((bind, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let authorization = Arc::new(Mutex::new(None::<String>));
        let expected = authorization.clone();
        let mode = Arc::new(AtomicUsize::new(mode as usize));
        let behavior = mode.clone();
        let observations = Arc::new(Observations::default());
        let observed = observations.clone();
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let disconnect = CancellationToken::new();
        let disconnecting = disconnect.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopping.cancelled() => break,
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        observed.accepts.fetch_add(1,Ordering::SeqCst);
                        let expected = expected.clone();
                        let mode = behavior.clone();
                        let tls = tls.clone();
                        let observed = observed.clone();
                        let stopping = stopping.clone();
                        let disconnecting = disconnecting.clone();
                        connections.spawn(async move {
                            let socket: Box<dyn AsyncIo> = if let Some(tls) = tls {
                                match tokio_rustls::TlsAcceptor::from(tls).accept(socket).await {
                                    Ok(socket) => {
                                        if let Some(name) = socket.get_ref().1.server_name() {observed.snis.lock().push(name.to_string());}
                                        Box::new(socket)
                                    }
                                    Err(_) => {observed.tls_failures.fetch_add(1, Ordering::SeqCst); return;}
                                }
                            } else {Box::new(socket)};
                            let Ok(mut connection) = h2::server::handshake(socket).await else {return;};
                            let mut handlers = tokio::task::JoinSet::new();
                            let mut had_request = false;
                            loop {
                                tokio::select! {
                                    _ = stopping.cancelled() => break,
                                    _ = disconnecting.cancelled() => break,
                                    incoming = connection.accept() => {
                                        let Some(Ok((request, mut response))) = incoming else {break;};
                                        had_request = true;
                                        observed.requests.fetch_add(1, Ordering::SeqCst);
                                        let authorized = expected.lock().as_ref().is_none_or(|token| request.headers().get("authorization").is_some_and(|value| value.as_bytes()==token.as_bytes()));
                                        if !authorized {
                                            let _ = response.send_response(http::Response::builder().header("content-type","application/grpc").header("grpc-status","16").body(()).unwrap(),true);
                                            continue;
                                        }
                                        observed.metadata.lock().push(request.headers().get("authorization").and_then(|value| value.to_str().ok()).map(str::to_owned));
                                        let observed = observed.clone();
                                        let mode = match mode.load(Ordering::SeqCst) {1=>Mode::StallHeaders,2=>Mode::StallBody,3=>Mode::HostileHeaders,4=>Mode::HostileTrailers,5=>Mode::HostileBody,_=>Mode::Success};
                                        handlers.spawn(async move {
                                            let mut body = request.into_body();
                                            let mut bytes = Vec::new();
                                            while let Some(Ok(chunk)) = body.data().await {
                                                assert!(bytes.len() + chunk.len() <= 1024 * 1024);
                                                body.flow_control().release_capacity(chunk.len()).unwrap();
                                                bytes.extend_from_slice(&chunk);
                                            }
                                            observed.records.fetch_add(bytes.windows(b"otlp-test-record".len()).filter(|window| *window == b"otlp-test-record").count(), Ordering::SeqCst);
                                            observed.bodies.lock().push(bytes);
                                            if matches!(mode, Mode::HostileHeaders) {
                                                response.send_response(http::Response::builder().header("content-type", "application/grpc").header("grpc-status", "13").header("grpc-message", REMOTE_MESSAGE).body(()).unwrap(), true).unwrap();
                                                observed.hostile_responses.fetch_add(1, Ordering::SeqCst);
                                                return;
                                            }
                                            if matches!(mode, Mode::StallHeaders) {
                                                let _ = poll_fn(|context| response.poll_reset(context)).await;
                                                observed.resets.fetch_add(1, Ordering::SeqCst);
                                                return;
                                            }
                                            let Ok(mut stream) = response.send_response(http::Response::builder().header("content-type", "application/grpc").body(()).unwrap(), false) else {return;};
                                            if matches!(mode, Mode::StallBody) {
                                                let _ = poll_fn(|context| stream.poll_reset(context)).await;
                                                observed.resets.fetch_add(1, Ordering::SeqCst);
                                                return;
                                            }
                                            if matches!(mode, Mode::HostileBody) {
                                                let mut frame = vec![0];
                                                frame.extend_from_slice(&(REMOTE_BODY.len() as u32).to_be_bytes());
                                                frame.extend_from_slice(REMOTE_BODY);
                                                stream.send_data(Bytes::from(frame), false).unwrap();
                                            } else {
                                                // An empty ExportLogsServiceResponse is a five-byte gRPC frame.
                                                let _ = stream.send_data(Bytes::from_static(&[0, 0, 0, 0, 0]), false);
                                            }
                                            let mut trailers = http::HeaderMap::new();
                                            if matches!(mode, Mode::HostileTrailers | Mode::HostileBody) {
                                                trailers.insert("grpc-status", http::HeaderValue::from_static("13"));
                                                trailers.insert("grpc-message", http::HeaderValue::from_static(REMOTE_MESSAGE));
                                                stream.send_trailers(trailers).unwrap();
                                                observed.hostile_responses.fetch_add(1, Ordering::SeqCst);
                                            } else {
                                                trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
                                                let _ = stream.send_trailers(trailers);
                                            }
                                        });
                                    }
                                }
                            }
                            handlers.abort_all();
                            while handlers.join_next().await.is_some() {}
                            if had_request {observed.closes.fetch_add(1, Ordering::SeqCst);}
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            address,
            observations,
            mode,
            authorization,
            disconnect,
            stop,
            task,
        }
    }
    fn require_authorization(&self, value: &str) {
        *self.authorization.lock() = Some(value.to_owned());
    }
    fn set_mode(&self, mode: Mode) {
        self.mode.store(mode as usize, Ordering::SeqCst);
    }
    async fn shutdown(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned readiness deadline expired");
}
fn config(endpoint: String, timeout_ms: u64) -> OtlpClientConfig {
    OtlpClientConfig {
        endpoint,
        timeout_ms,
        auth: None,
        tls: None,
    }
}
fn otlp_resource(key: &str, config: OtlpClientConfig) -> LinkSys {
    let (namespace, name) = key.split_once('/').expect("test key");
    let mut resource: LinkSys = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "LinkSys",
        "metadata": {"namespace": namespace, "name": name},
        "spec": {"type": "otlp", "config": {"endpoint": "http://placeholder.test:4317"}}
    }))
    .expect("OTLP resource");
    let edgion_resources::resources::link_sys::SystemConfig::Otlp(current) = &mut resource.spec.config else {
        panic!("OTLP resource");
    };
    *current = config;
    resource
}
fn build_client(key: &str, config: OtlpClientConfig) -> Arc<OtlpLinkClient> {
    OtlpLinkClient::from_link_sys(Arc::new(otlp_resource(key, config))).expect("OTLP generation")
}
fn build_client_with_resolver(key: &str, config: OtlpClientConfig, resolver: ResolverSpec) -> Arc<OtlpLinkClient> {
    OtlpLinkClient::from_link_sys_with_resolver(Arc::new(otlp_resource(key, config)), resolver)
        .expect("OTLP generation")
}
fn emit(client: &OtlpLinkClient, count: usize) {
    for _ in 0..count {
        client
            .send_with_meta("otlp-test-record".into(), &LogMeta::default())
            .unwrap();
    }
}
fn tls_material() -> (String, String, String) {
    tls_material_for_name("localhost")
}
fn tls_material_for_name(name: &str) -> (String, String, String) {
    use rcgen::*;
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let key = KeyPair::generate().unwrap();
    let pem = ca.self_signed(&key).unwrap().pem();
    let issuer = Issuer::new(ca, key);
    let mut leaf = CertificateParams::new(vec![name.into()]).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, name);
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().unwrap();
    (pem, leaf.signed_by(&key, &issuer).unwrap().pem(), key.serialize_pem())
}
fn server_tls(ca: &str, cert: &str, key: &str, mtls: bool) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = if mtls {
        let mut roots = rustls::RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(ca.as_bytes()) {
            roots.add(certificate.unwrap()).unwrap();
        }
        builder.with_client_cert_verifier(
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap(),
        )
    } else {
        builder.with_no_client_auth()
    };
    let mut config = builder
        .with_single_cert(
            CertificateDer::pem_slice_iter(cert.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap(),
            PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap(),
        )
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}
fn client_tls(ca: &str, identity: Option<(&str, &str)>) -> TlsConfig {
    let mut tls = TlsConfig {
        enabled: true,
        ..Default::default()
    };
    tls.outbound.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".into(),
            name: "ca".into(),
            namespace: None,
        }]),
        ..Default::default()
    });
    tls.resolved_ca_certificates = Some(vec![Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("transport".into()),
            ..Default::default()
        },
        data: Some(
            [(
                String::from(edgion_resources::constants::secret_keys::tls::CA_CERT),
                ByteString(ca.as_bytes().to_vec()),
            )]
            .into(),
        ),
        ..Default::default()
    }]);
    if let Some((cert, key)) = identity {
        tls.outbound.client_certificate_ref = Some(edgion_resources::resources::common::SecretObjectReference {
            group: None,
            kind: None,
            name: "client".into(),
            namespace: None,
        });
        tls.resolved_client_certificate = Some(Secret {
            metadata: kube::core::ObjectMeta {
                name: Some("client".into()),
                namespace: Some("transport".into()),
                ..Default::default()
            },
            data: Some(
                [
                    (String::from("tls.crt"), ByteString(cert.as_bytes().to_vec())),
                    (String::from("tls.key"), ByteString(key.as_bytes().to_vec())),
                ]
                .into(),
            ),
            ..Default::default()
        });
    }
    tls
}

#[tokio::test(flavor = "current_thread")]
async fn live_tls_and_auth_delivery() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    let (ca, cert, key) = tls_material();
    for secure in [false, true] {
        for mtls in [false, true] {
            if !secure && mtls {
                continue;
            }
            let tls = secure.then(|| server_tls(&ca, &cert, &key, mtls));
            let collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, tls).await;
            let scheme = if secure { "https" } else { "http" };
            let mut cfg = config(format!("{scheme}://localhost:{}", collector.address.port()), 1000);
            if secure {
                cfg.tls = Some(client_tls(&ca, mtls.then_some((cert.as_str(), key.as_str()))));
            }
            cfg.auth = Some(OtlpAuth {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        name: "auth".into(),
                        namespace: None,
                        group: None,
                        kind: None,
                    },
                    secret: Some(Secret {
                        metadata: kube::api::ObjectMeta {
                            name: Some("auth".into()),
                            namespace: Some("transport".into()),
                            ..Default::default()
                        },
                        data: Some([(String::from("bearerToken"), ByteString(b"approved-token".to_vec()))].into()),
                        ..Default::default()
                    }),
                },
            });
            let client = build_client("transport/live", cfg);
            client.init().await.unwrap();
            emit(&client, 1);
            client.shutdown().await.unwrap();
            assert_eq!(collector.observations.records.load(Ordering::SeqCst), 1);
            assert!(collector
                .observations
                .metadata
                .lock()
                .iter()
                .all(|value| value.as_deref() == Some("Bearer approved-token")));
            wait_until(|| collector.observations.closes.load(Ordering::SeqCst) >= 1).await;
            if secure {
                assert!(collector.observations.snis.lock().iter().any(|sni| sni == "localhost"));
            }
            collector.shutdown().await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn tls_rejects_untrusted_wrong_hostname_and_missing_identity() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    let (ca, cert, key) = tls_material();
    for (override_ca, hostname, identity) in [
        (false, "localhost", false),
        (true, "127.0.0.1", false),
        (true, "localhost", false),
    ] {
        let require_identity = override_ca && hostname == "localhost";
        let collector = Collector::start(
            Ipv4Addr::LOCALHOST,
            Mode::Success,
            Some(server_tls(&ca, &cert, &key, require_identity)),
        )
        .await;
        let mut cfg = config(format!("https://{hostname}:{}", collector.address.port()), 500);
        if override_ca {
            cfg.tls = Some(client_tls(&ca, identity.then_some((cert.as_str(), key.as_str()))));
        }
        let client = build_client("transport/rejected", cfg);
        assert!(client.init().await.is_err());
        wait_until(|| collector.observations.tls_failures.load(Ordering::SeqCst) > 0).await;
        assert!(!client.healthy());
        client.shutdown().await.unwrap();
        assert_eq!(collector.observations.requests.load(Ordering::SeqCst), 0);
        collector.shutdown().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn entire_export_timeout_after_headers() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    for mode in [Mode::StallHeaders, Mode::StallBody] {
        let collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
        let client = build_client(
            "transport/stalled",
            config(format!("http://{}", collector.address), 200),
        );
        client.init().await.unwrap();
        collector.set_mode(mode);
        emit(&client, 512);
        wait_until(|| collector.observations.resets.load(Ordering::SeqCst) > 0).await;
        client.shutdown().await.unwrap();
        wait_until(|| collector.observations.closes.load(Ordering::SeqCst) > 0).await;
        collector.shutdown().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retirement_cancellation_survives_aborted_waiter() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    for mode in [Mode::StallHeaders, Mode::StallBody] {
        let collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
        let client = build_client(
            "transport/cancel",
            config(format!("http://{}", collector.address), 300000),
        );
        client.init().await.unwrap();
        collector.set_mode(mode);
        let before = collector.observations.requests.load(Ordering::SeqCst);
        emit(&client, 512);
        wait_until(|| collector.observations.requests.load(Ordering::SeqCst) > before).await;
        emit(&client, 2048);
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let caller = client.clone();
        let waiter = tokio::spawn(async move {
            let future = caller.shutdown();
            tokio::pin!(future);
            poll_fn(|context| {
                assert!(future.as_mut().poll(context).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            started.send(()).unwrap();
            future.await
        });
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(10), client.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(!client.healthy());
        wait_until(|| collector.observations.closes.load(Ordering::SeqCst) > 0).await;
        collector.shutdown().await;
    }
}

struct DnsPeer {
    address: SocketAddr,
    answers: Arc<Mutex<Vec<IpAddr>>>,
    queries: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}
impl DnsPeer {
    async fn start(answers: Vec<IpAddr>) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let answers = Arc::new(Mutex::new(answers));
        let current = answers.clone();
        let queries = Arc::new(AtomicUsize::new(0));
        let observed = queries.clone();
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            let mut packet = [0u8; 4096];
            loop {
                tokio::select! {
                    _=stopping.cancelled()=>break,
                    received=socket.recv_from(&mut packet)=>{
                        let (length,peer)=received.unwrap();
                        assert!(length>=17);
                        let mut cursor=12;
                        while packet[cursor]!=0 {cursor+=usize::from(packet[cursor])+1; assert!(cursor<length);}
                        cursor+=1;
                        let query_type=u16::from_be_bytes([packet[cursor],packet[cursor+1]]);
                        let question_end=cursor+4;
                        let records=current.lock().iter().filter_map(|ip|match (query_type,ip) {
                            (1,IpAddr::V4(ip))=>Some(ip.octets().to_vec()),
                            (28,IpAddr::V6(ip))=>Some(ip.octets().to_vec()),
                            _=>None,
                        }).collect::<Vec<_>>();
                        let mut response=vec![packet[0],packet[1],0x81,0x80,0,1,0,records.len() as u8,0,0,0,0];
                        response.extend_from_slice(&packet[12..question_end]);
                        for record in records {
                            response.extend_from_slice(&[0xc0,0x0c]);
                            response.extend_from_slice(&query_type.to_be_bytes());
                            response.extend_from_slice(&[0,1,0,0,0,0]);
                            response.extend_from_slice(&(record.len() as u16).to_be_bytes());
                            response.extend_from_slice(&record);
                        }
                        socket.send_to(&response,peer).await.unwrap();
                        observed.fetch_add(1,Ordering::SeqCst);
                    }
                }
            }
        });
        Self {
            address,
            answers,
            queries,
            stop,
            task,
        }
    }
    fn resolver(&self) -> ResolverSpec {
        ResolverSpec::StandardDns {
            servers: vec![self.address.to_string()],
        }
    }
    async fn shutdown(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

async fn production_dns_case() {
    use edgion_gateway::plugins::http::common::outbound::egress::configure_egress_defaults;
    assert!(!edgion_common::test_mode::is_integration_testing_mode());
    let route = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    // UDP connect selects an interface; it sends no packet to this documentation address.
    route.connect("192.0.2.1:9").await.unwrap();
    let IpAddr::V4(local) = route.local_addr().unwrap().ip() else {
        panic!("IPv4 interface required");
    };
    assert!(!local.is_loopback() && !local.is_unspecified());
    let collector = Collector::start(Ipv4Addr::UNSPECIFIED, Mode::Success, None).await;
    let dns = DnsPeer::start(vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V4(local)]).await;
    configure_egress_defaults(false, Duration::from_secs(5));
    let cfg = config(
        format!("http://collector.example.test:{}", collector.address.port()),
        500,
    );
    let client = build_client_with_resolver("transport/dns", cfg.clone(), dns.resolver());
    client.init().await.unwrap();
    emit(&client, 512);
    wait_until(|| collector.observations.records.load(Ordering::SeqCst) == 512).await;
    assert!(collector.observations.metadata.lock().iter().all(Option::is_none));
    let queries = dns.queries.load(Ordering::SeqCst);
    let accepts = collector.observations.accepts.load(Ordering::SeqCst);
    *dns.answers.lock() = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    collector.disconnect.cancel();
    wait_until(|| collector.observations.closes.load(Ordering::SeqCst) > 0).await;
    emit(&client, 512);
    wait_until(|| dns.queries.load(Ordering::SeqCst) > queries).await;
    wait_until(|| !client.healthy()).await;
    assert_eq!(collector.observations.records.load(Ordering::SeqCst), 512);
    client.shutdown().await.unwrap();
    assert_eq!(
        collector.observations.accepts.load(Ordering::SeqCst),
        accepts,
        "a blocked reconnect must never reach TCP accept"
    );

    for answer in ["::ffff:127.0.0.1", "169.254.169.254", "0.0.0.0", "::"] {
        *dns.answers.lock() = vec![answer.parse().unwrap()];
        let queries = dns.queries.load(Ordering::SeqCst);
        let accepts = collector.observations.accepts.load(Ordering::SeqCst);
        let client = build_client_with_resolver("transport/blocked", cfg.clone(), dns.resolver());
        assert!(client.init().await.is_err());
        wait_until(|| dns.queries.load(Ordering::SeqCst) > queries).await;
        client.shutdown().await.unwrap();
        assert!(!client.healthy());
        assert_eq!(collector.observations.records.load(Ordering::SeqCst), 512);
        assert_eq!(collector.observations.accepts.load(Ordering::SeqCst), accepts);
    }
    if local.is_private() {
        *dns.answers.lock() = vec![IpAddr::V4(local)];
        configure_egress_defaults(true, Duration::from_secs(5));
        let queries = dns.queries.load(Ordering::SeqCst);
        let accepts = collector.observations.accepts.load(Ordering::SeqCst);
        let client = build_client_with_resolver("transport/private", cfg.clone(), dns.resolver());
        assert!(client.init().await.is_err());
        wait_until(|| dns.queries.load(Ordering::SeqCst) > queries).await;
        client.shutdown().await.unwrap();
        assert!(!client.healthy());
        assert_eq!(collector.observations.accepts.load(Ordering::SeqCst), accepts);
    }
    dns.shutdown().await;
    collector.shutdown().await;
}

#[test]
fn production_dns_rebinding_and_policy() {
    const CHILD: &str = "EDGION_OTLP_PRODUCTION_DNS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(production_dns_case());
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "production_dns_rebinding_and_policy", "--nocapture"])
        .env(CHILD, "1")
        .env(
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
            "authorization=Bearer%20unapproved-token",
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "production DNS child failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_candidate_auth_and_mtls_preserve_old_delivery() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    let (ca, cert, key) = tls_material();
    let (_, wrong_cert, wrong_key) = tls_material();
    let collector = Collector::start(
        Ipv4Addr::LOCALHOST,
        Mode::Success,
        Some(server_tls(&ca, &cert, &key, true)),
    )
    .await;
    collector.require_authorization("Bearer approved-token");
    let mut good = config(format!("https://localhost:{}", collector.address.port()), 1000);
    good.tls = Some(client_tls(&ca, Some((&cert, &key))));
    good.auth = Some(OtlpAuth {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                name: "auth".into(),
                namespace: None,
                group: None,
                kind: None,
            },
            secret: Some(Secret {
                metadata: kube::api::ObjectMeta {
                    name: Some("auth".into()),
                    namespace: Some("transport".into()),
                    ..Default::default()
                },
                data: Some([(String::from("bearerToken"), ByteString(b"approved-token".to_vec()))].into()),
                ..Default::default()
            }),
        },
    });
    let client = build_client("transport/retain", good.clone());
    client.init().await.unwrap();
    let mut delivered = 0;
    for wrong_identity in [None, Some((wrong_cert.as_str(), wrong_key.as_str()))] {
        let before = collector.observations.tls_failures.load(Ordering::SeqCst);
        let mut candidate = good.clone();
        candidate.tls = Some(client_tls(&ca, wrong_identity));
        let candidate = build_client("transport/retain-candidate", candidate);
        assert!(candidate.init().await.is_err());
        candidate.shutdown().await.unwrap();
        wait_until(|| collector.observations.tls_failures.load(Ordering::SeqCst) > before).await;
        assert!(client.healthy());
        emit(&client, 512);
        delivered += 512;
        wait_until(|| collector.observations.records.load(Ordering::SeqCst) == delivered).await;
    }
    let before = collector.observations.requests.load(Ordering::SeqCst);
    let mut candidate = good.clone();
    candidate
        .auth
        .as_mut()
        .unwrap()
        .slot
        .secret
        .as_mut()
        .unwrap()
        .data
        .as_mut()
        .unwrap()
        .insert("bearerToken".into(), ByteString(b"wrong-token".to_vec()));
    let candidate = build_client("transport/retain-auth-candidate", candidate);
    assert!(candidate.init().await.is_err());
    candidate.shutdown().await.unwrap();
    wait_until(|| collector.observations.requests.load(Ordering::SeqCst) > before).await;
    assert!(client.healthy());
    emit(&client, 512);
    delivered += 512;
    wait_until(|| collector.observations.records.load(Ordering::SeqCst) == delivered).await;
    client.shutdown().await.unwrap();
    assert_eq!(collector.observations.records.load(Ordering::SeqCst), 1536);
    collector.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_preflight_is_unpublished_and_shutdown_cancels_it() {
    edgion_common::test_mode::init_integration_testing_mode(true);
    let collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::StallBody, None).await;
    let client = build_client(
        "transport/pending",
        config(format!("http://{}", collector.address), 300000),
    );
    assert!(client.init().await.is_err());
    wait_until(|| collector.observations.requests.load(Ordering::SeqCst) > 0).await;
    assert!(!client.healthy());
    assert!(client
        .send_with_meta("otlp-test-record".into(), &LogMeta::default())
        .is_err());
    tokio::time::timeout(Duration::from_secs(1), client.shutdown())
        .await
        .unwrap()
        .unwrap();
    wait_until(|| collector.observations.closes.load(Ordering::SeqCst) > 0).await;
    assert_eq!(collector.observations.records.load(Ordering::SeqCst), 0);
    collector.shutdown().await;
}

async fn same_key_lifecycle_case() {
    use edgion_gateway::link_sys::runtime::store::{
        get_global_link_sys_store, get_httpdns_client, get_otlp_client, otlp_client_count, otlp_retiring_client_count,
        shutdown_otlp_clients,
    };
    use edgion_resources::resources::link_sys::LinkSys;
    use std::collections::{HashMap, HashSet};
    edgion_common::test_mode::init_integration_testing_mode(true);
    let old_collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
    let intermediate = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
    let latest = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
    let resource = |port: u16| -> LinkSys {
        serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1","kind":"LinkSys",
            "metadata":{"namespace":"transport","name":"recreated"},
            "spec":{"type":"otlp","config":{"endpoint":format!("http://127.0.0.1:{port}"),"timeoutMs":300000}}
        }))
        .unwrap()
    };
    let key = String::from("transport/recreated");
    let store = get_global_link_sys_store();
    store.replace_all(HashMap::from([(key.clone(), resource(old_collector.address.port()))]));
    wait_until(|| get_otlp_client(&key).is_some_and(|client| client.healthy())).await;
    let old = get_otlp_client(&key).unwrap();
    old_collector.set_mode(Mode::StallBody);
    let before = old_collector.observations.requests.load(Ordering::SeqCst);
    emit(&old, 512);
    wait_until(|| old_collector.observations.requests.load(Ordering::SeqCst) > before).await;
    store.update(HashMap::new(), &HashSet::from([key.clone()]));
    wait_until(|| get_otlp_client(&key).is_none()).await;
    assert_eq!(otlp_client_count(), 0);
    assert_eq!(otlp_retiring_client_count(), 1);
    for index in 0..32 {
        let port = if index % 2 == 0 {
            intermediate.address.port()
        } else {
            latest.address.port()
        };
        store.update(HashMap::from([(key.clone(), resource(port))]), &HashSet::new());
    }
    // Simple-removal contract: retirement never blocks same-key recreation, so
    // brief overlap is accepted; the runner still publishes only the last source.
    assert_eq!(intermediate.observations.accepts.load(Ordering::SeqCst), 0);
    assert_eq!(latest.observations.accepts.load(Ordering::SeqCst), 0);
    wait_until(|| otlp_retiring_client_count() == 0 && get_otlp_client(&key).is_some_and(|client| client.healthy()))
        .await;
    let current = get_otlp_client(&key).unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    emit(&current, 1);
    wait_until(|| latest.observations.records.load(Ordering::SeqCst) == 1).await;
    assert_eq!(intermediate.observations.accepts.load(Ordering::SeqCst), 0);
    assert_eq!(otlp_client_count(), 1);

    latest.set_mode(Mode::StallBody);
    let before = latest.observations.requests.load(Ordering::SeqCst);
    emit(&current, 512);
    wait_until(|| latest.observations.requests.load(Ordering::SeqCst) > before).await;
    store.update(HashMap::new(), &HashSet::from([key.clone()]));
    store.update(
        HashMap::from([(key.clone(), resource(intermediate.address.port()))]),
        &HashSet::new(),
    );
    let other: LinkSys = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"namespace":"transport","name":"recreated"},
        "spec":{"type":"httpdns","config":{"urlTemplate":"https://dns.example.test/{domain}"}}
    }))
    .unwrap();
    store.update(HashMap::from([(key.clone(), other)]), &HashSet::new());
    wait_until(|| otlp_retiring_client_count() == 0 && get_otlp_client(&key).is_none()).await;
    assert!(get_otlp_client(&key).is_none());
    assert_eq!(intermediate.observations.accepts.load(Ordering::SeqCst), 0);

    store.update(
        HashMap::from([(key.clone(), resource(intermediate.address.port()))]),
        &HashSet::new(),
    );
    wait_until(|| get_otlp_client(&key).is_some_and(|client| client.healthy())).await;
    let unrelated_key = String::from("transport/shutdown-unrelated");
    let unrelated: LinkSys = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1","kind":"LinkSys",
        "metadata":{"namespace":"transport","name":"shutdown-unrelated"},
        "spec":{"type":"httpdns","config":{"urlTemplate":"https://dns.example.test/{domain}"}}
    }))
    .unwrap();
    tokio::join!(
        async {
            for _ in 0..32 {
                store.update(
                    HashMap::from([
                        (key.clone(), resource(latest.address.port())),
                        (unrelated_key.clone(), unrelated.clone()),
                    ]),
                    &HashSet::new(),
                );
                tokio::task::yield_now().await;
            }
        },
        shutdown_otlp_clients()
    );
    assert_eq!(otlp_client_count(), 0);
    assert_eq!(otlp_retiring_client_count(), 0);
    assert!(get_httpdns_client(&unrelated_key).is_some());
    let before = latest.observations.accepts.load(Ordering::SeqCst);
    store.update(
        HashMap::from([(key.clone(), resource(latest.address.port()))]),
        &HashSet::new(),
    );
    assert!(get_otlp_client(&key).is_none());
    shutdown_otlp_clients().await;
    assert_eq!(latest.observations.accepts.load(Ordering::SeqCst), before);
    old_collector.shutdown().await;
    intermediate.shutdown().await;
    latest.shutdown().await;
}

#[test]
fn same_key_recreation_overlaps_retirement_and_last_source_wins() {
    const CHILD: &str = "EDGION_OTLP_LIFECYCLE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(same_key_lifecycle_case());
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "same_key_recreation_overlaps_retirement_and_last_source_wins",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "lifecycle child failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

async fn hostile_transport_logging_case() {
    use edgion_common::observe::logs::{init_logging, LogConfig};

    edgion_common::test_mode::init_integration_testing_mode(true);
    let directory = tempfile::tempdir().unwrap();
    let guard = init_logging(LogConfig {
        log_dir: directory.path().to_path_buf(),
        file_prefix: "transport".into(),
        json_format: true,
        console: false,
        level: "info".into(),
    })
    .await
    .unwrap();
    const HOSTNAME: &str = "localhost";
    const CERTIFICATE_NAME: &str = "private-certificate-sentinel.example.test";
    const TOKEN: &str = "private-authorization-sentinel";
    let (ca, cert, key) = tls_material_for_name(CERTIFICATE_NAME);
    let collector = Collector::start(
        Ipv4Addr::LOCALHOST,
        Mode::Success,
        Some(server_tls(&ca, &cert, &key, false)),
    )
    .await;
    let mut cfg = config(format!("https://{HOSTNAME}:{}", collector.address.port()), 500);
    cfg.tls = Some(client_tls(&ca, None));
    // Exercise ordinary production construction and its System resolver. Custom
    // DNS selection exists only in the separate policy test's test-util seam.
    let client = build_client("transport/private-tls", cfg);
    assert!(client.init().await.is_err());
    wait_until(|| collector.observations.tls_failures.load(Ordering::SeqCst) > 0).await;
    assert!(!client.healthy());
    assert_eq!(collector.observations.requests.load(Ordering::SeqCst), 0);
    client.shutdown().await.unwrap();
    collector.shutdown().await;

    for mode in [Mode::HostileHeaders, Mode::HostileTrailers, Mode::HostileBody] {
        let collector = Collector::start(Ipv4Addr::LOCALHOST, Mode::Success, None).await;
        collector.require_authorization(&format!("Bearer {TOKEN}"));
        let mut cfg = config(format!("http://{}", collector.address), 500);
        cfg.auth = Some(OtlpAuth {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    name: "auth".into(),
                    namespace: None,
                    group: None,
                    kind: None,
                },
                secret: Some(Secret {
                    metadata: kube::api::ObjectMeta {
                        name: Some("auth".into()),
                        namespace: Some("transport".into()),
                        ..Default::default()
                    },
                    data: Some([(String::from("bearerToken"), ByteString(TOKEN.as_bytes().to_vec()))].into()),
                    ..Default::default()
                }),
            },
        });
        let client = build_client("transport/private-status", cfg);
        client.init().await.unwrap();
        collector.set_mode(mode);
        emit(&client, 512);
        wait_until(|| collector.observations.hostile_responses.load(Ordering::SeqCst) == 1).await;
        client.shutdown().await.unwrap();
        assert_eq!(collector.observations.records.load(Ordering::SeqCst), 512);
        wait_until(|| collector.observations.closes.load(Ordering::SeqCst) > 0).await;
        collector.shutdown().await;
    }

    // Flush the actual canonical nonblocking writer after all transport owners
    // and fixture tasks have joined. A production health transition proves this
    // is a live capture, not an empty subscriber or suppressed test logger.
    drop(guard);
    let output = std::fs::read_to_string(directory.path().join("transport.log")).unwrap();
    assert!(output.contains("link-sys recovered"));
    assert!(output.contains("transport/private-status"));
    for forbidden in [
        HOSTNAME,
        "127.0.0.1",
        CERTIFICATE_NAME,
        TOKEN,
        REMOTE_MESSAGE,
        std::str::from_utf8(REMOTE_BODY).unwrap(),
        "BEGIN CERTIFICATE",
        "BEGIN PRIVATE KEY",
    ] {
        assert!(
            !output.contains(forbidden),
            "sensitive transport material reached system logs: {forbidden}; matching lines: {:?}",
            output
                .lines()
                .filter(|line| line.contains(forbidden))
                .collect::<Vec<_>>()
        );
    }
    // PEM contents can also be formatted without their envelope.
    assert!(!output.contains(cert.lines().nth(1).unwrap()));
    assert!(!output.contains(key.lines().nth(1).unwrap()));
}

#[test]
fn trace_logging_redacts_real_tls_and_hostile_grpc_failures() {
    const CHILD: &str = "EDGION_OTLP_LOGGING_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(hostile_transport_logging_case());
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "trace_logging_redacts_real_tls_and_hostile_grpc_failures",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("RUST_LOG", "trace")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "logging child failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

async fn captured_global_ca_case() {
    use edgion_common::link_sys_core::outbound_tls::{
        outbound_global_default, set_outbound_global_default, GlobalOutboundTlsSnapshot,
    };
    use edgion_resources::resources::common::OutboundTlsConfig;
    struct Restore(Arc<GlobalOutboundTlsSnapshot>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_outbound_global_default(self.0.clone());
        }
    }
    let _restore = Restore(outbound_global_default());
    edgion_common::test_mode::init_integration_testing_mode(true);
    let (ca, cert, key) = tls_material();
    let collector = Collector::start(
        Ipv4Addr::LOCALHOST,
        Mode::Success,
        Some(server_tls(&ca, &cert, &key, false)),
    )
    .await;
    let material = client_tls(&ca, None);
    let mut validation = material.outbound.validation.unwrap();
    validation.ca_certificate_refs.as_mut().unwrap()[0].namespace = Some("transport".into());
    let mut global: edgion_resources::EdgionGatewayConfig =
        serde_json::from_value(serde_json::json!({"metadata":{"name":"captured"},"spec":{}})).unwrap();
    global.spec.outbound_tls = Some(OutboundTlsConfig {
        validation: Some(validation),
        resolved_ca_certificates: material.resolved_ca_certificates,
        ..Default::default()
    });
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(global)])));
    let actual = Arc::new(otlp_resource(
        "transport/captured",
        config(format!("https://localhost:{}", collector.address.port()), 1_000),
    ));
    let client = OtlpLinkClient::from_link_sys(actual.clone()).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({"metadata":{"name":"invalid","namespace":"transport"},"spec":{"outboundTls":{"verify":true}}})).unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    assert!(
        OtlpLinkClient::from_link_sys(actual).is_err(),
        "new input must reject invalid inherited source"
    );
    client.init().await.unwrap();
    emit(&client, 1);
    client.shutdown().await.unwrap();
    assert_eq!(collector.observations.records.load(Ordering::SeqCst), 1);
    assert!(collector
        .observations
        .snis
        .lock()
        .iter()
        .any(|name| name == "localhost"));
    collector.shutdown().await;
}

#[test]
fn real_https_delivery_retains_captured_global_ca_after_invalid_publication() {
    const CHILD: &str = "EDGION_OTLP_CAPTURED_CA_CHILD";
    const TEST: &str = "real_https_delivery_retains_captured_global_ca_after_invalid_publication";
    if std::env::var(CHILD).as_deref() == Ok(TEST) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(captured_global_ca_case());
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, TEST)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
}
