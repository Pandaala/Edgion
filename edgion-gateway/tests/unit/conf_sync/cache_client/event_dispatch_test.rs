//! Unit tests for the `CacheEventDispatch` impl on `ClientCache`.
//!
//! Covers cache mutations, generic Watch diagnostics and wire decoding, and
//! Backend withdrawal/publication with an in-process handler. The full
//! watch/relist/backoff loop in `start_watch` needs integration coverage.
use super::*;
use crate::ai::AiBackendStore;
use edgion_common::conf_sync::proto::{ListRequest, ListResponse, WatchResponse};
use edgion_common::conf_sync::traits::{CacheEventDispatch, ConfHandler, ResourceChange};
use edgion_resources::resources::{BackendTLSPolicy, EdgionBackend, Gateway, GatewayClass};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::core::ObjectMeta;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
struct LogBuffer(Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_watch_diagnostics(run: impl FnOnce()) -> (String, String) {
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || tracing::subscriber::with_default(subscriber, run));
    let logs = String::from_utf8(buffer.0.lock().clone()).unwrap();
    (logs, recorder.handle().render())
}

#[test]
fn generic_watch_stops_at_first_failure_and_keeps_valid_prefix() {
    let cache = new_client_cache();
    cache.set_ready();
    let mut malformed = serde_json::to_value(make_cm("ns1", "bad")).unwrap();
    malformed["metadata"]["resourceVersion"] = serde_json::json!("123");
    // Serde's Display text includes this string when decoding bool fails.
    malformed["immutable"] = serde_json::json!("private-password-sentinel");
    let batch = serde_json::json!([
        {"type": "add", "data": make_cm("ns1", "before")},
        {"type": "update", "sync_version": 41, "data": malformed},
        {"type": "private-envelope-sentinel", "data": {}},
        {"type": "add", "data": make_cm("ns1", "after")}
    ]);
    let (logs, metrics) = capture_watch_diagnostics(|| {
        assert!(apply_generic_watch_payload(&cache.cache_data, &batch.to_string(), 42).is_err());
    });
    assert_eq!(cached_keys(&cache), ["ns1/before"]);
    assert!(cache.is_ready());
    assert_eq!(logs.lines().count(), 1, "{logs}");
    for field in [
        "kind=\"ConfigMap\"",
        "ns=\"ns1\"",
        "name=\"bad\"",
        "resourceVersion=\"123\"",
        "syncVersion=41",
    ] {
        assert!(logs.contains(field), "{logs}");
    }
    assert!(logs.contains("watch resource does not match the resource schema"));
    assert!(!logs.contains("sentinel"));
    assert!(
        metrics.contains("edgion_config_event_parse_errors_total{kind=\"ConfigMap\"} 1"),
        "{metrics}"
    );
}

