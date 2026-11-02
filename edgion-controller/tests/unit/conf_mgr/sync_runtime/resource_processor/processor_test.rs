use super::*;
use kube::api::ObjectMeta;
use serde::{Deserialize, Serialize};

struct TestHandler;

struct EligibilityHandler;
#[async_trait::async_trait]
impl ProcessorHandler<TestResource> for EligibilityHandler {
    fn filter(&self, obj: &TestResource, _ctx: &HandlerContext) -> bool {
        obj.spec != "excluded"
    }
}

#[tokio::test]
async fn local_raw_replacement_losing_eligibility_unpublishes_without_removing_raw() {
    use super::super::raw_store::LocalRawStore;
    for namespace_filter in [false, true] {
        let processor = ResourceProcessor::new(
            "TestResource",
            10,
            Arc::new(EligibilityHandler),
            Arc::new(ControllerRuntime::new()),
        );
        let raw = Arc::new(LocalRawStore::new());
        processor.set_raw_store(raw.clone());
        let mut obj = TestResource {
            metadata: ObjectMeta {
                name: Some("same-key".into()),
                namespace: Some("a".into()),
                uid: Some("first".into()),
                ..Default::default()
            },
            spec: "accepted".into(),
            status: None,
        };
        raw.upsert("a/same-key".into(), Arc::new(obj.clone()));
        processor.on_init_apply(obj.clone(), None).await;
        processor.on_init_done();
        obj.metadata.uid = Some("second".into());
        if namespace_filter {
            processor.set_namespace_filter(vec!["b".into()]);
        } else {
            obj.spec = "excluded".into();
        }
        raw.upsert("a/same-key".into(), Arc::new(obj.clone()));
        let result = processor
            .process_work_item("a/same-key", Some(obj), Some("{}".into()), TriggerChain::default())
            .await;
        assert!(matches!(result, WorkItemResult::Deleted { .. }));
        assert!(processor.get("a/same-key").is_none());
        assert_eq!(
            processor
                .get_raw_any("a/same-key")
                .unwrap()
                .downcast::<TestResource>()
                .unwrap()
                .metadata
                .uid
                .as_deref(),
            Some("second")
        );
    }
}

#[async_trait::async_trait]
impl<K> ProcessorHandler<K> for TestHandler where K: kube::Resource + Clone + Send + Sync + 'static {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestResource {
    metadata: ObjectMeta,
    spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<serde_json::Value>,
}

impl ResourceMeta for TestResource {
    fn get_version(&self) -> u64 {
        0
    }
    fn get_sync_version(&self) -> u64 {
        0
    }
    fn resource_kind() -> edgion_resources::ResourceKind {
        edgion_resources::ResourceKind::Unspecified
    }
    fn kind_name() -> &'static str {
        "TestResource"
    }
    fn key_name(&self) -> String {
        make_resource_key(self)
    }
}

impl kube::Resource for TestResource {
    type DynamicType = ();
    type Scope = kube::core::ClusterResourceScope;

    fn kind(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "TestResource".into()
    }
    fn group(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "test.example.com".into()
    }
    fn version(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "v1".into()
    }
    fn plural(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
        "testresources".into()
    }
    fn meta(&self) -> &ObjectMeta {
        &self.metadata
    }
    fn meta_mut(&mut self) -> &mut ObjectMeta {
        &mut self.metadata
    }
}

