//! Unit tests for `CacheData<T>` data-structure operations and `CompressEvent`
//! event accumulation. These tests intentionally avoid the
//! `set_conf_processor` path (which spawns a tokio background task and is
//! deferred to integration coverage).
use super::*;
use edgion_common::conf_sync::traits::ResourceChange;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::core::ObjectMeta;

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

#[test]
fn new_creates_empty_unready_cache() {
    let cache: CacheData<ConfigMap> = CacheData::new();
    assert!(!cache.is_ready());
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.sync_version(), 0);
    assert!(cache.handler.is_none());
}

#[test]
fn insert_and_get_round_trip() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    let cm = make_cm("ns1", "cm-a");
    cache.insert("ns1/cm-a".to_string(), cm);
    assert_eq!(cache.len(), 1);
    let got = cache.get("ns1/cm-a").expect("present");
    assert_eq!(got.metadata.name.as_deref(), Some("cm-a"));
    assert_eq!(got.metadata.namespace.as_deref(), Some("ns1"));
}

#[test]
fn insert_duplicate_key_replaces_value() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    let mut cm1 = make_cm("ns1", "cm-a");
    cm1.metadata.resource_version = Some("1".to_string());
    cache.insert("ns1/cm-a".to_string(), cm1);

    let mut cm2 = make_cm("ns1", "cm-a");
    cm2.metadata.resource_version = Some("2".to_string());
    cache.insert("ns1/cm-a".to_string(), cm2);

    // Still a single entry, but with the newer resource_version.
    assert_eq!(cache.len(), 1);
    let got = cache.get("ns1/cm-a").expect("present");
    assert_eq!(got.metadata.resource_version.as_deref(), Some("2"));
}

#[test]
fn remove_existing_returns_some_and_drops_entry() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.insert("ns1/cm-a".to_string(), make_cm("ns1", "cm-a"));
    let removed = cache.remove("ns1/cm-a");
    assert!(removed.is_some());
    assert!(cache.is_empty());
    assert!(cache.get("ns1/cm-a").is_none());
}

#[test]
fn remove_missing_returns_none() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    assert!(cache.remove("ns1/missing").is_none());
}

#[test]
fn get_missing_returns_none() {
    let cache: CacheData<ConfigMap> = CacheData::new();
    assert!(cache.get("ns1/missing").is_none());
}

#[test]
fn reset_replaces_all_entries_and_updates_sync_version() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.insert("ns1/old".to_string(), make_cm("ns1", "old"));
    cache.set_sync_version(7);

    let new_set = vec![make_cm("ns1", "a"), make_cm("ns2", "b"), make_cm("ns2", "c")];
    cache.reset(new_set, 42);

    assert_eq!(cache.len(), 3);
    assert!(cache.get("ns1/old").is_none(), "old key must be evicted");
    assert!(cache.get("ns1/a").is_some());
    assert!(cache.get("ns2/b").is_some());
    assert!(cache.get("ns2/c").is_some());
    assert_eq!(cache.sync_version(), 42);
}

#[test]
fn reset_with_empty_list_clears_everything() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
    cache.insert("ns1/b".to_string(), make_cm("ns1", "b"));
    cache.reset(Vec::new(), 100);
    assert!(cache.is_empty());
    assert_eq!(cache.sync_version(), 100);
}

#[test]
fn ready_state_toggle() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    assert!(!cache.is_ready());
    cache.set_ready();
    assert!(cache.is_ready());
    // set_ready is idempotent — second call must not flip back to false.
    cache.set_ready();
    assert!(cache.is_ready());
}

#[test]
fn sync_version_round_trip() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    assert_eq!(cache.sync_version(), 0);
    cache.set_sync_version(5);
    assert_eq!(cache.sync_version(), 5);
    cache.set_sync_version(0);
    assert_eq!(cache.sync_version(), 0);
}

#[test]
fn keys_and_values_iterate_inserted_entries() {
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
    cache.insert("ns1/b".to_string(), make_cm("ns1", "b"));

    let mut keys: Vec<String> = cache.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["ns1/a".to_string(), "ns1/b".to_string()]);

    let value_count = cache.values().count();
    assert_eq!(value_count, 2);
}

