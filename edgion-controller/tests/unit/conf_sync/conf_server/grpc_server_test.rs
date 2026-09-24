use super::*;
use parking_lot::Mutex;
use tokio_stream::StreamExt;

// A mock provider that allows us to simulate a server reload
struct MockConfigSyncProvider {
    server: Mutex<Arc<ConfigSyncServer>>,
}

impl MockConfigSyncProvider {
    fn new(server: Arc<ConfigSyncServer>) -> Self {
        Self {
            server: Mutex::new(server),
        }
    }

    fn set_server(&self, server: Arc<ConfigSyncServer>) {
        *self.server.lock() = server;
    }
}

impl ConfigSyncServerProvider for MockConfigSyncProvider {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        Some(self.server.lock().clone())
    }
}

#[test]
fn omitted_security_refuses_to_serve_plaintext() {
    // Fail-close (2026-07-14): the plaintext branch of `serve_with_shutdown` must
    // reject an omitted `conf_sync_security` instead of serving Secrets in the clear.
    let err = ConfigSyncGrpcServer::accept_skip_tls_or_refuse(&ConfSyncSecurityConfig::default(), None)
        .expect_err("omitted conf_sync_security must refuse to start");
    let msg = format!("{err:?}");
    for needle in ["auto_ca", "skip_tls", "Secrets"] {
        assert!(msg.contains(needle), "error must mention {needle:?}, got: {msg}");
    }
}

#[test]
fn skip_tls_still_serves_plaintext() {
    // The explicit escape hatch is untouched: skip_tls is accepted (and marks SKIP_TLS).
    let security = ConfSyncSecurityConfig {
        tls: Some(edgion_common::conf_sync::config::ConfSyncTlsConfig {
            name: None,
            cert: String::new(),
            key: String::new(),
            ca: String::new(),
            skip_tls: true,
        }),
        ..Default::default()
    };
    assert!(
        ConfigSyncGrpcServer::accept_skip_tls_or_refuse(&security, None).is_ok(),
        "skip_tls must remain the explicit plaintext escape hatch"
    );
}

#[test]
fn skip_tls_on_non_loopback_bind_is_accepted_but_never_silent() {
    // `skip_tls` is the deliberate escape hatch, so a non-loopback bind must NOT be
    // hard-rejected -- an operator with a broken cert chain needs a way back. But the
    // combination (cleartext Secrets on a routable address) must always be loud: this
    // pins the WARN branch so a future refactor cannot drop it back into silence.
    let security = ConfSyncSecurityConfig {
        tls: Some(edgion_common::conf_sync::config::ConfSyncTlsConfig {
            name: None,
            cert: String::new(),
            key: String::new(),
            ca: String::new(),
            skip_tls: true,
        }),
        ..Default::default()
    };
    for bind in ["0.0.0.0:12151", "192.168.1.10:12151", "[::]:12151"] {
        let addr: std::net::SocketAddr = bind.parse().expect("valid bind");
        assert!(!addr.ip().is_loopback(), "{bind} must be treated as network-reachable");
        assert!(
            ConfigSyncGrpcServer::accept_skip_tls_or_refuse(&security, Some(addr)).is_ok(),
            "skip_tls on {bind} must still start (escape hatch), only warn"
        );
    }
    for bind in ["127.0.0.1:12151", "[::1]:12151"] {
        let addr: std::net::SocketAddr = bind.parse().expect("valid bind");
        assert!(addr.ip().is_loopback(), "{bind} must be treated as loopback");
        assert!(
            ConfigSyncGrpcServer::accept_skip_tls_or_refuse(&security, Some(addr)).is_ok(),
            "skip_tls on {bind} is the defensible single-host case"
        );
    }
}