struct PublishOrderHandler {
    processor: parking_lot::Mutex<Option<std::sync::Weak<ResourceProcessor<TestResource>>>>,
    observed_final_cache: std::sync::atomic::AtomicBool,
    observed_unpublished_cache: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl ProcessorHandler<TestResource> for PublishOrderHandler {
    fn update_status(&self, obj: &mut TestResource, _ctx: &HandlerContext, _validation_errors: &[String]) {
        obj.spec = "status-derived".to_string();
    }

    fn finalize_transport(&self, obj: &mut TestResource, _ctx: &HandlerContext) {
        assert_eq!(obj.spec, "status-derived");
        obj.spec = "final-transport".to_string();
    }

    async fn after_publish(&self, obj: &TestResource, _ctx: &HandlerContext) {
        let processor = self
            .processor
            .lock()
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .expect("test processor must still be alive");
        let cached = processor
            .get(&obj.key_name())
            .expect("after_publish must observe cache");
        self.observed_final_cache.store(
            cached.spec == "final-transport" && cached.spec == obj.spec,
            std::sync::atomic::Ordering::Release,
        );
    }

    async fn after_unpublish(&self, obj: &TestResource, _ctx: &HandlerContext) {
        let processor = self
            .processor
            .lock()
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .expect("test processor must still be alive");
        self.observed_unpublished_cache.store(
            processor.get(&obj.key_name()).is_none(),
            std::sync::atomic::Ordering::Release,
        );
    }
}

#[tokio::test]
async fn final_transport_is_published_before_post_publish_consumers_run() {
    let handler = Arc::new(PublishOrderHandler {
        processor: parking_lot::Mutex::new(None),
        observed_final_cache: std::sync::atomic::AtomicBool::new(false),
        observed_unpublished_cache: std::sync::atomic::AtomicBool::new(false),
    });
    let processor = Arc::new(ResourceProcessor::<TestResource>::new(
        "TestResource",
        100,
        handler.clone(),
        Arc::new(ControllerRuntime::default()),
    ));
    *handler.processor.lock() = Some(Arc::downgrade(&processor));
    let resource = TestResource {
        metadata: ObjectMeta {
            name: Some("publish-order".to_string()),
            ..Default::default()
        },
        spec: "raw".to_string(),
        status: None,
    };

    let result = processor.on_init_apply(resource, None).await;
    let WorkItemResult::Processed { obj, .. } = result else {
        panic!("expected processed result")
    };
    assert_eq!(obj.spec, "final-transport");
    assert!(handler.observed_final_cache.load(std::sync::atomic::Ordering::Acquire));

    let result = processor
        .process_work_item(
            "publish-order",
            None,
            None,
            crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
        )
        .await;
    assert!(matches!(result, WorkItemResult::Deleted { .. }));
    assert!(handler
        .observed_unpublished_cache
        .load(std::sync::atomic::Ordering::Acquire));
}

#[tokio::test]
async fn test_processor_basic() {
    let handler = Arc::new(TestHandler);

    let processor =
        ResourceProcessor::<TestResource>::new("TestResource", 100, handler, Arc::new(ControllerRuntime::default()));

    assert_eq!(processor.kind(), "TestResource");
    assert!(!processor.is_ready());

    processor.set_ready();
    assert!(processor.is_ready());
}

/// `notify_ready()` must wake a `wait_kinds_ready` waiter promptly, without
/// relying on the ~1s fallback poll inside `wait_kinds_ready`.
///
/// The waiter is given a 5s timeout (far longer than the 1s fallback poll)
/// so a pass driven by the fallback poll would still take ~1s. We then wrap
/// the join in a 500ms timeout: if `notify_ready()` did nothing, the waiter
/// would only resolve via the fallback poll (~1s later) and this 500ms
/// timeout would fire, failing the test. Only an actual wake from
/// `notify_ready()` can resolve the waiter within the 500ms window.
#[tokio::test]
async fn notify_ready_wakes_wait_kinds_ready_promptly() {
    use std::time::Duration;

    let handler = Arc::new(TestHandler);
    let processor = Arc::new(ResourceProcessor::<TestResource>::new(
        "TestResource",
        100,
        handler,
        Arc::new(ControllerRuntime::default()),
    ));

    processor.runtime.processor_registry.register(processor.clone());

    let runtime = processor.runtime.clone();
    let kind = processor.kind;

    // Long timeout: if this waiter resolves quickly, it must be because
    // notify_ready() woke it, not the 1s fallback poll.
    let waiter = tokio::spawn(async move {
        runtime
            .processor_registry
            .wait_kinds_ready(&[kind], Duration::from_secs(5))
            .await
    });

    // Give the waiter a moment to register its `Notify::notified()` future.
    // `notify_waiters()` only wakes futures that are already registered, so
    // notifying before the waiter subscribes would be a silent no-op.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Mirror what `on_init_done()` does: mark the cache ready directly
    // (bypassing the `ProcessorObj::set_ready()` trait method, which
    // already calls `notify_ready()` itself), then call `notify_ready()`
    // separately so this test isolates its effect.
    WatchObj::set_ready(processor.cache.as_ref());
    processor.notify_ready();

    let resolved = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("waiter did not resolve within 500ms; notify_ready() failed to wake it promptly")
        .expect("waiter task panicked");

    assert!(resolved, "wait_kinds_ready should report the kind as ready");
}

#[tokio::test]
async fn test_processor_init_apply() {
    let handler = Arc::new(TestHandler);

    let processor =
        ResourceProcessor::<TestResource>::new("TestResource", 100, handler, Arc::new(ControllerRuntime::default()));

    let resource = TestResource {
        metadata: ObjectMeta {
            name: Some("test".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: "test spec".to_string(),
        status: None,
    };

    processor.on_init();
    assert!(!processor.is_ready());

    let result = processor.on_init_apply(resource.clone(), None).await;
    assert!(matches!(result, WorkItemResult::Processed { .. }));

    processor.on_init_done();
    assert!(processor.is_ready());

    // Check resource is in cache
    let cached = processor.get("default/test");
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().spec, "test spec");
    let snapshot = ProcessorObj::list_cached_json(&processor);
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0]["spec"], "test spec");
}