#[test]
fn generic_watch_envelope_and_batch_failures_are_observable_without_payloads() {
    let cache = new_client_cache();
    let cases = [
        (
            "not-json-private-sentinel",
            "watch response must be a valid JSON event array",
        ),
        ("[null]", "watch event must be an object"),
        (r#"[{"type":"private-sentinel","data":{}}]"#, "unknown watch event type"),
        (r#"[{"data":{}}]"#, "watch event type must be a string"),
        (r#"[{"type":"add"}]"#, "watch event data is missing"),
    ];
    for (payload, expected) in cases {
        let (logs, metrics) = capture_watch_diagnostics(|| {
            assert!(apply_generic_watch_payload(&cache.cache_data, payload, 42).is_err());
        });
        assert_eq!(logs.lines().count(), 1, "{logs}");
        assert!(logs.contains(expected) && logs.contains("syncVersion=42"), "{logs}");
        assert!(!logs.contains("sentinel"));
        assert!(
            metrics.contains("edgion_config_event_parse_errors_total{kind=\"ConfigMap\"} 1"),
            "{metrics}"
        );
    }
}

#[test]
fn generic_watch_does_not_log_invalid_or_unbounded_metadata() {
    let cache = new_client_cache();
    let batch = serde_json::json!([{"type": "add", "data": {
        "metadata": {
            "namespace": "private-sentinel\nforged-log",
            "name": "private-sentinel".repeat(64),
            "resourceVersion": {"password": "private-sentinel"}
        },
        "immutable": "private-sentinel"
    }}]);
    let (logs, _) = capture_watch_diagnostics(|| {
        assert!(apply_generic_watch_payload(&cache.cache_data, &batch.to_string(), 42).is_err());
    });
    assert_eq!(logs.lines().count(), 1, "{logs}");
    assert!(!logs.contains("sentinel") && !logs.contains("forged-log"));
    assert!(logs.len() < 512);
}

#[test]
fn generic_watch_round_trips_controller_events_and_resolved_tls_secret() {
    use edgion_common::conf_sync::types::{EventType, WatcherEvent};
    use edgion_resources::resources::EdgionTls;
    use k8s_openapi::{api::core::v1::Secret, ByteString};

    let mut tls: EdgionTls = serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "ns1", "name": "tls", "resourceVersion": "123"},
        "spec": {"hosts": ["example.com"], "secretRef": {"name": "cert"}}
    }))
    .unwrap();
    tls.spec.secret = Some(Secret {
        data: Some(std::collections::BTreeMap::from([(
            "tls.key".to_string(),
            ByteString(b"private-test-key".to_vec()),
        )])),
        ..Default::default()
    });
    let cache = Arc::new(RwLock::new(CacheData::<EdgionTls>::new()));
    // Match Controller watch_json: serialize the shared typed event array.
    for (event_type, sv, present) in [
        (EventType::Add, 41, true),
        (EventType::Update, 42, true),
        (EventType::Delete, 43, false),
    ] {
        let payload = serde_json::to_string(&vec![WatcherEvent {
            event_type,
            sync_version: sv,
            data: tls.clone(),
        }])
        .unwrap();
        let (logs, metrics) = capture_watch_diagnostics(|| apply_generic_watch_payload(&cache, &payload, 99).unwrap());
        assert!(logs.is_empty() && !metrics.contains("edgion_config_event_parse_errors_total"));
        let data = cache.read();
        assert_eq!(data.get("ns1/tls").is_some(), present);
        if let Some(stored) = data.get("ns1/tls") {
            assert_eq!(stored.metadata.resource_version.as_deref(), Some("123"));
            assert_eq!(
                stored.spec.secret.as_ref().unwrap().data.as_ref().unwrap()["tls.key"].0,
                b"private-test-key"
            );
            assert_eq!(stored.get_sync_version(), sv);
        }
    }
}

#[derive(Default)]
struct RecordingMapHandler {
    data: parking_lot::Mutex<HashMap<String, Arc<ConfigMap>>>,
    publications: parking_lot::Mutex<Vec<&'static str>>,
}

impl ConfHandler<ConfigMap> for RecordingMapHandler {
    fn full_set(&self, data: &HashMap<String, Arc<ConfigMap>>) {
        *self.data.lock() = data.clone();
        self.publications.lock().push("full");
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<ConfigMap>>,
        update: HashMap<String, Arc<ConfigMap>>,
        remove: HashSet<String>,
    ) {
        let mut data = self.data.lock();
        for key in remove {
            data.remove(&key);
        }
        data.extend(add);
        data.extend(update);
        self.publications.lock().push("partial");
    }
}

#[test]
fn generic_relist_preserves_readiness_and_supersedes_pending_events() {
    // Exercise both possible callback orders around reset: an already
    // published valid prefix, and a prefix still waiting in the queue.
    for flush_before_reset in [false, true] {
        let cache = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
        let handler = Arc::new(RecordingMapHandler::default());
        cache.write().set_conf_processor_for_test(Box::new(handler.clone()));
        let initial = serde_json::to_vec(&vec![make_cm("ns1", "old")]).unwrap();
        publish_list_payload(&cache, &initial, 10).unwrap();
        let mut ready = false;
        mark_ready_after_list(&cache, &mut ready);

        let batch = serde_json::json!([
            {"type": "delete", "data": make_cm("ns1", "old")},
            {"type": "unknown", "data": {}},
            {"type": "add", "data": make_cm("ns1", "tail")}
        ]);
        assert!(apply_generic_watch_payload(&cache, &batch.to_string(), 11).is_err());
        assert!(cache.read().is_empty());
        assert_eq!(cache.read().compressed_event_count_for_test(), 1);
        assert_eq!(cache.read().sync_version(), 10);
        assert!(cache.read().is_ready());
        assert!(handler.data.lock().contains_key("ns1/old"));

        // A failed replacement must not clear old runtime configuration or
        // publish partially decoded List data, and its error must be safe.
        let bad_list = serde_json::json!([
            make_cm("ns1", "must-not-publish"),
            {"metadata": {"name": "bad"}, "immutable": "private-list-password-sentinel"}
        ]);
        let error = publish_list_payload(&cache, &serde_json::to_vec(&bad_list).unwrap(), 12).unwrap_err();
        assert_eq!(error, "list payload does not match the resource array schema");
        assert_eq!(handler.data.lock().len(), 1);
        assert!(handler.data.lock().contains_key("ns1/old"));
        assert_eq!(cache.read().sync_version(), 10);
        assert!(cache.read().is_ready());
        if flush_before_reset {
            CacheData::flush_compressed_events_for_test(&cache);
            assert!(handler.data.lock().is_empty());
        }

        let replacement = serde_json::to_vec(&vec![make_cm("ns1", "tail")]).unwrap();
        publish_list_payload(&cache, &replacement, 20).unwrap();
        mark_ready_after_list(&cache, &mut ready);
        let publications = handler.publications.lock().len();
        assert_eq!(cache.read().compressed_event_count_for_test(), 0);
        CacheData::flush_compressed_events_for_test(&cache);
        assert_eq!(handler.publications.lock().len(), publications);
        assert_eq!(handler.data.lock().len(), 1);
        assert!(handler.data.lock().contains_key("ns1/tail"));
        assert!(cache.read().is_ready());
        assert_eq!(cache.read().sync_version(), 20);

        let resumed = serde_json::json!([{"type": "delete", "data": make_cm("ns1", "tail")}]);
        apply_generic_watch_payload(&cache, &resumed.to_string(), 21).unwrap();
        CacheData::flush_compressed_events_for_test(&cache);
        assert!(handler.data.lock().is_empty());
        assert_eq!(handler.publications.lock().last(), Some(&"partial"));
    }
}