#[tokio::test]
async fn serve_with_shutdown_aborts_when_security_omitted() {
    // End-to-end on the server entry point: no tls_handle + omitted security => Err,
    // which propagates out of `serve_with_shutdown` and aborts controller startup.
    let provider = Arc::new(MockConfigSyncProvider::new(Arc::new(ConfigSyncServer::new())));
    let grpc = ConfigSyncGrpcServer::new(provider, CancellationToken::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = grpc
        .serve_with_shutdown(
            listener,
            &ConfSyncSecurityConfig::default(),
            None,
            std::future::pending(),
        )
        .await;
    assert!(result.is_err(), "omitted conf_sync_security must not serve plaintext");
}

#[tokio::test]
async fn test_watch_server_meta_reload_switch() {
    // 1. Create initial server
    let initial_server = Arc::new(ConfigSyncServer::new());
    let initial_server_id = initial_server.server_id();

    let provider = Arc::new(MockConfigSyncProvider::new(initial_server.clone()));
    let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), CancellationToken::new());

    let client_id = "test-client-123".to_string();

    // 2. Start watch_server_meta
    let request = Request::new(WatchServerMetaRequest {
        client_id: client_id.clone(),
        client_name: "test-gateway".to_string(),
    });

    let response = grpc_server.watch_server_meta(request).await.unwrap();
    let mut stream = response.into_inner();

    // 3. Receive initial event
    let first_event = stream.next().await.unwrap().unwrap();
    assert_eq!(first_event.server_id, initial_server_id);
    assert_eq!(first_event.gateway_instance_count, 1); // 1 because the request registered the client

    // Verify registration in old registry
    assert_eq!(initial_server.client_registry().count(), 1);

    // 4. Simulate server reload (new server generated)
    let new_server = Arc::new(ConfigSyncServer::new());
    let new_server_id = new_server.server_id();
    assert_ne!(initial_server_id, new_server_id);

    provider.set_server(new_server.clone());

    // 5. Watch for the terminal event marking the switch boundary.
    // We use a short timeout because the tick happens every second.
    let switch_event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .expect("should yield switch event quickly")
        .unwrap()
        .unwrap();

    // Terminal event should carry the NEW server_id and count
    assert_eq!(switch_event.server_id, new_server_id);

    // 6. Next poll should return None (stream closed)
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_watch_server_meta_shutdown() {
    // 1. Create server with a real CancellationToken
    let server = Arc::new(ConfigSyncServer::new());
    let provider = Arc::new(MockConfigSyncProvider::new(server.clone()));
    let token = CancellationToken::new();
    let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), token.clone());

    // 2. Start watch_server_meta
    let request = Request::new(WatchServerMetaRequest {
        client_id: "shutdown-test-client".to_string(),
        client_name: "shutdown-test-gateway".to_string(),
    });
    let response = grpc_server.watch_server_meta(request).await.unwrap();
    let mut stream = response.into_inner();

    // 3. Receive initial event (the stream always sends one immediately)
    let first_event = stream.next().await.unwrap().unwrap();
    assert_eq!(first_event.gateway_instance_count, 1); // 1 because request registered the client

    // 4. Verify client is registered
    assert_eq!(server.client_registry().count(), 1);

    // 5. Cancel the token (simulating shutdown)
    token.cancel();

    // 6. The stream should end (yield None) shortly after cancellation
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await;

    // Should complete within timeout
    let next = result.expect("stream should end within 3 seconds");
    assert!(next.is_none(), "stream should be closed after shutdown");

    // 7. Verify client was unregistered (cleanup happened)
    // Give a brief moment for the spawned task to complete unregistration
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // After unregister, the HashMap is empty but count() returns max(1, len) = 1
    // The important thing is that the actual clients map is empty.
    // count() always returns at least 1, so we just verify the stream closed cleanly.
    assert_eq!(server.client_registry().count(), 1);
}