#[tokio::test]
async fn test_processor_init_apply_bumps_resource_version_floor() {
    use crate::conf_mgr::sync_runtime::resource_version::next_resource_version;

    let handler = Arc::new(TestHandler);
    let processor =
        ResourceProcessor::<TestResource>::new("TestResource", 100, handler, Arc::new(ControllerRuntime::default()));

    // Pick an rv that is guaranteed higher than the current process-wide floor.
    let baseline = next_resource_version();
    let high_rv = baseline + 1_000_000;

    let resource = TestResource {
        metadata: ObjectMeta {
            name: Some("test".to_string()),
            namespace: Some("default".to_string()),
            resource_version: Some(high_rv.to_string()),
            ..Default::default()
        },
        spec: "test spec".to_string(),
        status: None,
    };

    processor.on_init();
    let result = processor.on_init_apply(resource, None).await;
    assert!(matches!(result, WorkItemResult::Processed { .. }));

    let after = next_resource_version();
    assert!(
        after > high_rv,
        "on_init_apply should bump the floor above the resource's rv (after={}, high_rv={})",
        after,
        high_rv
    );
}

#[test]
fn test_gateway_class_status_change_detection() {
    use edgion_resources::prelude_resources::GatewayClass;
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway_class::GatewayClassStatus;

    let gc = GatewayClass {
        metadata: ObjectMeta {
            name: Some("test-gc".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
            current_status: None,
            controller_name: "example.com/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
        status: Some(GatewayClassStatus {
            conditions: vec![Condition {
                type_: "Accepted".to_string(),
                status: "Unknown".to_string(),
                reason: "Pending".to_string(),
                message: "Waiting for controller".to_string(),
                last_transition_time: "1970-01-01T00:00:00Z".to_string(),
                observed_generation: None,
            }],
            supported_features: None,
        }),
    };

    let old_status = extract_status_json(&gc);

    let mut gc_modified = gc.clone();
    let status = gc_modified.status.get_or_insert_with(GatewayClassStatus::default);
    crate::conf_mgr::sync_runtime::resource_processor::update_condition(
        &mut status.conditions,
        crate::conf_mgr::sync_runtime::resource_processor::accepted_condition(gc_modified.metadata.generation),
    );
    crate::conf_mgr::sync_runtime::resource_processor::update_condition(
        &mut status.conditions,
        crate::conf_mgr::sync_runtime::resource_processor::status_utils::condition_unknown(
            "SupportedVersion",
            "CompatibilityUnknown",
            "Installed Gateway API CRD bundle and channel compatibility has not been verified",
            gc_modified.metadata.generation,
        ),
    );

    let new_status = extract_status_json(&gc_modified);

    let changed = status_has_changed("GatewayClass", &old_status, &new_status, None);
    assert!(changed, "Status should have changed after update_status");
}

struct ParseStatusHandler;

#[async_trait::async_trait]
impl ProcessorHandler<TestResource> for ParseStatusHandler {
    async fn parse(&self, mut obj: TestResource, _ctx: &HandlerContext) -> ProcessResult<TestResource> {
        obj.status = Some(serde_json::json!({
            "conditions": [{
                "type": "ResolvedRefs",
                "status": "True",
                "reason": "ResolvedRefs",
                "message": "All references resolved",
                "observedGeneration": 1
            }]
        }));
        ProcessResult::Continue(obj)
    }
}

#[tokio::test]
async fn parse_status_change_is_compared_against_persisted_status() {
    let processor = ResourceProcessor::<TestResource>::new(
        "TestResource",
        100,
        Arc::new(ParseStatusHandler),
        Arc::new(ControllerRuntime::default()),
    );
    let resource = TestResource {
        metadata: ObjectMeta {
            name: Some("status-change".to_string()),
            ..Default::default()
        },
        spec: "test spec".to_string(),
        status: Some(serde_json::json!({
            "conditions": [{
                "type": "ResolvedRefs",
                "status": "False",
                "reason": "SecretNotFound",
                "message": "Secret not found",
                "observedGeneration": 1
            }]
        })),
    };

    let result = processor.on_init_apply(resource, None).await;
    match result {
        WorkItemResult::Processed {
            obj, status_changed, ..
        } => {
            assert!(status_changed, "parse-time status mutation must be persisted");
            assert_eq!(
                obj.status
                    .as_ref()
                    .and_then(|status| status.pointer("/conditions/0/status"))
                    .and_then(serde_json::Value::as_str),
                Some("True")
            );
        }
        other => panic!("expected processed result, got {other:?}"),
    }
}

#[test]
fn seed_existing_status_enables_ltt_preservation() {
    use edgion_resources::prelude_resources::GatewayClass;

    // FS/Etcd mode: the spec file carries no status.
    let mut gc = GatewayClass {
        metadata: ObjectMeta {
            name: Some("test-gc".to_string()),
            generation: Some(2),
            ..Default::default()
        },
        spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
            current_status: None,
            controller_name: "example.com/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
        status: None,
    };

    let sidecar = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"Resource accepted","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
    seed_existing_status(&mut gc, sidecar);

    let status = gc.status.as_mut().expect("status seeded from sidecar");
    assert_eq!(status.conditions.len(), 1);

    // A reconcile that does not flip the condition keeps the stored LTT
    // (while observedGeneration still advances).
    crate::conf_mgr::sync_runtime::resource_processor::update_condition(
        &mut status.conditions,
        crate::conf_mgr::sync_runtime::resource_processor::accepted_condition(gc.metadata.generation),
    );
    assert_eq!(status.conditions[0].last_transition_time, "2026-01-01T00:00:00Z");
    assert_eq!(status.conditions[0].observed_generation, Some(2));
}

#[test]
fn seed_existing_status_preserves_all_unknown_field_sidecars() {
    use edgion_resources::resources::edgion_config_data::{ConfigDataVisibility, ConfigEntry, EdgionConfigDataSpec};
    use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPluginsSpec;
    use edgion_resources::resources::link_sys::httpdns::{HttpDnsFallback, HttpDnsResponseFormat};
    use edgion_resources::resources::link_sys::{HttpDnsResolverConfig, LinkSysSpec, SystemConfig};
    use edgion_resources::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins, LinkSys};

    let expected = vec!["config.typo".to_string()];
    let status = r#"{"conditions":[]}"#;

    let mut plugins = EdgionPlugins::new("plugins", Default::default());
    plugins.spec.unknown_fields = expected.clone();
    seed_existing_status(&mut plugins, status);
    assert_eq!(plugins.spec.unknown_fields, expected);

    let mut stream_plugins = EdgionStreamPlugins::new(
        "stream-plugins",
        EdgionStreamPluginsSpec {
            current_status: None,
            plugins: None,
            tls_route_plugins: None,
            unknown_fields: expected.clone(),
        },
    );
    seed_existing_status(&mut stream_plugins, status);
    assert_eq!(stream_plugins.spec.unknown_fields, expected);

    let mut config_data = EdgionConfigData::new(
        "config-data",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: ConfigDataVisibility::Namespace,
            data: ConfigEntry::Misc(serde_json::json!({})),
            unknown_fields: expected.clone(),
        },
    );
    seed_existing_status(&mut config_data, status);
    assert_eq!(config_data.spec.unknown_fields, expected);

    let mut link_sys = LinkSys::new(
        "link-sys",
        LinkSysSpec {
            current_status: None,
            config: SystemConfig::HttpDns(HttpDnsResolverConfig {
                preset: None,
                url_template: Some("https://resolver.example/dns?name={domain}".to_string()),
                response: HttpDnsResponseFormat::default(),
                fallback: HttpDnsFallback::default(),
                connection: None,
            }),
            unknown_fields: expected.clone(),
        },
    );
    seed_existing_status(&mut link_sys, status);
    assert_eq!(link_sys.spec.unknown_fields, expected);
}