struct RecordingBackendHandler {
    store: Arc<AiBackendStore>,
    full_set_sizes: parking_lot::Mutex<Vec<usize>>,
    partial_removes: parking_lot::Mutex<Vec<HashSet<String>>>,
}

impl RecordingBackendHandler {
    fn new(store: Arc<AiBackendStore>) -> Self {
        Self {
            store,
            full_set_sizes: parking_lot::Mutex::new(Vec::new()),
            partial_removes: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl ConfHandler<EdgionBackend> for RecordingBackendHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionBackend>>) {
        self.full_set_sizes.lock().push(data.len());
        self.store.full_set(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionBackend>>,
        update: HashMap<String, Arc<EdgionBackend>>,
        remove: HashSet<String>,
    ) {
        self.partial_removes.lock().push(remove.clone());
        self.store.partial_update(add, update, remove);
    }
}

fn backend_cache_with_handler() -> (
    Arc<RwLock<CacheData<EdgionBackend>>>,
    Arc<AiBackendStore>,
    Arc<RecordingBackendHandler>,
) {
    let cache = Arc::new(RwLock::new(CacheData::new()));
    let store = Arc::new(AiBackendStore::new());
    let handler = Arc::new(RecordingBackendHandler::new(Arc::clone(&store)));
    cache
        .write()
        .set_conf_processor_for_test(Box::new(Arc::clone(&handler)));
    (cache, store, handler)
}

fn make_cm(namespace: &str, name: &str) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn new_client_cache() -> ClientCache<ConfigMap> {
    ClientCache::<ConfigMap>::new("client-id".to_string(), "client-name".to_string())
}

fn cached_keys(cache: &ClientCache<ConfigMap>) -> Vec<String> {
    let mut keys = cache.keys();
    keys.sort();
    keys
}

#[test]
fn init_add_inserts_resource_into_cache() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
    assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
}

#[test]
fn event_add_inserts_resource_into_cache() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::EventAdd, make_cm("ns1", "a"), None);
    let stored = cache.get("ns1/a").expect("present");
    assert_eq!(stored.metadata.name.as_deref(), Some("a"));
}

#[test]
fn event_update_overwrites_existing_resource() {
    let cache = new_client_cache();
    let mut original = make_cm("ns1", "a");
    original.metadata.resource_version = Some("1".to_string());
    cache.apply_change(ResourceChange::EventAdd, original, None);

    let mut updated = make_cm("ns1", "a");
    updated.metadata.resource_version = Some("2".to_string());
    cache.apply_change(ResourceChange::EventUpdate, updated, None);

    assert_eq!(cache.len(), 1);
    let stored = cache.get("ns1/a").expect("present");
    assert_eq!(stored.metadata.resource_version.as_deref(), Some("2"));
}

#[test]
fn event_delete_removes_resource_from_cache() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
    assert_eq!(cache.len(), 1);
    cache.apply_change(ResourceChange::EventDelete, make_cm("ns1", "a"), None);
    assert!(cache.is_empty());
}

#[test]
fn init_start_signal_does_not_mutate_data() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
    // InitStart is a signal, not a data event — must not change stored data.
    cache.apply_change(ResourceChange::InitStart, make_cm("ns2", "b"), None);
    assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
}

