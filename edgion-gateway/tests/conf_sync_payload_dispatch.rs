//! Real gRPC coverage of compressed payload publication through the Gateway cache.
//! An owned fake peer exercises the production cache, not a second full topology.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use edgion_common::conf_sync::{
    payload::{self, EncodedPayload, MAX_MESSAGE_BYTES},
    proto::{
        config_sync_client::ConfigSyncClient,
        config_sync_server::{ConfigSync, ConfigSyncServer},
        ListRequest, ListResponse, PayloadEncoding, ServerInfoRequest, ServerInfoResponse, ServerMetaEvent,
        WatchRequest, WatchResponse, WatchServerMetaRequest,
    },
    traits::ConfHandler,
};
use edgion_gateway::conf_sync::cache_client::ClientCache;
use k8s_openapi::api::core::v1::ConfigMap;
use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

type WatchSender = mpsc::Sender<Result<WatchResponse, Status>>;

struct Peer {
    snapshots: Mutex<VecDeque<ListResponse>>,
    list_started: mpsc::UnboundedSender<()>,
    release_list: Arc<Notify>,
    watch_started: mpsc::UnboundedSender<WatchSender>,
}

#[tonic::async_trait]
impl ConfigSync for Peer {
    async fn get_server_info(&self, _: Request<ServerInfoRequest>) -> Result<Response<ServerInfoResponse>, Status> {
        Ok(Response::new(ServerInfoResponse {
            server_id: "payload-test".into(),
            supported_kinds: vec!["ConfigMap".into()],
        }))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        assert_eq!(request.into_inner().kind, "ConfigMap");
        self.list_started.send(()).unwrap();
        self.release_list.notified().await;
        self.snapshots
            .lock()
            .pop_front()
            .map(Response::new)
            .ok_or_else(|| Status::unavailable("No test snapshot"))
    }

    type WatchStream = ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        let request = request.into_inner();
        assert_eq!(request.kind, "ConfigMap");
        assert_eq!(request.expected_server_id, "payload-test");
        let (tx, rx) = mpsc::channel(1);
        self.watch_started.send(tx).unwrap();
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type WatchServerMetaStream = ReceiverStream<Result<ServerMetaEvent, Status>>;

    async fn watch_server_meta(
        &self,
        _: Request<WatchServerMetaRequest>,
    ) -> Result<Response<Self::WatchServerMetaStream>, Status> {
        Err(Status::unimplemented("Unused in cache dispatch test"))
    }
}

struct PublicationProbe(mpsc::UnboundedSender<String>);

impl PublicationProbe {
    fn record(&self, values: &HashMap<String, Arc<ConfigMap>>) {
        if let Some(stage) = values.get("test/config").and_then(|cm| cm.data.as_ref()?.get("stage")) {
            self.0.send(stage.clone()).unwrap();
        }
    }
}

impl ConfHandler<ConfigMap> for PublicationProbe {
    fn full_set(&self, values: &HashMap<String, Arc<ConfigMap>>) {
        self.record(values);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<ConfigMap>>,
        update: HashMap<String, Arc<ConfigMap>>,
        _: HashSet<String>,
    ) {
        self.record(&add);
        self.record(&update);
    }
}

fn config(stage: &str) -> ConfigMap {
    ConfigMap {
        metadata: kube::core::ObjectMeta {
            name: Some("config".into()),
            namespace: Some("test".into()),
            ..Default::default()
        },
        // This models the generic FS transport; Kubernetes admission limits are unrelated.
        data: Some(BTreeMap::from([
            ("stage".into(), stage.into()),
            ("padding".into(), "x".repeat(33 * 1024 * 1024)),
        ])),
        ..Default::default()
    }
}