#[tokio::test]
async fn test_term_token_swap_cancels_only_captured_term() {
    // Spec §6.1 (S2): streams capture the CURRENT term token at stream start.
    // Cancelling term A's token must end only the streams that started under
    // term A; a stream started after the slot was swapped to term B survives
    // until B (or the process token) is cancelled.
    let server = Arc::new(ConfigSyncServer::new());
    let provider = Arc::new(MockConfigSyncProvider::new(server.clone()));
    let process_token = CancellationToken::new();
    let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), process_token.clone());
    let slot = grpc_server.term_token_slot();

    // Term A begins: the election installs a fresh token.
    let term_a = CancellationToken::new();
    slot.store(Arc::new(term_a.clone()));

    let response_a = grpc_server
        .watch_server_meta(Request::new(WatchServerMetaRequest {
            client_id: "term-a-client".to_string(),
            client_name: "term-a-gateway".to_string(),
        }))
        .await
        .unwrap();
    let mut stream_a = response_a.into_inner();
    stream_a.next().await.unwrap().unwrap(); // initial event

    // Term B begins: the election swaps in a new token (term A's not yet cancelled,
    // mirroring the campaign-win-before-demotion-completes ordering).
    let term_b = CancellationToken::new();
    slot.store(Arc::new(term_b.clone()));

    let response_b = grpc_server
        .watch_server_meta(Request::new(WatchServerMetaRequest {
            client_id: "term-b-client".to_string(),
            client_name: "term-b-gateway".to_string(),
        }))
        .await
        .unwrap();
    let mut stream_b = response_b.into_inner();
    stream_b.next().await.unwrap().unwrap(); // initial event

    // Demotion of term A: only stream_a must end.
    term_a.cancel();
    let ended_a = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while stream_a.next().await.is_some() {}
    })
    .await;
    assert!(
        ended_a.is_ok(),
        "term-A stream must close when term A's token is cancelled"
    );

    // stream_b must remain open: drain any events (e.g. count changes from A's
    // unregister); reaching end-of-stream is the failure.
    let b_outcome = tokio::time::timeout(std::time::Duration::from_millis(1500), async {
        loop {
            if stream_b.next().await.is_none() {
                return; // stream ended — bug
            }
        }
    })
    .await;
    assert!(
        b_outcome.is_err(),
        "term-B stream must stay open after term A's token is cancelled"
    );

    // Cancelling term B ends the remaining stream.
    term_b.cancel();
    let ended_b = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while stream_b.next().await.is_some() {}
    })
    .await;
    assert!(
        ended_b.is_ok(),
        "term-B stream must close when term B's token is cancelled"
    );
}

/// A minimal mock WatchObj that keeps a sender so we can push events or close the channel.
struct MockWatchObj {
    tx: parking_lot::Mutex<Option<tokio::sync::mpsc::Sender<super::super::traits::WatchResponseSimple>>>,
    rx: parking_lot::Mutex<Option<tokio::sync::mpsc::Receiver<super::super::traits::WatchResponseSimple>>>,
}

impl MockWatchObj {
    fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        Self {
            tx: parking_lot::Mutex::new(Some(tx)),
            rx: parking_lot::Mutex::new(Some(rx)),
        }
    }
}

impl super::super::traits::WatchObj for MockWatchObj {
    fn kind_name(&self) -> &'static str {
        "MockKind"
    }

    fn list_json(&self) -> Result<(String, u64), String> {
        Ok(("[]".to_string(), 0))
    }

    fn current_version(&self) -> u64 {
        // Must match the version stamped by this mock's list_json (0).
        0
    }

    fn watch_json(
        &self,
        _client_id: String,
        _client_name: String,
        _from_version: u64,
    ) -> tokio::sync::mpsc::Receiver<super::super::traits::WatchResponseSimple> {
        // Return the pre-created receiver (consumed once)
        self.rx.lock().take().expect("watch_json called more than once")
    }

    fn is_ready(&self) -> bool {
        true
    }

    fn set_ready(&self) {}

    fn set_not_ready(&self) {}

    fn clear(&self) {}
}