#[test]
fn init_done_signal_does_not_mutate_data() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
    // InitDone is a signal — same expectation as InitStart.
    cache.apply_change(ResourceChange::InitDone, make_cm("ns2", "b"), None);
    assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
}

#[test]
fn set_ready_marks_cache_ready() {
    let cache = new_client_cache();
    assert!(!cache.is_ready());
    CacheEventDispatch::set_ready(&cache);
    assert!(cache.is_ready());
}

#[test]
fn multiple_resources_with_distinct_keys_coexist() {
    let cache = new_client_cache();
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "b"), None);
    cache.apply_change(ResourceChange::InitAdd, make_cm("ns2", "a"), None);
    // ns1/a and ns2/a share a name but different namespaces — both must
    // survive because key_name() includes the namespace prefix.
    assert_eq!(
        cached_keys(&cache),
        vec!["ns1/a".to_string(), "ns1/b".to_string(), "ns2/a".to_string()]
    );
}

#[test]
fn generation_cancel_relists_immediately_at_initial_backoff() {
    // Spec §7.2 item 5: a generation-cancel (cert reload / rotation) must
    // relist immediately (no sleep) — otherwise each kind sleeps at its
    // current backoff (up to 32s) after a failover, blowing the resync
    // budget. A real failure keeps sleeping at the current backoff.
    assert_eq!(super::resync_wait_secs(true, 32), None, "reload: skip the sleep");
    assert_eq!(super::resync_wait_secs(false, 8), Some(8), "failure: sleep the backoff");
}

#[test]
fn backend_watch_rejects_invalid_batch_json() {
    assert!(serde_json::from_str::<Vec<serde_json::Value>>("not-json").is_err());
}

#[test]
fn backend_watch_rejects_non_object_event() {
    assert!(take_watch_event_parts(serde_json::json!("add"), 1).is_err());
}

#[test]
fn backend_watch_rejects_missing_type() {
    assert!(take_watch_event_parts(serde_json::json!({"data": {}}), 1).is_err());
}

#[test]
fn backend_watch_rejects_non_string_type() {
    assert!(take_watch_event_parts(serde_json::json!({"type": 1, "data": {}}), 1).is_err());
}

#[test]
fn backend_watch_rejects_unknown_type() {
    assert!(take_watch_event_parts(serde_json::json!({"type": "replace", "data": {}}), 1).is_err());
}

#[test]
fn backend_watch_rejects_missing_data() {
    assert!(take_watch_event_parts(serde_json::json!({"type": "add"}), 1).is_err());
}

#[test]
fn backend_watch_accepts_recognized_event_envelopes() {
    for (event_type, expected) in [
        ("add", ResourceChange::EventAdd),
        ("update", ResourceChange::EventUpdate),
        ("delete", ResourceChange::EventDelete),
    ] {
        let event = serde_json::json!({"type": event_type, "data": {}});
        assert_eq!(take_watch_event_parts(event, 1).unwrap().0, expected);
    }
}

#[test]
fn watch_event_data_is_moved_without_deep_clone() {
    let payload = "x".repeat(4096);
    let event = serde_json::json!({
        "type": "update",
        "sync_version": 9,
        "data": {"payload": payload}
    });
    let original_pointer = event["data"]["payload"]
        .as_str()
        .expect("test payload must be a string")
        .as_ptr();

    let (change, sync_version, data) = take_watch_event_parts(event, 3).unwrap();
    assert_eq!(change, ResourceChange::EventUpdate);
    assert_eq!(sync_version, 9);
    assert_eq!(
        data["payload"]
            .as_str()
            .expect("moved payload must remain a string")
            .as_ptr(),
        original_pointer
    );
}

#[test]
fn backend_watch_uses_only_valid_exact_key_for_malformed_resource_withdrawal() {
    assert_eq!(
        raw_backend_key(&serde_json::json!({"metadata": {"namespace": "ns-1", "name": "backend.example"}})),
        Some("ns-1/backend.example".to_string())
    );
    for unsafe_identity in [
        serde_json::json!({}),
        serde_json::json!({"metadata": {"name": "backend"}}),
        serde_json::json!({"metadata": {"namespace": "", "name": "backend"}}),
        serde_json::json!({"metadata": {"namespace": "ns", "name": "../backend"}}),
    ] {
        assert_eq!(raw_backend_key(&unsafe_identity), None);
    }
}