#[test]
fn arc_get_returns_strong_reference_into_storage() {
    // Inserted resources are wrapped in `Arc<T>` for zero-copy sharing.
    // `get` must hand out a clone of that Arc, so the strong count grows
    // while the returned reference is alive.
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
    let arc_in_cache = cache.get("ns1/a").expect("present").clone();
    // One ref is held by the cache, one by `arc_in_cache`.
    assert_eq!(Arc::strong_count(&arc_in_cache), 2);
    // Removing the entry drops the cache's ref; `arc_in_cache` keeps the value alive.
    let _ = cache.remove("ns1/a");
    assert_eq!(Arc::strong_count(&arc_in_cache), 1);
}

#[test]
fn add_compress_event_without_handler_is_no_op() {
    // Without a registered processor, compression events are dropped on
    // the floor. The expected behavior — see `set_conf_processor` doc.
    let mut cache: CacheData<ConfigMap> = CacheData::new();
    cache.add_compress_event("ns1/a".to_string(), ResourceChange::EventAdd);
    // Nothing observable should change.
    assert!(cache.is_empty());
    assert!(cache.handler.is_none());
}

#[test]
fn compress_event_accumulates_per_key() {
    let mut events = CompressEvent::new();
    events.add_event("ns1/a".to_string(), ResourceChange::EventAdd);
    events.add_event("ns1/a".to_string(), ResourceChange::EventUpdate);
    events.add_event("ns1/b".to_string(), ResourceChange::EventDelete);

    assert_eq!(events.events.len(), 2);
    let series_a = events.events.get("ns1/a").expect("a present");
    assert_eq!(series_a.len(), 2);
    assert_eq!(series_a[0], ResourceChange::EventAdd);
    assert_eq!(series_a[1], ResourceChange::EventUpdate);
    let series_b = events.events.get("ns1/b").expect("b present");
    assert_eq!(series_b.len(), 1);
    assert_eq!(series_b[0], ResourceChange::EventDelete);
}

#[test]
fn compress_event_clear_empties_state() {
    let mut events = CompressEvent::new();
    events.add_event("k".to_string(), ResourceChange::EventAdd);
    assert!(!events.events.is_empty());
    events.clear();
    assert!(events.events.is_empty());
}

