//! Standalone transport regression with two owned fake ConfigSync peers, not a
//! second Gateway/Controller topology. An oversized metadata response isolates
//! production client construction limits from resource parsing and publication.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_common::conf_sync::payload::MAX_MESSAGE_BYTES;
use edgion_common::conf_sync::proto::config_sync_server::{ConfigSync, ConfigSyncServer};
use edgion_common::conf_sync::proto::*;
use edgion_gateway::conf_sync::conf_client::{current_reload_generation, ConfigSyncClient};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response, Status};

const RESPONSE_BYTES: usize = 5 * 1024 * 1024;
type RpcStream<T> = Pin<Box<dyn futures::Stream<Item = Result<T, Status>> + Send>>;

struct Peer {
    id: &'static str,
    serving: AtomicBool,
}

#[derive(Clone)]
struct PeerService(Arc<Peer>);

#[tonic::async_trait]
impl ConfigSync for PeerService {
    async fn get_server_info(&self, _: Request<ServerInfoRequest>) -> Result<Response<ServerInfoResponse>, Status> {
        if !self.0.serving.load(Ordering::SeqCst) {
            return Err(Status::unavailable("test peer unavailable"));
        }
        Ok(Response::new(ServerInfoResponse {
            server_id: self.0.id.into(),
            supported_kinds: vec!["x".repeat(RESPONSE_BYTES)],
        }))
    }

    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        Err(Status::unimplemented("resource parsing is outside this transport test"))
    }

    type WatchStream = RpcStream<WatchResponse>;
    async fn watch(&self, _: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        Err(Status::unimplemented("watch lifecycle is outside this transport test"))
    }

    type WatchServerMetaStream = RpcStream<ServerMetaEvent>;
    async fn watch_server_meta(
        &self,
        _: Request<WatchServerMetaRequest>,
    ) -> Result<Response<Self::WatchServerMetaStream>, Status> {
        Err(Status::unimplemented(
            "metadata lifecycle is outside this transport test",
        ))
    }
}

struct ServerTask(JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(peer: Arc<Peer>, ca: &str, cert: &str, key: &str) -> (String, ServerTask) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let incoming = futures::stream::unfold(listener, |listener| async {
        let socket = listener.accept().await.map(|(socket, _)| socket);
        Some((socket, listener))
    });
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(Certificate::from_pem(ca));
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(ConfigSyncServer::new(PeerService(peer)).max_encoding_message_size(MAX_MESSAGE_BYTES))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    (address, ServerTask(task))
}

fn leaf(issuer: &rcgen::Issuer<'_, rcgen::KeyPair>) -> (String, String) {
    use rcgen::*;
    let mut params = CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().unwrap();
    (params.signed_by(&key, issuer).unwrap().pem(), key.serialize_pem())
}

fn assert_response(info: &ServerInfoResponse, expected_id: &str) {
    assert_eq!(info.server_id, expected_id);
    assert_eq!(info.supported_kinds.len(), 1);
    assert_eq!(info.supported_kinds[0].len(), RESPONSE_BYTES);
}

#[tokio::test]
async fn large_messages_survive_initial_recovery_and_tls_reload_clients() {
    use rcgen::*;

    rustls::crypto::ring::default_provider().install_default().unwrap();
    // One test keeps all channel generation mutations serial within this binary.
    // No tracing subscriber is installed: never print the oversized metadata.
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap().pem();
    let issuer = Issuer::new(ca_params, ca_key);
    let (cert, key) = leaf(&issuer);
    let directory = tempfile::tempdir().unwrap();
    let ca_path = directory.path().join("ca.pem");
    let cert_path = directory.path().join("client.pem");
    let key_path = directory.path().join("client.key");
    std::fs::write(&ca_path, &ca).unwrap();
    std::fs::write(&cert_path, &cert).unwrap();
    std::fs::write(&key_path, &key).unwrap();
    let security: ConfSyncSecurityConfig = serde_json::from_value(serde_json::json!({
        "certs": [{"name": "test", "cert": cert_path, "key": key_path, "ca": ca_path}]
    }))
    .unwrap();
    let a = Arc::new(Peer {
        id: "peer-a",
        serving: AtomicBool::new(true),
    });
    let b = Arc::new(Peer {
        id: "peer-b",
        serving: AtomicBool::new(true),
    });
    let (a_addr, _a_server) = serve(a.clone(), &ca, &cert, &key).await;
    let (b_addr, _b_server) = serve(b.clone(), &ca, &cert, &key).await;
    let client = Arc::new(
        ConfigSyncClient::new(
            vec![a_addr, b_addr],
            "payload-client-test".into(),
            Duration::from_secs(5),
            &security,
        )
        .await
        .unwrap(),
    );
    let initial = client.get_server_info().await.unwrap();
    let (old, next) = if initial.server_id == a.id { (a, b) } else { (b, a) };
    assert_response(&initial, old.id);
    drop(initial);

    old.serving.store(false, Ordering::SeqCst);
    let recovered = tokio::time::timeout(Duration::from_secs(20), client.bootstrap_server_info())
        .await
        .expect("recovery exceeded deadline")
        .unwrap();
    assert_response(&recovered, next.id);
    drop(recovered);

    // Replace the identity with a distinct valid key and certificate. The real
    // reload entry point validates files and publishes a freshly built client.
    let (renewed_cert, renewed_key) = leaf(&issuer);
    std::fs::write(&cert_path, renewed_cert).unwrap();
    std::fs::write(&key_path, renewed_key).unwrap();
    let generation = current_reload_generation();
    client.reload_client_tls().await.unwrap();
    assert!(generation.is_cancelled());
    assert_response(&client.get_server_info().await.unwrap(), next.id);
}