fn valid_backend_json() -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackend",
        "metadata": {"namespace": "ns-1", "name": "backend.example"},
        "spec": {
            "ai": {
                "provider": "OpenAI",
                "credentialPool": {
                    "credentials": [{"name": "primary", "secretRef": {"name": "key"}}]
                },
                "models": [{"name": "model-a"}]
            }
        }
    })
}

fn runtime_backend_json(name: &str) -> serde_json::Value {
    let mut backend = valid_backend_json();
    backend["metadata"]["name"] = serde_json::json!(name);
    backend["spec"]["ai"]["credentialPool"]["credentials"][0]["secret"] = serde_json::json!({
        "data": {"api-key": "c2VjcmV0"},
        "type": "Opaque"
    });
    backend
}

#[test]
fn backend_list_validates_identity_before_typed_publication() {
    let valid = serde_json::to_vec(&vec![valid_backend_json()]).unwrap();
    let decoded = decode_list_payload::<EdgionBackend>(&valid).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].key_name(), "ns-1/backend.example");

    for metadata in [
        serde_json::json!({"name": "backend"}),
        serde_json::json!({"namespace": "ns", "name": ""}),
        serde_json::json!({"namespace": "Bad_Namespace", "name": "backend"}),
        serde_json::json!({"namespace": "ns", "name": "../backend"}),
    ] {
        let mut unsafe_backend = valid_backend_json();
        unsafe_backend["metadata"] = metadata;
        let payload = serde_json::to_vec(&vec![unsafe_backend]).unwrap();
        assert!(decode_list_payload::<EdgionBackend>(&payload).is_err());
    }
}

#[test]
fn ordinary_resource_list_keeps_existing_identity_semantics() {
    let payload = serde_json::to_vec(&vec![serde_json::json!({"metadata": {"name": "cm"}})]).unwrap();
    assert_eq!(decode_list_payload::<ConfigMap>(&payload).unwrap().len(), 1);
}

#[test]
fn gateway_api_list_rejects_noncanonical_type_meta_before_typed_parse() {
    let valid = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
        "metadata": {"name": "edge"},
        "spec": {"controllerName": "example.com/gateway"}
    });
    assert_eq!(
        decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![valid.clone()]).unwrap())
            .unwrap()
            .len(),
        1
    );
    for (field, replacement) in [
        ("apiVersion", serde_json::json!("gateway.networking.k8s.io/v1beta1")),
        ("apiVersion", serde_json::json!(1)),
        ("kind", serde_json::json!("Gateway")),
    ] {
        let mut wrong = valid.clone();
        wrong[field] = replacement;
        assert!(decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![wrong]).unwrap()).is_err());
    }
    for field in ["apiVersion", "kind"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![missing]).unwrap()).is_err());
    }
}

#[test]
fn gateway_api_watch_rejects_beta_and_recovers_on_canonical_event() {
    let cache = Arc::new(RwLock::new(CacheData::<GatewayClass>::new()));
    let mut resource = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
        "metadata": {"name": "edge"},
        "spec": {"controllerName": "example.com/gateway"}
    });
    let event = |data| serde_json::to_string(&vec![serde_json::json!({"type": "add", "data": data})]).unwrap();
    resource["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1beta1");
    assert!(apply_generic_watch_payload(&cache, &event(resource.clone()), 1).is_err());
    assert!(cache.read().is_empty());
    resource["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1");
    assert!(apply_generic_watch_payload(&cache, &event(resource), 2).is_ok());
    assert_eq!(cache.read().len(), 1);
}

#[test]
fn gateway_list_retains_sibling_when_address_value_is_absent_and_recovers() {
    let cache = Arc::new(RwLock::new(CacheData::<Gateway>::new()));
    let gateway = |name: &str, address: serde_json::Value| {
        serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": name, "namespace": "demo"},
            "spec": {"gatewayClassName": "edge", "addresses": [address],
                "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
        })
    };
    let valid = gateway("valid", serde_json::json!({"type": "IPAddress", "value": "192.0.2.1"}));
    let absent = gateway("absent", serde_json::json!({"type": "IPAddress"}));
    let mixed = serde_json::to_vec(&vec![valid.clone(), absent]).unwrap();
    assert_eq!(publish_list_payload(&cache, &mixed, 1).unwrap(), 2);
    assert_eq!(cache.read().len(), 2);
    assert!(cache.read().get("demo/valid").is_some());
    assert_eq!(
        cache
            .read()
            .get("demo/absent")
            .unwrap()
            .spec
            .addresses
            .as_ref()
            .unwrap()[0]
            .value,
        None
    );
    assert!(cache.read().get("demo/absent").unwrap().spec.listeners.is_none());

    let repaired = gateway("absent", serde_json::json!({"type": "IPAddress", "value": "192.0.2.2"}));
    let list = serde_json::to_vec(&vec![valid, repaired]).unwrap();
    assert_eq!(publish_list_payload(&cache, &list, 2).unwrap(), 2);
    assert_eq!(
        cache
            .read()
            .get("demo/absent")
            .unwrap()
            .spec
            .addresses
            .as_ref()
            .unwrap()[0]
            .value
            .as_deref(),
        Some("192.0.2.2")
    );
    assert!(cache.read().get("demo/absent").unwrap().spec.listeners.is_some());
}