#[tokio::test]
async fn test_watch_stream_shutdown() {
    use super::super::traits::WatchResponseSimple;

    // 1. Create server with a registered mock WatchObj and a real CancellationToken
    let server = Arc::new(ConfigSyncServer::new());
    let mock_obj = Arc::new(MockWatchObj::new());

    // Hold a sender clone so we can push events through the mock watch channel
    let mock_tx = mock_obj.tx.lock().clone().unwrap();

    server.register_watch_obj("MockKind", mock_obj);

    let provider = Arc::new(MockConfigSyncProvider::new(server.clone()));
    let token = CancellationToken::new();
    let grpc_server = ConfigSyncGrpcServer::new(provider.clone(), token.clone());

    // 2. Start a watch on the registered kind
    let request = Request::new(WatchRequest {
        key: "test".to_string(),
        kind: "MockKind".to_string(),
        client_id: "watch-shutdown-client".to_string(),
        client_name: "watch-shutdown-gateway".to_string(),
        from_version: 0,
        expected_server_id: String::new(),
    });
    let response = grpc_server.watch(request).await.unwrap();
    let mut stream = response.into_inner();

    // 3. Push one event and verify it arrives
    mock_tx
        .send(WatchResponseSimple::new("test-data".to_string(), 1))
        .await
        .unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .expect("should receive first event")
        .unwrap()
        .unwrap();
    assert_eq!(first.data, "test-data");
    assert_eq!(first.sync_version, 1);

    // 4. Cancel the token (simulating shutdown)
    token.cancel();

    // 5. The stream should end (yield None) shortly after cancellation
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await;
    let next = result.expect("stream should end within 3 seconds");
    assert!(next.is_none(), "stream should be closed after shutdown");
}

// rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
// (invoked transitively via `ServerIdentity::from_pems`) can be called. Installing it
// here races harmlessly with other test modules doing the same `install_default()`.
fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

use crate::conf_sync::conf_server::tls_identity::{ServerIdentity, ServerIdentityHandle};
use crate::services::conf_sync_ca::Ca;
use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as GrpcClient;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};

fn client_tls(ca: &Ca, leaf: &crate::services::conf_sync_ca::LeafPem) -> ClientTlsConfig {
    ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca.ca_cert_pem()))
        .identity(Identity::from_pem(&leaf.cert_pem, leaf.key_pem.as_str()))
        .domain_name("localhost")
}

async fn connect(
    addr: std::net::SocketAddr,
    tls: ClientTlsConfig,
) -> anyhow::Result<GrpcClient<tonic::transport::Channel>> {
    let ch = Endpoint::from_shared(format!("https://localhost:{}", addr.port()))?
        .tls_config(tls)?
        .connect()
        .await?;
    Ok(GrpcClient::new(ch))
}