#[test]
fn compressed_events_publish_final_values_without_reclassifying_adds() {
    #[derive(Default)]
    struct Recorder(parking_lot::Mutex<Vec<(&'static str, Option<String>)>>);
    impl ConfHandler<ConfigMap> for Recorder {
        fn full_set(&self, data: &HashMap<String, Arc<ConfigMap>>) {
            self.0.lock().push((
                "full",
                data.get("ns1/a")
                    .and_then(|value| value.metadata.resource_version.clone()),
            ));
        }
        fn partial_update(
            &self,
            add: HashMap<String, Arc<ConfigMap>>,
            update: HashMap<String, Arc<ConfigMap>>,
            remove: HashSet<String>,
        ) {
            assert_eq!(add.len() + update.len() + remove.len(), 1);
            let (kind, value) = if !add.is_empty() {
                ("add", add.get("ns1/a"))
            } else if !update.is_empty() {
                ("update", update.get("ns1/a"))
            } else {
                assert!(remove.contains("ns1/a"));
                ("remove", None)
            };
            self.0
                .lock()
                .push((kind, value.and_then(|v| v.metadata.resource_version.clone())));
        }
    }
    let recorder = Arc::new(Recorder::default());
    let cache = Arc::new(RwLock::new(CacheData::new()));
    cache.write().set_conf_processor_for_test(Box::new(recorder.clone()));
    let enqueue = |change, version: &str| {
        let mut value = make_cm("ns1", "a");
        value.metadata.resource_version = Some(version.to_owned());
        let mut data = cache.write();
        if matches!(change, ResourceChange::EventDelete) {
            data.remove("ns1/a");
        } else {
            data.insert("ns1/a".to_owned(), value);
        }
        data.add_compress_event("ns1/a".to_owned(), change);
    };
    let flush = || assert!(CacheData::flush_compressed_events_for_test(&cache));
    let expect = |kind, version: Option<&str>| {
        assert_eq!(*recorder.0.lock(), vec![(kind, version.map(str::to_owned))]);
        recorder.0.lock().clear();
    };

    enqueue(ResourceChange::EventAdd, "1");
    enqueue(ResourceChange::EventAdd, "2");
    flush();
    expect("add", Some("2"));
    enqueue(ResourceChange::EventAdd, "3");
    flush();
    expect("add", Some("3"));
    enqueue(ResourceChange::EventDelete, "3");
    enqueue(ResourceChange::EventAdd, "4");
    flush();
    expect("add", Some("4"));
    enqueue(ResourceChange::EventUpdate, "5");
    flush();
    expect("update", Some("5"));
    enqueue(ResourceChange::EventDelete, "5");
    flush();
    expect("remove", None);

    enqueue(ResourceChange::EventAdd, "6");
    let mut replacement = make_cm("ns1", "a");
    replacement.metadata.resource_version = Some("7".to_owned());
    cache.write().reset(vec![replacement], 7);
    expect("full", Some("7"));
    flush();
    assert!(recorder.0.lock().is_empty());
    assert_eq!(
        cache.read().get("ns1/a").unwrap().metadata.resource_version.as_deref(),
        Some("7")
    );
    cache.write().reset(Vec::new(), 8);
    expect("full", None);
    assert!(cache.read().is_empty());
}

#[test]
fn compressed_replacement_add_reconciles_backend_traffic_policy_ownership() {
    use crate::backends::degrade_threshold::{configure_lb_degrade_default, resolve_degrade_threshold};
    use crate::backends::health::check::get_hc_config_store;
    use crate::backends::outlier_detection::get_outlier_detection_store;
    use crate::backends::policy::backend_traffic::store::{BackendTrafficPolicyStore, GLOBAL_STORE_TEST_LOCK};
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
    use edgion_resources::resources::lb_policy::ParsedLBPolicy;
    use edgion_resources::EdgionBackendTrafficPolicy;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    configure_lb_degrade_default(0);
    let make_policy = |target: &str, algorithm: &str, threshold: u8| {
        serde_json::from_value::<EdgionBackendTrafficPolicy>(serde_json::json!({
            "metadata": {"namespace": "compressed-ebtp", "name": "policy"},
            "spec": {
                "targetRefs": [{"kind": "Service", "name": target}],
                "resolvedTargetClass": "service",
                "preparedPrecedence": {},
                "loadBalancer": {"type": algorithm, "degradeThreshold": threshold},
                "healthCheck": {"active": {"type": "http"}},
                "outlierDetection": {"consecutiveErrors": 3, "ejectionTime": "30s", "maxEjectionPercent": 50}
            }
        }))
        .expect("valid policy fixture")
    };
    let target = |name: &str| PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "compressed-ebtp".into(),
        name: name.into(),
    };
    let store = Arc::new(BackendTrafficPolicyStore::new());
    let cache = Arc::new(RwLock::new(CacheData::new()));
    cache.write().set_conf_processor_for_test(Box::new(store.clone()));
    cache.write().reset(vec![make_policy("a", "LeastConn", 40)], 1);
    let old = store.snapshot();
    assert!(get_hc_config_store().resolve_policy("compressed-ebtp/a").is_some());
    assert!(get_outlier_detection_store()
        .keys()
        .contains(&"compressed-ebtp/a".into()));
    {
        let mut data = cache.write();
        data.remove("compressed-ebtp/policy");
        data.add_compress_event("compressed-ebtp/policy".into(), ResourceChange::EventDelete);
        data.insert("compressed-ebtp/policy".into(), make_policy("b", "Ewma", 60));
        data.add_compress_event("compressed-ebtp/policy".into(), ResourceChange::EventAdd);
    }
    assert!(CacheData::flush_compressed_events_for_test(&cache));
    assert_eq!(store.snapshot().generation(), old.generation() + 1);
    assert!(store.snapshot().candidates(&target("a")).is_empty());
    assert!(store.get_effective(&target("a")).is_none());
    assert!(get_hc_config_store().resolve_policy("compressed-ebtp/a").is_none());
    assert!(!get_outlier_detection_store()
        .keys()
        .contains(&"compressed-ebtp/a".into()));
    assert_eq!(resolve_degrade_threshold("compressed-ebtp/a"), 0);
    assert_eq!(store.get_lb_policy("compressed-ebtp/b"), Some(ParsedLBPolicy::Ewma));
    assert_eq!(resolve_degrade_threshold("compressed-ebtp/b"), 60);
    assert!(old.get_effective(&target("a")).is_some());

    // Add to an already published key must also replace the same-target value.
    cache
        .write()
        .insert("compressed-ebtp/policy".into(), make_policy("b", "LeastConn", 20));
    cache
        .write()
        .add_compress_event("compressed-ebtp/policy".into(), ResourceChange::EventAdd);
    assert!(CacheData::flush_compressed_events_for_test(&cache));
    assert_eq!(store.snapshot().candidates(&target("b")).len(), 1);
    assert_eq!(
        store.get_lb_policy("compressed-ebtp/b"),
        Some(ParsedLBPolicy::LeastConn)
    );
    assert_eq!(resolve_degrade_threshold("compressed-ebtp/b"), 20);
    let current = store.snapshot();
    let raw = current.candidates(&target("b"))[0].clone();
    let effective = current.get_effective(&target("b")).unwrap();
    let hc = get_hc_config_store().resolve_policy("compressed-ebtp/b").unwrap();
    cache
        .write()
        .insert("compressed-ebtp/policy".into(), make_policy("b", "LeastConn", 20));
    cache
        .write()
        .add_compress_event("compressed-ebtp/policy".into(), ResourceChange::EventAdd);
    assert!(CacheData::flush_compressed_events_for_test(&cache));
    assert_eq!(store.snapshot().generation(), current.generation());
    assert!(Arc::ptr_eq(&raw, &store.snapshot().candidates(&target("b"))[0]));
    assert!(Arc::ptr_eq(&effective, &store.get_effective(&target("b")).unwrap()));
    assert!(Arc::ptr_eq(
        &hc,
        &get_hc_config_store().resolve_policy("compressed-ebtp/b").unwrap()
    ));

    // Compare the same final state via full_set, including shared background stores.
    let final_policy = make_policy("b", "LeastConn", 20);
    cache.write().reset(Vec::new(), 2);
    let full = BackendTrafficPolicyStore::new();
    full.full_set_policies(HashMap::from([(
        "compressed-ebtp/policy".into(),
        Arc::new(final_policy.clone()),
    )]));
    assert!(full.snapshot().candidates(&target("a")).is_empty());
    assert_eq!(full.get_lb_policy("compressed-ebtp/b"), effective.load_balancer);
    assert_eq!(resolve_degrade_threshold("compressed-ebtp/b"), 20);
    assert!(get_hc_config_store().resolve_policy("compressed-ebtp/b").is_some());
    assert!(get_outlier_detection_store()
        .keys()
        .contains(&"compressed-ebtp/b".into()));
    full.full_set_policies(HashMap::new());

    cache.write().reset(vec![final_policy.clone()], 3);
    let mut invalid = final_policy;
    invalid.spec.resolved_target_class = None;
    cache.write().insert("compressed-ebtp/policy".into(), invalid);
    cache
        .write()
        .add_compress_event("compressed-ebtp/policy".into(), ResourceChange::EventAdd);
    assert!(CacheData::flush_compressed_events_for_test(&cache));
    assert!(store.snapshot().candidates(&target("b")).is_empty());
    assert!(store.get_effective(&target("b")).is_none());
    assert!(get_hc_config_store().resolve_policy("compressed-ebtp/b").is_none());
    assert!(!get_outlier_detection_store()
        .keys()
        .contains(&"compressed-ebtp/b".into()));
    assert_eq!(resolve_degrade_threshold("compressed-ebtp/b"), 0);
    cache.write().reset(Vec::new(), 4);
}