#[test]
fn backend_tls_list_retains_sibling_with_unsupported_ca_name_and_recovers() {
    let cache = Arc::new(RwLock::new(CacheData::<BackendTLSPolicy>::new()));
    let policy = |name: &str, ca: &str| {
        serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "BackendTLSPolicy",
            "metadata": {"name": name, "namespace": "demo"},
            "spec": {"targetRefs": [{"group": "", "kind": "Service", "name": "backend"}],
                "validation": {"hostname": "backend.example.com", "wellKnownCACertificates": ca}}
        })
    };
    let valid = policy("valid", "System");
    let unsupported = policy("unsupported", "example.com/Custom");
    let mixed = serde_json::to_vec(&vec![valid.clone(), unsupported]).unwrap();
    assert_eq!(publish_list_payload(&cache, &mixed, 1).unwrap(), 2);
    assert_eq!(cache.read().len(), 2);
    assert!(cache.read().get("demo/valid").is_some());
    assert!(matches!(
        cache
            .read()
            .get("demo/unsupported")
            .unwrap()
            .spec
            .validation
            .well_known_ca_certificates,
        Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::Other(_))
    ));
    let repaired = policy("unsupported", "System");
    let list = serde_json::to_vec(&vec![valid, repaired]).unwrap();
    assert_eq!(publish_list_payload(&cache, &list, 2).unwrap(), 2);
    assert!(matches!(
        cache
            .read()
            .get("demo/unsupported")
            .unwrap()
            .spec
            .validation
            .well_known_ca_certificates,
        Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System)
    ));
}

#[test]
fn kind_23_conf_sync_envelopes_require_nested_ai_spec() {
    let request = ListRequest {
        key: String::new(),
        kind: EdgionBackend::kind(&()).to_string(),
        expected_server_id: "server-a".to_string(),
    };
    assert_eq!(request.kind, "EdgionBackend");
    assert_eq!(edgion_resources::ResourceKind::EdgionBackend as i32, 23);

    let nested = valid_backend_json();
    let nested_response = ListResponse {
        data: serde_json::to_vec(&vec![nested.clone()]).unwrap().into(),
        sync_version: 7,
        server_id: "server-a".to_string(),
        ..Default::default()
    };
    assert_eq!(
        decode_list_payload::<EdgionBackend>(&nested_response.data)
            .unwrap()
            .len(),
        1
    );
    let mut flat = nested.clone();
    flat["spec"] = flat["spec"]["ai"].take();
    let flat_response = ListResponse {
        data: serde_json::to_vec(&vec![flat.clone()]).unwrap().into(),
        sync_version: 8,
        server_id: "server-a".to_string(),
        ..Default::default()
    };
    assert!(decode_list_payload::<EdgionBackend>(&flat_response.data).is_err());

    let nested_watch = WatchResponse {
        data: serde_json::to_vec(&vec![serde_json::json!({"type": "update", "data": nested})])
            .unwrap()
            .into(),
        sync_version: 9,
        err: String::new(),
        server_id: "server-a".to_string(),
        ..Default::default()
    };
    let nested_events: Vec<serde_json::Value> = serde_json::from_slice(&nested_watch.data).unwrap();
    let (_, _, nested_data) = take_watch_event_parts(nested_events.into_iter().next().unwrap(), 9).unwrap();
    assert!(serde_json::from_value::<EdgionBackend>(nested_data).is_ok());

    let flat_watch = WatchResponse {
        data: serde_json::to_vec(&vec![serde_json::json!({"type": "update", "data": flat})])
            .unwrap()
            .into(),
        sync_version: 10,
        err: String::new(),
        server_id: "server-a".to_string(),
        ..Default::default()
    };
    let flat_events: Vec<serde_json::Value> = serde_json::from_slice(&flat_watch.data).unwrap();
    let (_, _, flat_data) = take_watch_event_parts(flat_events.into_iter().next().unwrap(), 10).unwrap();
    assert!(raw_backend_key(&flat_data).is_some());
    assert!(serde_json::from_value::<EdgionBackend>(flat_data).is_err());
}