#[test]
fn seed_existing_status_skips_error_status_sidecar() {
    use edgion_resources::prelude_resources::GatewayClass;

    let mut gc = GatewayClass {
        metadata: ObjectMeta {
            name: Some("test-gc".to_string()),
            ..Default::default()
        },
        spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
            current_status: None,
            controller_name: "example.com/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
        status: None,
    };

    // A resource that was briefly unparseable has an ErrorStatus sidecar;
    // on recovery it must NOT be seeded, or its unmanaged condition type
    // would ghost through every subsequent upsert-only update_status.
    let sidecar = r#"{"conditions":[{"type":"Ready","status":"False","reason":"ParseError","message":"bad yaml","lastTransitionTime":"2026-01-01T00:00:00Z"}]}"#;
    seed_existing_status(&mut gc, sidecar);
    assert!(gc.status.is_none(), "ErrorStatus sidecar must not be seeded");
}

#[test]
fn seed_existing_status_ignores_unparseable_json() {
    use edgion_resources::prelude_resources::GatewayClass;

    let mut gc = GatewayClass {
        metadata: ObjectMeta {
            name: Some("test-gc".to_string()),
            ..Default::default()
        },
        spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
            current_status: None,
            controller_name: "example.com/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
        status: None,
    };

    seed_existing_status(&mut gc, "not-json{{");
    assert!(gc.status.is_none(), "object must be left unchanged on parse failure");
}

