//! Standalone transport regressions: two fake ConfigSync peers exercise client
//! failover without starting another shared Gateway/Controller topology. Their
//! readiness gate simulates the existing service gate, not a real leader election.

use edgion_common::conf_sync::config::ConfSyncSecurityConfig;
use edgion_common::conf_sync::proto::config_sync_server::{ConfigSync, ConfigSyncServer};
use edgion_common::conf_sync::proto::*;
use edgion_gateway::conf_sync::conf_client::ConfigSyncClient;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

type RpcStream<T> = Pin<Box<dyn futures::Stream<Item = Result<T, Status>> + Send>>;

struct Peer {
    id: &'static str,
    serving: AtomicBool,
    block_info: AtomicBool,
    release_info: Notify,
    info_calls: Mutex<Vec<SocketAddr>>,
    rejected: AtomicUsize,
    meta_starts: AtomicUsize,
    changed: watch::Sender<usize>,
    meta: Mutex<Option<mpsc::Sender<Result<ServerMetaEvent, Status>>>>,
}

impl Peer {
    fn new(id: &'static str) -> Arc<Self> {
        Arc::new(Self {
            id,
            serving: AtomicBool::new(true),
            block_info: AtomicBool::new(false),
            release_info: Notify::new(),
            info_calls: Mutex::new(Vec::new()),
            rejected: AtomicUsize::new(0),
            meta_starts: AtomicUsize::new(0),
            changed: watch::channel(0).0,
            meta: Mutex::new(None),
        })
    }

    fn calls(&self) -> usize {
        self.info_calls.lock().len()
    }

    async fn wait_for_calls(&self, count: usize) {
        let mut changes = self.changed.subscribe();
        tokio::time::timeout(Duration::from_secs(20), async {
            while self.calls() < count {
                changes.changed().await.unwrap();
            }
        })
        .await
        .expect("peer was not probed before deadline");
    }

    fn close_meta(&self) {
        self.meta.lock().take();
    }
}

#[derive(Clone)]
struct PeerService(Arc<Peer>);

#[tonic::async_trait]
impl ConfigSync for PeerService {
    async fn get_server_info(
        &self,
        request: Request<ServerInfoRequest>,
    ) -> Result<Response<ServerInfoResponse>, Status> {
        self.0.info_calls.lock().push(request.remote_addr().unwrap());
        self.0.changed.send_modify(|version| *version += 1);
        if self.0.block_info.load(Ordering::SeqCst) {
            self.0.release_info.notified().await;
        }
        if !self.0.serving.load(Ordering::SeqCst) {
            self.0.rejected.fetch_add(1, Ordering::SeqCst);
            self.0.changed.send_modify(|version| *version += 1);
            return Err(Status::unavailable("configuration service is not serving"));
        }
        Ok(Response::new(ServerInfoResponse {
            server_id: self.0.id.into(),
            supported_kinds: Vec::new(),
        }))
    }

    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        Err(Status::unimplemented(
            "resource synchronization is outside this transport test",
        ))
    }

    type WatchStream = RpcStream<WatchResponse>;
    async fn watch(&self, _: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        Err(Status::unimplemented(
            "resource synchronization is outside this transport test",
        ))
    }

    type WatchServerMetaStream = RpcStream<ServerMetaEvent>;
    async fn watch_server_meta(
        &self,
        _: Request<WatchServerMetaRequest>,
    ) -> Result<Response<Self::WatchServerMetaStream>, Status> {
        if !self.0.serving.load(Ordering::SeqCst) {
            return Err(Status::unavailable("configuration service is not serving"));
        }
        let (sender, receiver) = mpsc::channel(1);
        sender
            .try_send(Ok(ServerMetaEvent {
                server_id: self.0.id.into(),
                gateway_instance_count: 1,
                timestamp: 0,
            }))
            .unwrap();
        *self.0.meta.lock() = Some(sender);
        self.0.meta_starts.fetch_add(1, Ordering::SeqCst);
        self.0.changed.send_modify(|version| *version += 1);
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }
}

// Abort-on-drop also cleans up when a regression assertion panics.
struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(peer: Arc<Peer>) -> (String, Task) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let incoming = futures::stream::unfold(listener, |listener| async {
        let next = listener.accept().await.map(|(socket, _)| socket);
        Some((next, listener))
    });
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ConfigSyncServer::new(PeerService(peer)))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    (address, Task(task))
}

async fn wait_for_server(client: &ConfigSyncClient, id: &str) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while client.get_config_client().current_server_id() != id {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("metadata session did not reach the expected peer");
}