#[test]
fn backend_fail_close_clears_cache_version_and_readiness() {
    let cache = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
    {
        let mut data = cache.write();
        data.insert("ns/a".to_string(), make_cm("ns", "a"));
        data.set_ready();
    }
    clear_backend_cache(&cache, 42);
    let data = cache.read();
    assert!(data.is_empty());
    assert!(!data.is_ready());
    assert_eq!(data.sync_version(), 42);
}

#[tokio::test]
async fn invalid_transport_preserves_generic_state_and_withdraws_backend_state() {
    let generic = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
    generic.write().insert("ns/a".into(), make_cm("ns", "a"));
    generic.write().set_ready();
    let generation = CancellationToken::new();
    assert!(
        decode_transport_payload(&generic, Bytes::from_static(b"[]"), 0, 3, 2, &generation)
            .await
            .is_err()
    );
    assert_eq!(generic.read().len(), 1);
    assert!(generic.read().is_ready());

    let (backend, store, _) = backend_cache_with_handler();
    let initial = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
    publish_list_payload(&backend, &initial, 1).unwrap();
    backend.write().set_ready();
    assert!(
        decode_transport_payload(&backend, Bytes::from_static(b"broken"), 1, 100, 2, &generation)
            .await
            .is_err()
    );
    assert!(backend.read().is_empty());
    assert!(!backend.read().is_ready());
    assert_eq!(store.count(), 0);
    publish_list_payload(&backend, &initial, 3).unwrap();
    assert_eq!(store.count(), 1, "a valid relist restores the runtime");
}

#[tokio::test]
async fn cancelled_generation_cannot_clear_backend_during_payload_decode() {
    use std::{future::Future, task::Poll};
    let (backend, store, _) = backend_cache_with_handler();
    let initial = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
    publish_list_payload(&backend, &initial, 1).unwrap();
    backend.write().set_ready();
    let encoded = payload::encode(vec![b' '; payload::COMPRESSION_THRESHOLD + 1].into())
        .await
        .unwrap();
    let generation = CancellationToken::new();
    // An incorrect declared size would withdraw backend state if the old response
    // were allowed to finish after the channel generation was retired.
    let future = decode_transport_payload(
        &backend,
        encoded.data,
        encoded.encoding,
        encoded.uncompressed_size - 1,
        2,
        &generation,
    );
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending(), "blocking decode must yield");
        Poll::Ready(())
    })
    .await;
    generation.cancel();
    assert_eq!(future.await.unwrap_err().code(), tonic::Code::Cancelled);
    assert_eq!(backend.read().sync_version(), 1);
    assert!(backend.read().is_ready());
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn cancelled_generation_discards_successful_compressed_payload() {
    use std::{future::Future, task::Poll};
    let cache = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
    cache.write().insert("ns/old".into(), make_cm("ns", "old"));
    cache.write().set_ready();
    let json = format!("[{}]", " ".repeat(payload::COMPRESSION_THRESHOLD));
    let encoded = payload::encode(json.into()).await.unwrap();
    let generation = CancellationToken::new();
    let future = decode_transport_payload(
        &cache,
        encoded.data,
        encoded.encoding,
        encoded.uncompressed_size,
        2,
        &generation,
    );
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending(), "blocking decode must yield");
        Poll::Ready(())
    })
    .await;
    generation.cancel();
    let result = future.await;
    assert_eq!(result.as_ref().unwrap_err().code(), tonic::Code::Cancelled);
    if let Ok(data) = result {
        publish_list_payload(&cache, &data, 2).unwrap();
    }
    assert!(cache.read().get("ns/old").is_some());
    assert!(cache.read().is_ready());
}

#[test]
fn backend_list_failure_withdraws_runtime_and_valid_retry_restores_readiness() {
    let (cache, store, handler) = backend_cache_with_handler();
    let valid_payload = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
    let mut loop_ready = false;

    assert_eq!(publish_list_payload(&cache, &valid_payload, 10).unwrap(), 1);
    mark_ready_after_list(&cache, &mut loop_ready);
    assert!(cache.read().is_ready());
    assert_eq!(store.keys(), vec!["ns-1/backend-a"]);

    cache
        .write()
        .add_compress_event("ns-1/backend-a".to_string(), ResourceChange::EventUpdate);
    assert_eq!(cache.read().compressed_event_count_for_test(), 1);

    assert!(publish_list_payload(&cache, b"not-json", 11).is_err());
    loop_ready = false;
    assert!(cache.read().is_empty());
    assert!(!cache.read().is_ready());
    assert_eq!(cache.read().sync_version(), 11);
    assert_eq!(cache.read().compressed_event_count_for_test(), 0);
    assert_eq!(store.count(), 0, "empty full_set must withdraw the runtime snapshot");
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0]);

    assert_eq!(publish_list_payload(&cache, &valid_payload, 12).unwrap(), 1);
    mark_ready_after_list(&cache, &mut loop_ready);
    assert!(loop_ready);
    assert!(cache.read().is_ready());
    assert_eq!(cache.read().sync_version(), 12);
    assert_eq!(store.keys(), vec!["ns-1/backend-a"]);
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1]);
}