#[tokio::test]
async fn on_event_key_enqueues_for_worker() {
    let processor = ResourceProcessor::<TestResource>::new(
        "TestResource",
        100,
        Arc::new(TestHandler),
        Arc::new(ControllerRuntime::default()),
    );

    processor.on_event_key("obj1".to_string()).await;

    let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .expect("dequeue timed out")
        .expect("queue closed");
    assert_eq!(item.key, "obj1");
}

#[tokio::test]
async fn delete_flows_through_worker_arm() {
    let processor = ResourceProcessor::<TestResource>::new(
        "TestResource",
        100,
        Arc::new(TestHandler),
        Arc::new(ControllerRuntime::default()),
    );
    // Seed cache with the object, as a prior apply would have.
    let obj = TestResource {
        metadata: ObjectMeta {
            name: Some("obj1".to_string()),
            ..Default::default()
        },
        spec: "v1".to_string(),
        status: None,
    };
    processor.save(obj, None);
    assert!(processor.get("obj1").is_some());

    // Delete event: enqueue the key, then drive the worker path manually.
    processor.on_event_key("obj1".to_string()).await;
    let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .expect("dequeue timed out")
        .expect("queue closed");
    let result = processor
        .process_work_item(&item.key, None, None, item.trigger_chain.clone())
        .await;

    assert!(matches!(result, WorkItemResult::Deleted { .. }));
    assert!(processor.get("obj1").is_none());
}