#[tokio::test]
async fn verified_candidate_reuses_connection_and_recovers_sessions() {
    // Keep all cases sequential: production reload generations are process-global.
    let a = Peer::new("peer-a");
    let b = Peer::new("peer-b");
    let (a_addr, _a_task) = serve(a.clone()).await;
    let (b_addr, _b_task) = serve(b.clone()).await;
    let security: ConfSyncSecurityConfig = serde_json::from_value(serde_json::json!({
        "certs": [{"name": "test", "skip_tls": true}]
    }))
    .unwrap();
    let client = Arc::new(
        ConfigSyncClient::new(
            vec![a_addr, b_addr],
            "failover-test".into(),
            Duration::from_secs(2),
            &security,
        )
        .await
        .unwrap(),
    );
    // Identify the initial peer without assuming the once-shuffled address order.
    let initial = client.get_server_info().await.unwrap();
    let (old, candidate) = if initial.server_id == a.id { (a, b) } else { (b, a) };
    old.serving.store(false, Ordering::SeqCst);
    candidate.block_info.store(true, Ordering::SeqCst);
    let bootstrap_client = client.clone();
    let mut bootstrap = tokio::spawn(async move { bootstrap_client.bootstrap_server_info().await });
    let bootstrap_abort = AbortOnDrop(bootstrap.abort_handle());
    candidate.wait_for_calls(1).await;

    // An in-flight candidate probe must not become the installed RPC client.
    let old_calls = old.calls();
    assert_eq!(
        client.get_server_info().await.unwrap_err().code(),
        tonic::Code::Unavailable
    );
    assert_eq!(old.calls(), old_calls + 1);
    assert_eq!(candidate.calls(), 1);
    candidate.block_info.store(false, Ordering::SeqCst);
    candidate.release_info.notify_one();
    let selected = tokio::time::timeout(Duration::from_secs(10), &mut bootstrap)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(selected.server_id, candidate.id);
    drop(bootstrap_abort);
    assert_eq!(client.get_server_info().await.unwrap().server_id, candidate.id);
    let sockets = candidate.info_calls.lock().clone();
    assert!(sockets.len() >= 2);
    assert!(
        sockets.iter().all(|socket| *socket == sockets[0]),
        "the probed channel must be reused"
    );

    let session = Task(tokio::spawn(client.clone().run_leader_session()));
    wait_for_server(&client, candidate.id).await;
    candidate.serving.store(false, Ordering::SeqCst);
    let old_rejections = old.rejected.load(Ordering::SeqCst);
    candidate.close_meta();
    let mut changes = old.changed.subscribe();
    tokio::time::timeout(Duration::from_secs(20), async {
        while old.rejected.load(Ordering::SeqCst) == old_rejections {
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("unavailable alternative must be rejected");
    // Both peers were unavailable. A later recovered peer must still be found.
    old.serving.store(true, Ordering::SeqCst);
    wait_for_server(&client, old.id).await;
    assert_eq!(client.get_server_info().await.unwrap().server_id, old.id);

    // The alternative remains unavailable, but the original active address can
    // serve again. Discovery must include it instead of skipping it forever.
    let candidate_calls = candidate.calls();
    let old_calls = old.calls();
    let old_sessions = old.meta_starts.load(Ordering::SeqCst);
    old.close_meta();
    candidate.wait_for_calls(candidate_calls + 1).await;
    old.wait_for_calls(old_calls + 1).await;
    let mut changes = old.changed.subscribe();
    tokio::time::timeout(Duration::from_secs(20), async {
        while old.meta_starts.load(Ordering::SeqCst) == old_sessions {
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("the original peer must establish a fresh metadata session");
    assert_eq!(client.get_server_info().await.unwrap().server_id, old.id);
    drop(session);
    stalled_unary_body_is_bounded(&security).await;
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn stalled_unary_body_is_bounded(security: &ConfSyncSecurityConfig) {
    // Send successful HTTP/2 response headers, then leave the unary body open.
    // Endpoint header timeouts alone cannot bound this complete RPC exchange.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let headers_sent = Arc::new(Notify::new());
    let headers_signal = headers_sent.clone();
    let _server = Task(tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(socket).await.unwrap();
        let mut open_streams = Vec::new();
        while let Some(request) = connection.accept().await {
            let (request, mut response) = request.unwrap();
            let headers = http::Response::builder()
                .status(200)
                .header("content-type", "application/grpc")
                .body(())
                .unwrap();
            open_streams.push((request, response.send_response(headers, false).unwrap()));
            headers_signal.notify_one();
        }
    }));
    let client = ConfigSyncClient::new(
        vec![address],
        "stalled-body".into(),
        Duration::from_millis(200),
        security,
    )
    .await
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), client.get_server_info())
        .await
        .expect("complete unary response body must have a deadline");
    tokio::time::timeout(Duration::from_secs(1), headers_sent.notified())
        .await
        .expect("the test must reach successful response headers before timing out");
    assert_eq!(result.unwrap_err().code(), tonic::Code::DeadlineExceeded);
}
