//! Transport-seam integration exception: Controller admission rejects TLS/Terminate,
//! so the shared configured Gateway/Controller topology cannot reach this builder.
//! This test owns one in-process Pingora service, no product process or Controller,
//! and verifies only the existing Gateway TLS-to-TCP transport contract.
#![cfg(all(unix, feature = "boringssl"))]

use async_trait::async_trait;
use boring::ssl::{SslConnector, SslMethod, SslVerifyMode};
use edgion_gateway::config::bind_gateway_config;
use edgion_gateway::observe::AccessLogger;
use edgion_gateway::runtime::server::downstream_h2::DownstreamH2Policy;
use edgion_gateway::runtime::server::listener_builder::{
    add_tls_terminate_to_tcp_listener, ListenerContext, TlsOffloadBudget,
};
use edgion_gateway::runtime::server::proxy_protocol_policy::PreparedProxyProtocolPolicy;
use edgion_gateway::tls::store::get_global_tls_store;
use edgion_resources::{EdgionTls, ListenerKey};
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::{RunArgs, Server, ShutdownSignal, ShutdownSignalWatch};
use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

struct Shutdown(Arc<Notify>);

#[async_trait]
impl ShutdownSignalWatch for Shutdown {
    async fn recv(&self) -> ShutdownSignal {
        self.0.notified().await;
        ShutdownSignal::GracefulTerminate
    }
}