async fn snapshot(stage: &str, version: u64) -> ListResponse {
    let encoded = payload::encode(Bytes::from(serde_json::to_vec(&[config(stage)]).unwrap()))
        .await
        .unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Zstd as i32);
    ListResponse {
        data: encoded.data,
        encoding: encoded.encoding,
        uncompressed_size: encoded.uncompressed_size,
        sync_version: version,
        server_id: "payload-test".into(),
    }
}

fn watch_response(encoded: EncodedPayload, version: u64) -> WatchResponse {
    WatchResponse {
        data: encoded.data,
        encoding: encoded.encoding,
        uncompressed_size: encoded.uncompressed_size,
        sync_version: version,
        server_id: "payload-test".into(),
        err: String::new(),
    }
}

async fn receive<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("test event timed out")
        .expect("test channel closed")
}

#[tokio::test]
async fn compressed_list_watch_and_corrupt_watch_relist_publish_through_gateway_cache() {
    let (list_tx, mut list_rx) = mpsc::unbounded_channel();
    let (watch_tx, mut watch_rx) = mpsc::unbounded_channel();
    let release_list = Arc::new(Notify::new());
    let peer = Peer {
        snapshots: Mutex::new(VecDeque::from([
            snapshot("initial", 1).await,
            snapshot("recovered", 3).await,
        ])),
        list_started: list_tx,
        release_list: release_list.clone(),
        watch_started: watch_tx,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = futures::stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(stream, _)| stream), listener))
    });
    let serving = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ConfigSyncServer::new(peer).max_encoding_message_size(MAX_MESSAGE_BYTES))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let cache = ClientCache::<ConfigMap>::new("payload-test".into(), "payload-test".into());
    let (publication_tx, mut publications) = mpsc::unbounded_channel();
    cache.set_conf_processor(Box::new(PublicationProbe(publication_tx)));
    cache
        .set_grpc_client(ConfigSyncClient::new(channel).max_decoding_message_size(MAX_MESSAGE_BYTES))
        .await;
    cache.start_watch().await.unwrap();
    receive(&mut list_rx).await;
    assert!(!cache.is_ready());
    assert!(cache.is_empty());
    release_list.notify_one();
    assert_eq!(receive(&mut publications).await, "initial");
    let watch = receive(&mut watch_rx).await;
    assert!(cache.is_ready());

    let encoded = payload::encode(Bytes::from(
        serde_json::to_vec(&serde_json::json!([{"type":"update", "sync_version":2, "data":config("updated")}]))
            .unwrap(),
    ))
    .await
    .unwrap();
    assert_eq!(encoded.encoding, PayloadEncoding::Zstd as i32);
    watch.send(Ok(watch_response(encoded, 2))).await.unwrap();
    assert_eq!(receive(&mut publications).await, "updated");
    assert!(cache.is_ready());
    let current = cache.get("test/config").unwrap();
    assert_eq!(current.data.as_ref().unwrap()["padding"], "x".repeat(33 * 1024 * 1024));
    drop(current);

    watch
        .send(Ok(watch_response(
            EncodedPayload {
                data: Bytes::from_static(b"invalid-zstd-frame"),
                encoding: PayloadEncoding::Zstd as i32,
                uncompressed_size: 100,
            },
            3,
        )))
        .await
        .unwrap();
    // A malformed compressed update ends Watch. Hold the recovery RPC to inspect old state.
    receive(&mut list_rx).await;
    assert!(
        cache.is_ready(),
        "generic readiness means existing configuration is available"
    );
    assert_eq!(cache.get("test/config").unwrap().data.unwrap()["stage"], "updated");
    assert!(publications.try_recv().is_err(), "malformed bytes must not publish");
    release_list.notify_one();
    assert_eq!(receive(&mut publications).await, "recovered");
    let _resumed_watch = receive(&mut watch_rx).await;
    assert!(cache.is_ready());
    assert_eq!(cache.get("test/config").unwrap().data.unwrap()["stage"], "recovered");
    serving.abort();
    // start_watch is process-scoped; dropping this test runtime stops its background loop.
}
