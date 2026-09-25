//! Exercise production snapshot caching and Watch encoding over a real gRPC connection.

use std::{collections::HashMap, sync::Arc, time::Duration};

use edgion_common::conf_sync::{
    payload::{self, COMPRESSION_THRESHOLD, MAX_MESSAGE_BYTES},
    proto::{config_sync_client::ConfigSyncClient, ListRequest, PayloadEncoding, WatchRequest},
    traits::{CacheEventDispatch, ResourceChange},
};
use edgion_controller::conf_sync::{
    cache_server::ServerCache,
    conf_server::{ConfigSyncGrpcServer, ConfigSyncServer, ConfigSyncServerProvider, WatchObj},
};
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, WafBundleMember, WafRuleBundleData,
};
use edgion_resources::resources::edgion_plugins::WafConfig;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const KIND: &str = "EdgionConfigData";
const MIB: usize = 1024 * 1024;

struct Provider(Arc<ConfigSyncServer>);

impl ConfigSyncServerProvider for Provider {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        Some(self.0.clone())
    }
}

fn large_bundle() -> WafRuleBundleData {
    // Valid rule text, with quotes and newlines to exercise JSON escaping too.
    let mut content = "# transport padding \"quoted\"\n".repeat(5 * MIB / 29);
    content.push_str("\nSecRule REQUEST_URI \"@contains blocked\" \"id:1001,phase:1,deny\"\n");
    let report = WafConfig::inspect_inline_source(&content);
    assert!(
        report.strict_validation_passed,
        "fixture must compile: {:?}",
        report.failure
    );
    let bundle = WafRuleBundleData {
        version: "transport-test-v1".into(),
        profile: "strict".into(),
        provenance: "local/transport-test".into(),
        digest: None,
        roots: vec!["rules/main.conf".into()],
        rules: vec![WafBundleMember {
            name: "rules/main.conf".into(),
            content,
        }],
        phrase_assets: Vec::new(),
    };
    bundle.validate().expect("fixture must satisfy WAF bundle limits");
    bundle
}

fn resource(index: usize, bundle: &WafRuleBundleData) -> EdgionConfigData {
    let mut value = EdgionConfigData::new(
        &format!("bundle-{index}"),
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            data: ConfigEntry::WafRuleBundle(bundle.clone()),
            unknown_fields: Vec::new(),
        },
    );
    value.metadata.namespace = Some("transport-test".into());
    value
}

fn assert_bundle(value: &EdgionConfigData, expected: &WafRuleBundleData) {
    let ConfigEntry::WafRuleBundle(actual) = &value.spec.data else {
        panic!("expected WAF rule bundle");
    };
    assert_eq!(actual.rules[0].content, expected.rules[0].content);
    assert_eq!(actual.roots, expected.roots);
}

fn list_request(server_id: &str) -> ListRequest {
    ListRequest {
        key: String::new(),
        kind: KIND.into(),
        expected_server_id: server_id.into(),
    }
}

#[derive(Deserialize)]
struct ReceivedEvent {
    #[serde(rename = "type")]
    event_type: String,
    sync_version: u64,
    data: EdgionConfigData,
}