// Installed before identity publication, so every subsequent error/panic clears
// the owned identity and shuts down any started service. notify_one stores a
// permit even if Pingora has not yet started polling its shutdown watcher.
struct Owner {
    stop: Arc<Notify>,
    completion: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Owner {
    fn clear_inputs(&self) {
        get_global_tls_store().replace_all_tls(HashMap::new());
        edgion_gateway::runtime::create_gateway_handler().full_set(&HashMap::new());
        edgion_gateway::config::create_gateway_class_handler().full_set(&HashMap::new());
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.stop.notify_one();
        if let Some(thread) = self.thread.take() {
            let completed = !matches!(
                self.completion.recv_timeout(Duration::from_secs(10)),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            self.clear_inputs();
            if completed {
                let joined = thread.join();
                if !std::thread::panicking() {
                    joined.expect("owned Pingora service must stop cleanly");
                }
            } else {
                // Rust cannot safely kill a live thread. Fail this isolated test
                // process rather than detach a server after assertion unwinding.
                eprintln!("owned Pingora service did not stop within 10 seconds");
                std::process::abort();
            }
        }
        self.clear_inputs();
    }
}

#[test]
fn tls_to_tcp_builder_does_not_advertise_http_alpn() {
    // Dynamic ephemeral selection, with the builder's normal bind probe retained.
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = reservation.local_addr().unwrap();
    let listener_key = ListenerKey::new(address, "TLS");
    let mut gateway: edgion_resources::Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {
            "name": "gateway", "namespace": "alpn-test", "uid": "alpn-test-gateway",
            "generation": 1, "resourceVersion": "1",
            "annotations": {"edgion.io/local-bind-ip": address.ip().to_string()}
        },
        "spec": {"gatewayClassName": "alpn-test-class", "listeners": [{
            "name": "tls", "protocol": "TLS", "port": address.port(), "tls": {"mode": "Terminate"}
        }]}
    }))
    .unwrap();
    // Emulate the resolved Controller publication, which includes the serving
    // listener status. Missing current_status is deliberately ineligible.
    gateway.spec.current_status = Some(
        serde_json::from_value(serde_json::json!({
            "listeners": [{"name": "tls", "attachedRoutes": 0, "supportedKinds": [], "conditions": [{
                "type": "Programmed", "status": "True", "observedGeneration": 1,
                "reason": "Programmed", "message": "Listener is programmed",
                "lastTransitionTime": "2026-10-13T00:00:00Z"
            }]}]
        }))
        .unwrap(),
    );
    // This transport fixture supplies a processed Controller publication proof;
    // it does not exercise Controller admission or attachment-digest generation.
    gateway.spec.resolved_attachment_proof = Some(edgion_resources::resources::edgion_tls::GatewayAttachmentProof {
        instance: gateway.metadata.uid.clone().expect("fixture Gateway UID"),
        generation: gateway.metadata.generation.expect("fixture Gateway generation"),
        epoch: "alpn-test-controller-publication".into(),
        attachment_digest: "a".repeat(64),
    });
    let attachment_proof = edgion_resources::resources::gateway::gateway_attachment_proof(&gateway);
    assert!(
        attachment_proof.is_resolved(),
        "fixture must carry a resolved publication proof"
    );
    let class: edgion_resources::GatewayClass = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "alpn-test-class"},
        "spec": {"controllerName": "edgion.io/gateway-controller"}
    }))
    .unwrap();
    let certificate = rcgen::generate_simple_self_signed(vec!["test.example.com".into()]).unwrap();
    let mut tls: EdgionTls = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "alpn-test", "name": "identity"},
        "spec": {
            "hosts": ["test.example.com"],
            "secretRef": {"name": "identity"},
            "parentRefs": [{"name": "gateway", "sectionName": "tls"}]
        }
    }))
    .unwrap();
    tls.spec.secret = Some(k8s_openapi::api::core::v1::Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("identity".into()),
            namespace: Some("alpn-test".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (
                "tls.crt".into(),
                k8s_openapi::ByteString(certificate.cert.pem().into_bytes()),
            ),
            (
                "tls.key".into(),
                k8s_openapi::ByteString(certificate.signing_key.serialize_pem().into_bytes()),
            ),
        ])),
        ..Default::default()
    });
    tls.spec.resolved_listeners = Some(vec![listener_key.clone()]);
    tls.spec.resolved_listener_attachments = Some(vec![
        edgion_resources::resources::edgion_tls::ResolvedListenerAttachment {
            gateway_namespace: "alpn-test".into(),
            gateway_name: "gateway".into(),
            listener_name: "tls".into(),
            listener_key: listener_key.clone(),
            gateway_proof: attachment_proof,
        },
    ]);
    let validation = edgion_gateway::tls::validation::cert::validate_cert(&tls);
    assert!(
        validation.is_valid,
        "generated identity must validate: {:?}",
        validation.errors
    );

    let (completed, completion) = mpsc::channel();
    let mut owner = Owner {
        stop: Arc::new(Notify::new()),
        completion,
        thread: None,
    };
    // The production connection filter fails closed without an available
    // GatewayClass-scoped publication. Populate it through ordinary handlers;
    // never bypass admission just to make the handshake convenient to test.
    edgion_gateway::config::create_gateway_class_handler()
        .full_set(&HashMap::from([("alpn-test-class".into(), Arc::new(class))]));
    edgion_gateway::runtime::create_gateway_handler().full_set(&HashMap::from([(
        "alpn-test/gateway".into(),
        Arc::new(gateway.clone()),
    )]));
    get_global_tls_store().replace_all_tls(HashMap::from([("alpn-test/identity".into(), Arc::new(tls))]));
    assert_eq!(get_global_tls_store().get_cert_stats(), (1, 0));
    assert_eq!(
        get_global_tls_store()
            .snapshot()
            .match_sni_candidates(&listener_key, "test.example.com")
            .len(),
        1
    );
    assert!(get_global_tls_store()
        .snapshot()
        .match_sni_candidates(&listener_key, "test.example.com")[0]
        .identity
        .is_ok());
    let gateway_snapshot = edgion_gateway::runtime::matching::tls::get_gateway_tls_matcher().snapshot();
    assert!(
        get_global_tls_store()
            .snapshot()
            .match_eligible_sni(&listener_key, "test.example.com", |attachment| {
                gateway_snapshot.listener_is_eligible(attachment)
            })
            .is_some(),
        "published identity must satisfy the handshake's current Gateway attachment eligibility"
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let filter =
        edgion_gateway::plugins::stream::StreamPluginConnectionFilter::new(listener_key.clone(), address.port());
    assert!(
        runtime.block_on(filter.accept(Some(&address), 1)),
        "canonical publication must admit the owned listener"
    );
    assert!(
        !edgion_gateway::runtime::server::proxy_protocol_policy::get_global_proxy_protocol_policy_runtime().is_fatal()
    );
    drop(runtime);

    let conf = ServerConf {
        threads: 1,
        grace_period_seconds: Some(0),
        graceful_shutdown_timeout_seconds: Some(1),
        ..Default::default()
    };
    let mut server = Server::new_with_opt_and_conf(None, conf);
    let context = ListenerContext {
        gateway_namespace: Some("alpn-test".into()),
        gateway_name: "gateway".into(),
        gateway_key: "alpn-test/gateway".into(),
        listener: gateway.spec.listeners.unwrap().remove(0),
        listener_key,
        bind_ip: address.ip(),
        access_logger: Arc::new(AccessLogger::new()),
        egc: bind_gateway_config(None),
        server_conf: Arc::clone(&server.configuration),
        downstream_h2_policy: DownstreamH2Policy::try_from(&edgion_gateway::cli::config::ServerConfig::default())
            .unwrap(),
        downstream_connections: Arc::new(
            edgion_gateway::runtime::server::downstream_connections::ConnectionCounter::unlimited(),
        ),
        enable_http2: true, // HTTP frontend policy must not affect this transport.
        gateway_annotations: HashMap::new(),
        proxy_protocol_policy: PreparedProxyProtocolPolicy::Disabled,
        proxy_protocol_source: None,
    };
    drop(reservation);
    add_tls_terminate_to_tcp_listener(
        &mut server,
        &context,
        &mut TlsOffloadBudget::new(&context.server_conf).unwrap(),
    )
    .unwrap();
    let stop = Arc::clone(&owner.stop);
    owner.thread = Some(thread::spawn(move || {
        server.run(RunArgs {
            shutdown_signal: Box::new(Shutdown(stop)),
        });
        let _ = completed.send(());
    }));

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("owned TLS listener did not become ready: {error}"),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut connector = SslConnector::builder(SslMethod::tls()).unwrap();
    connector.set_verify(SslVerifyMode::NONE);
    connector.set_alpn_protos(b"\x02h2\x08http/1.1").unwrap();
    let connection = connector.build().connect("test.example.com", stream).unwrap();
    assert_eq!(connection.ssl().selected_alpn_protocol(), None);
    drop(connection);
    drop(owner);
    assert_eq!(get_global_tls_store().entry_count(), 0);
}