mod policy_precedence_test {
    use super::*;
    use crate::conf_mgr::conf_center::common::parse_resource_content_offline;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::{
        BackendTlsPolicyHandler, EdgionBackendTrafficPolicyHandler,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceRef};
    use crate::conf_sync::conf_server::WatchObj;
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::{BackendTLSPolicy, EdgionBackendTrafficPolicy, ResourceKind};
    use k8s_openapi::api::core::v1::Service;
    use serde_json::{json, Value};
    use std::time::Duration;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-resources/tests/fixtures/policy_precedence_wire.json"
    ));

    fn runtime_with_backend() -> Arc<ControllerRuntime> {
        let runtime = Arc::new(ControllerRuntime::default());
        let service: Service = serde_json::from_value(
            json!({"metadata":{"name":"backend","namespace":"precision"},"spec":{"ports":[{"name":"https","port":443}]}}),
        )
        .unwrap();
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert("precision/backend".into(), Arc::new(service.clone()));
        let processor = Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            16,
            Arc::new(TestHandler),
            runtime.clone(),
        ));
        processor.set_raw_store(raw);
        processor.save(service, None);
        runtime.processor_registry.register(processor);
        runtime.service_ref_mgr.add_ref(
            "precision/backend".into(),
            ResourceRef::new(ResourceKind::HTTPRoute, Some("precision".into()), "route".into()),
        );
        runtime.gateway_route_index.update_route(
            ResourceKind::HTTPRoute,
            "precision/route",
            &[ParentReference {
                group: Some("gateway.networking.k8s.io".into()),
                kind: Some("Gateway".into()),
                namespace: Some("precision".into()),
                name: "gateway".into(),
                section_name: None,
                port: None,
            }],
            "precision",
        );
        runtime
    }

    // Remove only volatile transport metadata and native status. Every store-relevant
    // spec field is compared against the exact object consumed by Gateway tests.
    fn stable_wire(mut wire: Value) -> Value {
        wire.as_object_mut().unwrap().remove("status");
        wire["spec"].as_object_mut().unwrap().remove("currentStatus");
        let meta = wire["metadata"].as_object_mut().unwrap();
        meta.remove("resourceVersion");
        meta.remove("annotations");
        wire
    }

    fn accepted(status: &Value) -> &Value {
        let ancestors = status["ancestors"].as_array().unwrap();
        let ancestor = ancestors
            .iter()
            .find(|ancestor| ancestor["ancestorRef"]["name"] == "gateway")
            .expect("actual Gateway ancestor");
        ancestor["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|condition| condition["type"] == "Accepted")
            .unwrap()
    }

    macro_rules! policy_boundary_test {
        ($test:ident, $ty:ty, $handler:ty) => {
            #[tokio::test]
            async fn $test() {
                let fixtures: Vec<Value> = serde_json::from_str(FIXTURE).unwrap();
                let fixture = fixtures.iter().find(|value| value["kind"] == <$ty>::kind_name()).unwrap();
                for initial_status in [None, Some("not-json"), Some("{\"ancestors\":false}"), Some("{\"ancestors\":[]}"), Some("{\"conditions\":[{\"type\":\"Ready\",\"status\":\"False\",\"reason\":\"ParseError\",\"message\":\"invalid\"}]}")] {
                    let runtime = runtime_with_backend();
                    let processor = Arc::new(ResourceProcessor::<$ty>::new(<$ty>::kind_name(), 32, Arc::new(<$handler>::new("test.edgion.io/controller".into())), runtime.clone()));
                    runtime.processor_registry.register(processor.clone());
                    let sources = fixture["source"].as_array().unwrap();
                    for source in sources {
                        let mut forged = source.clone();
                        forged["spec"]["preparedPrecedence"] = json!({"creationTimestamp":"1900-01-01T00:00:00Z"});
                        let resource: $ty = parse_resource_content_offline(&forged.to_string()).unwrap();
                        processor.on_init_apply(resource, initial_status.map(str::to_owned)).await;
                    }
                    processor.on_init_done();
                    let queue = processor.workqueue();
                    // Parsing a queues the registered older sibling b. Exercise the
                    // real queue and original source reread, not a manual handler call.
                    let item = tokio::time::timeout(Duration::from_secs(3), queue.dequeue()).await.unwrap().unwrap();
                    assert_eq!(item.key, "precision/b");
                    let prior = processor.get(&item.key).unwrap();
                    let status = serde_json::to_string(&prior.status).unwrap();
                    let resource: $ty = parse_resource_content_offline(&sources[0].to_string()).unwrap();
                    processor.process_work_item(&item.key, Some(resource), Some(status), item.trigger_chain).await;
                    queue.done(&item.key);
                    for (index, name) in ["b", "a"].iter().enumerate() {
                        let cached = processor.get(&format!("precision/{name}")).unwrap();
                        assert_eq!(cached.metadata.creation_timestamp.as_ref().unwrap().0,
                            sources[index]["metadata"]["creationTimestamp"].as_str().unwrap().parse::<k8s_openapi::jiff::Timestamp>().unwrap());
                        assert_eq!(cached.spec.prepared_precedence.unwrap().creation_timestamp, cached.metadata.creation_timestamp.map(|time| time.0));
                        let status = serde_json::to_value(&cached.status).unwrap();
                        assert_eq!(accepted(&status)["reason"], if index == 0 { "Accepted" } else { "Conflicted" });
                    }
                    let cache = processor.cache();
                    let (list, version) = cache.list_json().unwrap();
                    let resources: Vec<Value> = serde_json::from_str(&list).unwrap();
                    for expected in fixture["wire"].as_array().unwrap() {
                        let actual = resources.iter().find(|resource| resource["metadata"]["name"] == expected["metadata"]["name"]).unwrap();
                        assert_eq!(stable_wire(actual.clone()), *expected);
                    }
                    // Normal no-op reconciliation with a valid persisted sidecar
                    // preserves the complete instant and condition transition time.
                    let before = processor.get("precision/b").unwrap();
                    let mut before_status = serde_json::to_value(&before.status).unwrap();
                    // A fixed old transition time makes this assertion distinguish
                    // preservation from recomputation within the same wall-clock second.
                    for ancestor in before_status["ancestors"].as_array_mut().unwrap() {
                        for condition in ancestor["conditions"].as_array_mut().unwrap() {
                            condition["lastTransitionTime"] = json!("2000-01-01T00:00:00Z");
                        }
                    }
                    let mut watch = cache.watch_json("precision-test".into(), "precision-test".into(), version);
                    processor.on_event_key("precision/b".into()).await;
                    let item = tokio::time::timeout(Duration::from_secs(3), queue.dequeue()).await.unwrap().unwrap();
                    assert_eq!(item.key, "precision/b");
                    processor.process_work_item(&item.key, Some(parse_resource_content_offline::<$ty>(&sources[0].to_string()).unwrap()), Some(before_status.to_string()), item.trigger_chain).await;
                    queue.done(&item.key);
                    let after = processor.get("precision/b").unwrap();
                    let after_status = serde_json::to_value(&after.status).unwrap();
                    assert!(accepted(&before_status)["lastTransitionTime"].as_str().is_some());
                    assert_eq!(accepted(&before_status)["lastTransitionTime"], accepted(&after_status)["lastTransitionTime"]);
                    let response = tokio::time::timeout(Duration::from_secs(3), watch.recv()).await.unwrap().unwrap();
                    assert!(response.err.is_none(), "{:?}", response.err);
                    let events: Vec<Value> = serde_json::from_str(&response.data).unwrap();
                    assert!(events.iter().any(|event| stable_wire(event["data"].clone()) == fixture["wire"][0]));
                    drop(watch);

                    let (_, version) = cache.list_json().unwrap();
                    let mut watch = cache.watch_json("precision-update".into(), "precision-update".into(), version);
                    processor.on_event_key("precision/b".into()).await;
                    let item = tokio::time::timeout(Duration::from_secs(3), queue.dequeue()).await.unwrap().unwrap();
                    processor.process_work_item(&item.key, Some(parse_resource_content_offline::<$ty>(&fixture["replacementSource"].to_string()).unwrap()), Some(after_status.to_string()), item.trigger_chain).await;
                    queue.done(&item.key);
                    let response = tokio::time::timeout(Duration::from_secs(3), watch.recv()).await.unwrap().unwrap();
                    assert!(response.err.is_none());
                    let events: Vec<Value> = serde_json::from_str(&response.data).unwrap();
                    assert!(events.iter().any(|event| stable_wire(event["data"].clone()) == fixture["replacementWire"]));
                    drop(watch);
                    // A fraction-only replacement must enqueue a and change its
                    // real ancestor status after serial processing.
                    let sibling = tokio::time::timeout(Duration::from_secs(3), queue.dequeue()).await.unwrap().unwrap();
                    assert_eq!(sibling.key, "precision/a");
                    let prior = processor.get(&sibling.key).unwrap();
                    processor.process_work_item(&sibling.key, Some(parse_resource_content_offline::<$ty>(&sources[1].to_string()).unwrap()), Some(serde_json::to_string(&prior.status).unwrap()), sibling.trigger_chain).await;
                    queue.done(&sibling.key);
                    assert_eq!(accepted(&serde_json::to_value(processor.get("precision/a").unwrap().status).unwrap())["reason"], "Accepted");
                    assert_eq!(accepted(&serde_json::to_value(processor.get("precision/b").unwrap().status).unwrap())["reason"], "Conflicted");
                    assert!(queue.is_empty());
                }
            }
        };
    }

    policy_boundary_test!(
        policy_precedence_tls_source_status_list_watch,
        BackendTLSPolicy,
        BackendTlsPolicyHandler
    );
    policy_boundary_test!(
        policy_precedence_traffic_source_status_list_watch,
        EdgionBackendTrafficPolicy,
        EdgionBackendTrafficPolicyHandler
    );

    #[tokio::test]
    async fn policy_precedence_canonical_ingress_domain_and_missing_source() {
        let fixtures: Vec<Value> = serde_json::from_str(FIXTURE).unwrap();
        macro_rules! check {
            ($ty:ty, $handler:ty) => {{
                let fixture = fixtures.iter().find(|value| value["kind"] == <$ty>::kind_name()).unwrap();
                let runtime = runtime_with_backend();
                let processor = ResourceProcessor::<$ty>::new(<$ty>::kind_name(), 32, Arc::new(<$handler>::new("test.edgion.io/controller".into())), runtime);
                let instants = [
                    Some("0000-01-01T00:00:00Z".parse().unwrap()),
                    Some(k8s_openapi::jiff::Timestamp::MAX),
                    Some("1969-12-31T23:59:59.999999999Z".parse().unwrap()),
                    Some("2026-01-01T01:00:00.123456789+01:00".parse().unwrap()),
                    None,
                ];
                for instant in instants {
                    let mut source = fixture["source"][0].clone();
                    source["metadata"]["creationTimestamp"] = serde_json::to_value(instant).unwrap();
                    source["spec"]["preparedPrecedence"] = json!({"creationTimestamp":"1900-01-01T00:00:00Z"});
                    let resource: $ty = parse_resource_content_offline(&source.to_string()).unwrap();
                    assert_eq!(resource.metadata.creation_timestamp.as_ref().map(|time| time.0), instant);
                    let result = processor.on_init_apply(resource, Some("{\"ancestors\":[]}".into())).await;
                    let WorkItemResult::Processed { obj, .. } = result else { panic!("policy must be processed") };
                    assert_eq!(obj.metadata.creation_timestamp.as_ref().map(|time| time.0), instant);
                    assert_eq!(obj.spec.prepared_precedence.unwrap().creation_timestamp, instant);
                    let wire = serde_json::to_string(&obj).unwrap();
                    let decoded: $ty = serde_json::from_str(&wire).unwrap();
                    assert_eq!(decoded.spec.prepared_precedence.unwrap().creation_timestamp, instant);
                }
                for timestamp in ["-262143-01-01T00:00:00Z", "+262142-12-31T23:59:59Z", "+10000-01-01T00:00:00Z"] {
                    let mut source = fixture["source"][0].clone();
                    source["metadata"]["creationTimestamp"] = json!(timestamp);
                    assert!(parse_resource_content_offline::<$ty>(&source.to_string()).is_err());
                }
                let mut source = fixture["source"][0].clone();
                source["metadata"]["creationTimestamp"] = json!("2016-12-31T23:59:60.123456789Z");
                let resource: $ty = parse_resource_content_offline(&source.to_string()).unwrap();
                let expected = "2016-12-31T23:59:59.123456789Z".parse::<k8s_openapi::jiff::Timestamp>().unwrap();
                assert_eq!(resource.metadata.creation_timestamp.as_ref().unwrap().0, expected);
                let result = processor.on_init_apply(resource, Some("{\"ancestors\":[]}".into())).await;
                let WorkItemResult::Processed { obj, .. } = result else { panic!("policy must be processed") };
                assert_eq!(obj.spec.prepared_precedence.unwrap().creation_timestamp, Some(expected));
                let decoded: $ty = serde_json::from_str(&serde_json::to_string(&obj).unwrap()).unwrap();
                assert_eq!(decoded.spec.prepared_precedence.unwrap().creation_timestamp, Some(expected));
            }};
        }
        check!(BackendTLSPolicy, BackendTlsPolicyHandler);
        check!(EdgionBackendTrafficPolicy, EdgionBackendTrafficPolicyHandler);
    }
}