#[tokio::test]
async fn real_grpc_transports_large_waf_snapshots_and_watch_batches() {
    // One sequential test keeps the large payload fixtures from running concurrently.
    let bundle = large_bundle();
    let cache = Arc::new(ServerCache::<EdgionConfigData>::new(32));
    cache.apply_change(ResourceChange::EventAdd, resource(0, &bundle), Some(1));
    WatchObj::set_ready(cache.as_ref());
    let server = Arc::new(ConfigSyncServer::new());
    server.register_all(HashMap::from([(KIND.into(), cache.clone() as Arc<dyn WatchObj>)]));
    let server_id = server.server_id();
    let stop = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = futures::stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(stream, _)| stream), listener))
    });
    let service = ConfigSyncGrpcServer::new(Arc::new(Provider(server.clone())), stop.clone()).into_service();
    let shutdown = stop.clone();
    let serving = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming_shutdown(incoming, shutdown.cancelled())
            .await
            .unwrap();
    });
    let endpoint = format!("http://{address}");
    let channel = tonic::transport::Endpoint::from_shared(endpoint.clone())
        .unwrap()
        .timeout(Duration::from_secs(60))
        .connect()
        .await
        .unwrap();
    let mut client = ConfigSyncClient::new(channel).max_decoding_message_size(MAX_MESSAGE_BYTES);

    // A valid single WAF resource exceeds tonic's old 4 MiB default but stays uncompressed.
    let response = client.list(list_request(&server_id)).await.unwrap().into_inner();
    assert_eq!(response.encoding, PayloadEncoding::Identity as i32);
    assert!(response.data.len() > 4 * MIB);
    assert!(response.data.len() < COMPRESSION_THRESHOLD);
    assert_eq!(response.sync_version, 1);
    assert_eq!(response.server_id, server_id);
    let decoded = payload::decode(response.data, response.encoding, response.uncompressed_size)
        .await
        .unwrap();
    let initial: Vec<EdgionConfigData> = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(initial.len(), 1);
    assert_bundle(&initial[0], &bundle);
    drop((initial, decoded));

    // Queue a batch before opening Watch so the production ring buffer sends one large batch.
    for index in 1..13 {
        cache.apply_change(
            ResourceChange::EventAdd,
            resource(index, &bundle),
            Some(index as u64 + 1),
        );
    }
    let mut watch = client
        .watch(WatchRequest {
            key: String::new(),
            kind: KIND.into(),
            client_id: "large-payload-test".into(),
            client_name: "large-payload-test".into(),
            from_version: 1,
            expected_server_id: server_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    let response = tokio::time::timeout(Duration::from_secs(60), watch.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(response.err.is_empty());
    assert_eq!(response.encoding, PayloadEncoding::Zstd as i32);
    assert!(response.uncompressed_size > COMPRESSION_THRESHOLD as u64);
    assert!(response.data.len() < response.uncompressed_size as usize);
    assert_eq!(response.sync_version, 13);
    assert_eq!(response.server_id, server_id);
    let decoded = payload::decode(response.data, response.encoding, response.uncompressed_size)
        .await
        .unwrap();
    let events: Vec<ReceivedEvent> = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(events.len(), 12);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.event_type, "add");
        assert_eq!(event.sync_version, index as u64 + 2);
        assert_eq!(
            event.data.metadata.name.as_deref(),
            Some(format!("bundle-{}", index + 1).as_str())
        );
        assert_bundle(&event.data, &bundle);
    }
    drop((watch, events, decoded));

    // Aggregate snapshots beyond the removed 64 MiB business cap still round-trip intact.
    let response = client.list(list_request(&server_id)).await.unwrap().into_inner();
    assert_eq!(response.encoding, PayloadEncoding::Zstd as i32);
    assert!(response.uncompressed_size > 64 * MIB as u64);
    let decoded = payload::decode(response.data, response.encoding, response.uncompressed_size)
        .await
        .unwrap();
    let resources: Vec<EdgionConfigData> = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(resources.len(), 13);
    for value in &resources {
        assert_bundle(value, &bundle);
    }
    drop((resources, decoded));

    let (first, version, _) = server.list_grpc_cached(KIND).await.unwrap();
    let (second, next_version, _) = server.list_grpc_cached(KIND).await.unwrap();
    assert_eq!(version, 13);
    assert_eq!(next_version, version);
    assert_eq!(first.encoding, PayloadEncoding::Zstd as i32);
    assert_eq!(
        first.data.as_ptr(),
        second.data.as_ptr(),
        "reuse the compressed allocation"
    );

    drop(client);
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .unwrap()
        .unwrap();
}