#[test]
fn backend_watch_safe_typed_failure_withdraws_exact_key_only() {
    let (cache, store, handler) = backend_cache_with_handler();
    let initial = serde_json::to_vec(&vec![
        runtime_backend_json("backend-a"),
        runtime_backend_json("backend-b"),
    ])
    .unwrap();
    publish_list_payload(&cache, &initial, 20).unwrap();
    let mut loop_ready = false;
    mark_ready_after_list(&cache, &mut loop_ready);

    let mut malformed = runtime_backend_json("backend-a");
    malformed["spec"] = malformed["spec"]["ai"].take();
    let payload = serde_json::to_string(&vec![serde_json::json!({
        "type": "update",
        "sync_version": 21,
        "data": malformed
    })])
    .unwrap();

    assert_eq!(
        apply_backend_watch_payload(&cache, &payload, 21),
        BackendWatchBatchOutcome::Applied
    );
    assert!(
        cache.read().is_ready(),
        "a safe exact-key withdrawal does not poison the stream"
    );
    assert!(cache.read().get("ns-1/backend-a").is_none());
    assert!(cache.read().get("ns-1/backend-b").is_some());
    assert_eq!(cache.read().compressed_event_count_for_test(), 1);

    assert!(CacheData::flush_compressed_events_for_test(&cache));
    assert_eq!(store.keys(), vec!["ns-1/backend-b"]);
    assert_eq!(cache.read().compressed_event_count_for_test(), 0);
    assert_eq!(
        &*handler.partial_removes.lock(),
        &[HashSet::from(["ns-1/backend-a".to_string()])]
    );
}

#[test]
fn backend_watch_unsafe_batch_aborts_later_events_and_relist_restores_runtime() {
    let (cache, store, handler) = backend_cache_with_handler();
    let initial = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
    publish_list_payload(&cache, &initial, 30).unwrap();
    let mut loop_ready = false;
    mark_ready_after_list(&cache, &mut loop_ready);

    cache
        .write()
        .add_compress_event("ns-1/backend-a".to_string(), ResourceChange::EventUpdate);
    let batch = serde_json::to_string(&vec![
        serde_json::json!({"type": "delete", "data": runtime_backend_json("backend-a")}),
        serde_json::json!({"type": "replace", "data": {}}),
        serde_json::json!({"type": "add", "data": runtime_backend_json("must-not-apply")}),
    ])
    .unwrap();

    assert_eq!(
        apply_backend_watch_payload(&cache, &batch, 31),
        BackendWatchBatchOutcome::Unsafe
    );
    loop_ready = false;
    assert!(cache.read().is_empty());
    assert!(!cache.read().is_ready());
    assert_eq!(cache.read().sync_version(), 31);
    assert_eq!(cache.read().compressed_event_count_for_test(), 0);
    assert_eq!(store.count(), 0);
    assert!(store.get_by_key("ns-1/must-not-apply").is_none());
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0]);

    let restored = serde_json::to_vec(&vec![runtime_backend_json("backend-restored")]).unwrap();
    publish_list_payload(&cache, &restored, 32).unwrap();
    mark_ready_after_list(&cache, &mut loop_ready);
    assert!(loop_ready);
    assert!(cache.read().is_ready());
    assert_eq!(store.keys(), vec!["ns-1/backend-restored"]);
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1]);

    assert_eq!(
        apply_backend_watch_payload(&cache, "not-json", 33),
        BackendWatchBatchOutcome::Unsafe
    );
    loop_ready = false;
    assert!(cache.read().is_empty());
    assert!(!cache.read().is_ready());
    assert_eq!(store.count(), 0);
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1, 0]);

    publish_list_payload(&cache, &restored, 34).unwrap();
    mark_ready_after_list(&cache, &mut loop_ready);
    assert!(loop_ready);
    assert!(cache.read().is_ready());
    assert_eq!(store.keys(), vec!["ns-1/backend-restored"]);
    assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1, 0, 1]);
}