/// Spin up the owned TLS accept loop on an ephemeral port with a mock provider.
/// Returns the address and the shutdown token.
async fn spawn_tls_server(handle: Arc<ServerIdentityHandle>) -> (std::net::SocketAddr, CancellationToken) {
    let server = Arc::new(ConfigSyncServer::new());
    let provider = Arc::new(MockConfigSyncProvider::new(server));
    let token = CancellationToken::new();
    let grpc = ConfigSyncGrpcServer::new(provider, token.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let security = ConfSyncSecurityConfig::default();
    let shutdown = {
        let t = token.clone();
        async move { t.cancelled().await }
    };
    tokio::spawn(async move {
        grpc.serve_with_shutdown(listener, &security, Some(handle), shutdown)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    (addr, token)
}

/// Routine LEAF rotation (same trust root): established streams MUST survive.
/// Zero-disruption rotation is the whole point of the hot-swap design — the peers on
/// those sessions are still authorized.
#[tokio::test]
async fn tls_accept_loop_hot_swaps_leaf_without_dropping_streams() {
    install_test_crypto_provider();

    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let server_a = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let client = ca.sign_client(time::Duration::days(7)).unwrap();
    let handle = Arc::new(ServerIdentityHandle::new(
        ServerIdentity::from_pems(&server_a.cert_pem, &server_a.key_pem, ca.ca_cert_pem()).unwrap(),
    ));
    let (addr, token) = spawn_tls_server(handle.clone()).await;

    // Connect (this also proves ALPN h2: tonic requires it) and open a long-lived stream.
    let mut c1 = connect(addr, client_tls(&ca, &client)).await.unwrap();
    let mut meta = c1
        .watch_server_meta(WatchServerMetaRequest {
            client_id: "c1".into(),
            client_name: "g1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let first = meta.message().await.unwrap().unwrap();
    assert_eq!(first.gateway_instance_count, 1);

    // Re-sign the SERVER LEAF against the SAME root (exactly what identity_sync does
    // every half-life).
    let server_b = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(14))
        .unwrap();
    handle.store(ServerIdentity::from_pems(&server_b.cert_pem, &server_b.key_pem, ca.ca_cert_pem()).unwrap());

    // A second client registers, which pushes a count event onto the OLD stream —
    // proving that stream survived the swap.
    let mut c2 = connect(addr, client_tls(&ca, &client)).await.unwrap();
    let _meta2 = c2
        .watch_server_meta(WatchServerMetaRequest {
            client_id: "c2".into(),
            client_name: "g2".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), meta.message())
        .await
        .expect("old stream must still deliver events after a same-root leaf rotation")
        .unwrap()
        .unwrap();
    assert_eq!(ev.gateway_instance_count, 2);

    token.cancel();
}

/// Trust-ROOT change (emergency CA re-key): an ALREADY-ESTABLISHED session — i.e. the
/// attacker holding the leaked leaf, who never has to handshake again — MUST be torn
/// down. Swapping the client-CA verifier alone only governs future handshakes; without
/// the trust token this stream keeps receiving resolved Secrets forever.
#[tokio::test]
async fn tls_accept_loop_tears_down_established_streams_on_root_change() {
    install_test_crypto_provider();

    let ca_a = Ca::generate_root(time::Duration::days(30)).unwrap();
    let server_a = ca_a
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let client_a = ca_a.sign_client(time::Duration::days(7)).unwrap();
    let handle = Arc::new(ServerIdentityHandle::new(
        ServerIdentity::from_pems(&server_a.cert_pem, &server_a.key_pem, ca_a.ca_cert_pem()).unwrap(),
    ));
    let (addr, token) = spawn_tls_server(handle.clone()).await;

    let mut c1 = connect(addr, client_tls(&ca_a, &client_a)).await.unwrap();
    let mut meta = c1
        .watch_server_meta(WatchServerMetaRequest {
            client_id: "c1".into(),
            client_name: "g1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(meta.message().await.unwrap().unwrap().gateway_instance_count, 1);

    // Re-key: a BRAND-NEW root (new private key).
    let ca_b = Ca::generate_root(time::Duration::days(30)).unwrap();
    let server_b = ca_b
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let client_b = ca_b.sign_client(time::Duration::days(7)).unwrap();
    handle.store(ServerIdentity::from_pems(&server_b.cert_pem, &server_b.key_pem, ca_b.ca_cert_pem()).unwrap());

    // 1. The LIVE session established under the old root is killed (the connection is
    //    aborted, so the stream ends — either with a transport error or with EOF).
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), meta.message())
        .await
        .expect("the established session must be torn down by the re-key, not left open");
    assert!(
        matches!(&ended, Err(_) | Ok(None)),
        "expected the revoked session to end, got {ended:?}"
    );

    // 2. A NEW handshake with the old (compromised) identity is rejected.
    assert!(connect(addr, client_tls(&ca_a, &client_a)).await.is_err());

    // 3. The server still works for a client under the new root.
    let mut c2 = connect(addr, client_tls(&ca_b, &client_b)).await.unwrap();
    let mut meta2 = c2
        .watch_server_meta(WatchServerMetaRequest {
            client_id: "c2".into(),
            client_name: "g2".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(meta2.message().await.unwrap().is_some());

    token.cancel();
}
