use super::filename_identity_mismatch;
use super::parse_resource_content;
use std::path::Path;

#[test]
fn identity_match_returns_none() {
    let path = Path::new("/conf/default/HTTPRoute_default_r1.yaml");
    assert_eq!(filename_identity_mismatch(path, "HTTPRoute", "default/r1"), None);
}

#[test]
fn identity_name_change_is_detected() {
    let path = Path::new("/conf/default/HTTPRoute_default_r1.yaml");
    assert_eq!(
        filename_identity_mismatch(path, "HTTPRoute", "default/r2"),
        Some("default/r1".to_string())
    );
}

#[test]
fn identity_namespace_change_is_detected() {
    let path = Path::new("/conf/HTTPRoute_default_r1.yaml");
    assert_eq!(
        filename_identity_mismatch(path, "HTTPRoute", "other/r1"),
        Some("default/r1".to_string())
    );
}

#[test]
fn identity_cluster_scoped_match_returns_none() {
    let path = Path::new("/conf/GatewayClass__gc1.yaml");
    assert_eq!(filename_identity_mismatch(path, "GatewayClass", "gc1"), None);
}

#[test]
fn identity_cluster_scoped_name_change_is_detected() {
    let path = Path::new("/conf/GatewayClass__gc1.yaml");
    assert_eq!(
        filename_identity_mismatch(path, "GatewayClass", "gc2"),
        Some("gc1".to_string())
    );
}

#[test]
fn identity_unparseable_filename_returns_none() {
    // Filename kind does not match the controller kind → extract_key_from_path
    // returns None, so no mismatch is raised here (handled by other paths).
    let path = Path::new("/conf/random.yaml");
    assert_eq!(filename_identity_mismatch(path, "HTTPRoute", "default/r2"), None);
}
use edgion_resources::prelude_resources::{
    BackendTLSPolicy, BackendTLSPolicySpec, BackendTLSPolicyStatus, BackendTLSPolicyTargetRef,
    BackendTLSPolicyValidation, TLSBackendRef, TLSRoute, TLSRouteRule, TLSRouteSpec, TLSRouteStatus,
};
use edgion_resources::resources::common::ParentReference;

fn make_policy() -> BackendTLSPolicy {
    BackendTLSPolicy {
        metadata: kube::api::ObjectMeta {
            name: Some("btls".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            prepared_precedence: None,
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: "".to_string(),
                kind: "Service".to_string(),
                name: "backend".to_string(),
                section_name: None,
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: "backend.default.svc".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: false,
        },
        status: Some(BackendTLSPolicyStatus { ancestors: vec![] }),
    }
}

#[test]
fn parse_backend_tls_policy_converts_v1alpha3() {
    let yaml = serde_yaml::to_string(&make_policy()).unwrap().replacen(
        "gateway.networking.k8s.io/v1",
        "gateway.networking.k8s.io/v1alpha3",
        1,
    );

    let parsed = parse_resource_content::<BackendTLSPolicy>(&yaml).unwrap();
    assert_eq!(parsed.spec.validation.hostname, "backend.default.svc");
    assert_eq!(
        serde_json::to_value(parsed).unwrap()["apiVersion"],
        "gateway.networking.k8s.io/v1"
    );
}

#[test]
fn parse_backend_tls_policy_rejects_missing_api_version() {
    let yaml = serde_yaml::to_string(&make_policy())
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("apiVersion:"))
        .collect::<Vec<_>>()
        .join("\n");

    let error = parse_resource_content::<BackendTLSPolicy>(&yaml).unwrap_err();
    assert!(error
        .to_string()
        .contains("BackendTLSPolicy apiVersion must use a supported"));
}

#[test]
fn parse_backend_tls_policy_v1_keeps_canonical_type() {
    let policy = make_policy();
    let yaml = serde_yaml::to_string(&policy).unwrap();

    let parsed: BackendTLSPolicy = parse_resource_content(&yaml).unwrap();
    let serialized = serde_yaml::to_value(&parsed).unwrap();

    assert_eq!(
        serialized.get("apiVersion").and_then(|v| v.as_str()),
        Some("gateway.networking.k8s.io/v1")
    );
    assert_eq!(parsed.metadata.name.as_deref(), Some("btls"));
}

fn make_tls_route() -> TLSRoute {
    TLSRoute {
        metadata: kube::api::ObjectMeta {
            name: Some("tr".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: TLSRouteSpec {
            current_status: None,
            resolved_status_controller: None,
            parent_refs: Some(vec![ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("default".to_string()),
                name: "gw".to_string(),
                section_name: None,
                port: None,
            }]),
            hostnames: Some(vec!["example.com".to_string()]),
            rules: Some(vec![TLSRouteRule {
                backend_refs: Some(vec![TLSBackendRef {
                    name: "svc".to_string(),
                    namespace: None,
                    port: Some(443),
                    weight: Some(1),
                    group: None,
                    kind: None,
                    ref_denied: None,
                }]),
            }]),
            resolved_listeners: None,
        },
        status: Some(TLSRouteStatus { parents: vec![] }),
    }
}

#[test]
fn parse_tls_route_rejects_noncanonical_versions() {
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(1)),
        Some(serde_json::json!("")),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha2")),
    ] {
        let mut document = serde_json::to_value(make_tls_route()).unwrap();
        match version {
            Some(value) => document["apiVersion"] = value,
            None => {
                document.as_object_mut().unwrap().remove("apiVersion");
            }
        }
        for body in [
            serde_json::to_string(&document).unwrap(),
            serde_yaml::to_string(&document).unwrap(),
        ] {
            let error = parse_resource_content::<TLSRoute>(&body).unwrap_err();
            assert!(error.to_string().contains("TLSRoute apiVersion must use a supported"));
        }
    }
}

#[test]
fn parse_tls_route_v1_keeps_canonical_type() {
    let route = make_tls_route();
    let yaml = serde_yaml::to_string(&route).unwrap();

    let parsed: TLSRoute = parse_resource_content(&yaml).unwrap();
    let serialized = serde_yaml::to_value(&parsed).unwrap();

    assert_eq!(
        serialized.get("apiVersion").and_then(|v| v.as_str()),
        Some("gateway.networking.k8s.io/v1")
    );
    assert_eq!(parsed.metadata.name.as_deref(), Some("tr"));
}

use super::{spawn_worker, FileSystemEvent, FileSystemResourceController, ResourceKey, ResourceProcessor};
use crate::conf_mgr::conf_center::common::{clear_runtime_invalid, store_runtime_invalid, RuntimeInvalidEnvelopes};
use crate::conf_mgr::conf_center::file_system::path_registry::PathRegistry;
use crate::conf_mgr::conf_center::file_system::status::FileSystemStatusHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, LocalRawStore, ProcessorHandler};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::HTTPRoute;
use edgion_resources::resources::http_route::HTTPRouteSpec;
use kube::api::ObjectMeta;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::timeout;

struct FsNoopHandler;
#[async_trait::async_trait]
impl ProcessorHandler<HTTPRoute> for FsNoopHandler {}

#[tokio::test]
async fn tls_route_ingress_rejects_invalid_versions_before_raw_store_and_recovers() {
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
    struct TlsNoopHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<TLSRoute> for TlsNoopHandler {}

    let tmp = tempfile::tempdir().unwrap();
    let processor = Arc::new(ResourceProcessor::<TLSRoute>::new(
        "TLSRoute",
        100,
        Arc::new(TlsNoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let (tx, rx) = broadcast::channel(16);
    let shutdown = ShutdownHandle::new();
    let controller = FileSystemResourceController::new(
        "TLSRoute",
        processor.clone(),
        tmp.path().to_path_buf(),
        Arc::new(PathRegistry::default()),
        rx,
    )
    .with_shutdown(shutdown.signal());
    let raw = controller.fs_raw_store.clone();
    let run = tokio::spawn(controller.run());
    tx.send(FileSystemEvent::Init).unwrap();
    for (index, version) in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(42)),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha2")),
    ]
    .into_iter()
    .enumerate()
    {
        let mut document = serde_json::to_value(make_tls_route()).unwrap();
        document["metadata"]["name"] = serde_json::json!(format!("invalid-{index}"));
        match version {
            Some(value) => document["apiVersion"] = value,
            None => {
                document.as_object_mut().unwrap().remove("apiVersion");
            }
        }
        tx.send(FileSystemEvent::InitApply {
            path: tmp.path().join(format!("TLSRoute_default_invalid-{index}.yaml")),
            content: serde_yaml::to_string(&document).unwrap(),
        })
        .unwrap();
    }
    // A valid event after the invalid events proves the same stream can recover.
    tx.send(FileSystemEvent::InitApply {
        path: tmp.path().join("TLSRoute_default_tr.yaml"),
        content: serde_yaml::to_string(&make_tls_route()).unwrap(),
    })
    .unwrap();
    tx.send(FileSystemEvent::InitDone).unwrap();
    timeout(Duration::from_secs(5), async {
        while processor.get("default/tr").is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for index in 0..4 {
        let key = format!("default/invalid-{index}");
        assert!(processor.get(&key).is_none());
        assert!(raw.get(&key).is_none());
    }
    assert!(raw.get("default/tr").is_some());
    shutdown.shutdown();
    timeout(Duration::from_secs(5), run).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn duplicate_init_done_keeps_one_worker() {
    struct InitCountingHandler(AtomicUsize);
    #[async_trait::async_trait]
    impl ProcessorHandler<HTTPRoute> for InitCountingHandler {
        fn on_init_done(&self, _ctx: &HandlerContext) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let handler = Arc::new(InitCountingHandler(AtomicUsize::new(0)));
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        handler.clone(),
        Arc::new(ControllerRuntime::default()),
    ));
    let (event_tx, event_rx) = broadcast::channel(16);
    let shutdown = ShutdownHandle::new();
    let controller = FileSystemResourceController::new(
        "HTTPRoute",
        processor.clone(),
        tmp.path().to_path_buf(),
        Arc::new(PathRegistry::default()),
        event_rx,
    )
    .with_shutdown(shutdown.signal());
    let mut run = Box::pin(controller.run());

    event_tx.send(FileSystemEvent::Init).unwrap();
    event_tx.send(FileSystemEvent::InitDone).unwrap();
    assert!(futures::poll!(&mut run).is_pending());
    // With no resources or awaits in the init sequence, this poll drains
    // all events. Only the test, controller, and sole worker own the processor.
    assert_eq!(Arc::strong_count(&processor), 3);
    assert_eq!(handler.0.load(Ordering::Relaxed), 1);

    // Cover both a duplicate completion and a repeated empty init sequence.
    for repeat_init in [false, true] {
        if repeat_init {
            event_tx.send(FileSystemEvent::Init).unwrap();
        }
        event_tx.send(FileSystemEvent::InitDone).unwrap();
        assert!(futures::poll!(&mut run).is_pending());
        assert_eq!(
            Arc::strong_count(&processor),
            3,
            "duplicate InitDone spawned another worker"
        );
        assert_eq!(handler.0.load(Ordering::Relaxed), 1, "init callback ran more than once");
    }

    shutdown.shutdown();
    timeout(Duration::from_secs(1), &mut run).await.unwrap().unwrap();
    drop(run);
    assert_eq!(Arc::strong_count(&processor), 1, "worker was not joined on shutdown");
}

fn make_route(ns: &str, name: &str) -> HTTPRoute {
    HTTPRoute {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            ..Default::default()
        },
        spec: HTTPRouteSpec {
            current_status: None,
            resolved_status_controller: None,
            parent_refs: None,
            hostnames: None,
            resolved_hostnames: None,
            resolved_listeners: None,
            invalid_rule_indices: None,
            rules: None,
            resolved_rules: None,
            delegation_issues: Vec::new(),
            use_default_gateways: None,
        },
        status: None,
    }
}

/// Build processor + worker over a temp conf dir. Returns what tests need
/// to drive and assert; the JoinHandle keeps the worker alive.
type FsWorkerTestHarness = (
    Arc<ResourceProcessor<HTTPRoute>>,
    Arc<PathRegistry>,
    Arc<LocalRawStore<HTTPRoute>>,
    FileSystemStatusHandler,
    Arc<RuntimeInvalidEnvelopes>,
    tokio::task::JoinHandle<()>,
);

fn fs_worker_test_harness(conf_dir: &std::path::Path) -> FsWorkerTestHarness {
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        Arc::new(FsNoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let registry = Arc::new(PathRegistry::default());
    let raw_store = Arc::new(LocalRawStore::<HTTPRoute>::new());
    let status_handler = FileSystemStatusHandler::new(conf_dir.to_path_buf()).with_registry(registry.clone());
    let invalid_envelopes = Arc::new(RuntimeInvalidEnvelopes::default());
    let worker = spawn_worker(
        processor.clone(),
        conf_dir.to_path_buf(),
        registry.clone(),
        "HTTPRoute",
        None,
        raw_store.clone(),
        invalid_envelopes.clone(),
    );
    (
        processor,
        registry,
        raw_store,
        status_handler,
        invalid_envelopes,
        worker,
    )
}

#[tokio::test]
async fn sole_file_delete_removes_resource_and_status() {
    let tmp = tempfile::tempdir().unwrap();
    let (processor, _registry, _raw_store, status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

    // Prior state: resource in cache + a persisted .status file, no spec
    // file on disk (the file was just deleted by the operator).
    let route = make_route("default", "r1");
    processor.save(route, None);
    status_handler
        .write_status_value("HTTPRoute", "default/r1", &serde_json::json!({"parents": []}))
        .await
        .unwrap();
    assert!(status_handler
        .read_status_json("HTTPRoute", "default/r1")
        .await
        .is_some());

    // Delete event arrives: key is enqueued; the worker must delete.
    processor.on_event_key("default/r1".to_string()).await;

    // Poll until the worker has processed (cache emptied), max 5s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get("default/r1").is_some() {
        assert!(std::time::Instant::now() < deadline, "worker did not delete in time");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Status sidecar must be cleaned up too (the bonus fix from the spec).
    assert!(status_handler
        .read_status_json("HTTPRoute", "default/r1")
        .await
        .is_none());
}

#[tokio::test]
async fn never_cached_key_delete_is_skipped_without_status_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (processor, _registry, _raw_store, status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

    // An orphan error-status file from a resource that never parsed.
    status_handler
        .write_status_value(
            "HTTPRoute",
            "default/ghost",
            &serde_json::json!({"error": "ParseError"}),
        )
        .await
        .unwrap();

    // Delete event for a key in neither cache nor store: worker must skip.
    processor.on_event_key("default/ghost".to_string()).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(processor.get("default/ghost").is_none());
    // Documented residual (spec: "Bonus fix" section): the orphan status
    // file is NOT cleaned by the Skipped arm — same as before this change.
    assert!(status_handler
        .read_status_json("HTTPRoute", "default/ghost")
        .await
        .is_some());
}

#[tokio::test]
async fn delete_then_recreate_before_dequeue_reapplies() {
    let tmp = tempfile::tempdir().unwrap();
    let (processor, _registry, _raw_store, _status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

    // v1 is cached; the file is deleted (delete event pending), but a new
    // file for the same key reappears before the worker dequeues — the
    // level-triggered outcome must be re-apply, not delete.
    let v1 = make_route("default", "r1");
    processor.save(v1, None);

    // Recreate on disk at the canonical fallback path (registry has no
    // record, so the worker falls back to build_resource_path_from_key).
    let mut v2 = make_route("default", "r1");
    v2.spec.hostnames = Some(vec!["v2.example.com".to_string()]);
    let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    tokio::fs::write(&path, serde_yaml::to_string(&v2).unwrap())
        .await
        .unwrap();

    processor.on_event_key("default/r1".to_string()).await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(cur) = processor.get("default/r1") {
            if cur.spec.hostnames.as_deref() == Some(&["v2.example.com".to_string()]) {
                break; // re-applied with v2 content
            }
        } else {
            panic!("resource was deleted; expected level-triggered re-apply");
        }
        assert!(std::time::Instant::now() < deadline, "worker did not re-apply in time");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn handler_owned_status_survives_a_statusless_reconcile() {
    let tmp = tempfile::tempdir().unwrap();
    let (processor, _registry, raw_store, status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());
    let key = "default/r1";
    let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();

    // A status this controller computed earlier, not a failure sidecar.
    let owned_status = serde_json::json!({
        "parents": [{
            "parentRef": {"name": "gw"},
            "controllerName": "edgion.io/gateway-controller",
            "conditions": [{
                "type": "Accepted",
                "status": "True",
                "reason": "Accepted",
                "message": "Route accepted",
                "lastTransitionTime": "2026-01-01T00:00:00Z"
            }]
        }]
    });
    status_handler
        .write_status_value("HTTPRoute", key, &owned_status)
        .await
        .unwrap();

    let route = make_route("default", "r1");
    tokio::fs::write(&path, serde_yaml::to_string(&route).unwrap())
        .await
        .unwrap();
    raw_store.upsert(key.to_string(), Arc::new(route));
    processor.on_event_key(key.to_string()).await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_none() {
        assert!(std::time::Instant::now() < deadline, "worker did not apply object");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // The handler computes no status here, so nothing replaces the stored
    // one; the cleanup must not mistake it for a failure sidecar.
    assert!(
        status_handler.read_status_json("HTTPRoute", key).await.is_some(),
        "a handler-owned status must survive a successful reconcile"
    );
}

#[tokio::test]
async fn imported_controller_envelope_status_is_never_cleared() {
    use edgion_resources::prelude_resources::EdgionPlugins;

    struct PluginsNoopHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for PluginsNoopHandler {}

    let tmp = tempfile::tempdir().unwrap();
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        100,
        Arc::new(PluginsNoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let registry = Arc::new(PathRegistry::default());
    let raw_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
    let status_handler = FileSystemStatusHandler::new(tmp.path().to_path_buf()).with_registry(registry.clone());
    let _worker = spawn_worker(
        processor.clone(),
        tmp.path().to_path_buf(),
        registry.clone(),
        "EdgionPlugins",
        None,
        raw_store.clone(),
        Arc::new(RuntimeInvalidEnvelopes::default()),
    );

    let key = "default/p1";
    let path = tmp.path().join("default").join("EdgionPlugins_default_p1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "p1", "namespace": "default"},
        "spec": {}
    }))
    .unwrap();
    tokio::fs::write(&path, serde_yaml::to_string(&plugins).unwrap())
        .await
        .unwrap();
    raw_store.upsert(key.to_string(), Arc::new(plugins));

    // A Kubernetes controller envelope carried into FileSystem mode. It is
    // legitimate persisted status, but it is not native here, so the
    // pipeline clears it from the object and computes nothing to persist.
    status_handler
        .write_status_value(
            "EdgionPlugins",
            key,
            &serde_json::json!({"controllers": [{
                "controllerName": "edgion.io/gateway-controller",
                "status": {"conditions": []}
            }]}),
        )
        .await
        .unwrap();

    processor.on_event_key(key.to_string()).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_none() {
        assert!(std::time::Instant::now() < deadline, "worker did not apply object");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert!(
        status_handler.read_status_json("EdgionPlugins", key).await.is_some(),
        "only an error sidecar may be cleared; a foreign-but-real status must survive"
    );
}

#[tokio::test]
async fn init_clears_error_status_of_a_resource_repaired_while_down() {
    let tmp = tempfile::tempdir().unwrap();
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        Arc::new(FsNoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let registry = Arc::new(PathRegistry::default());
    let key = "default/r1";
    let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    registry.record(ResourceKey::new("HTTPRoute", key), path.clone());
    let status_handler = FileSystemStatusHandler::new(tmp.path().to_path_buf()).with_registry(registry.clone());

    // The previous run left a ParseError beside the file; the operator then
    // repaired it while the controller was down.
    status_handler
        .write_error_status("HTTPRoute", key, "ParseError", "invalid listener structure")
        .await
        .unwrap();
    let repaired = make_route("default", "r1");
    let repaired_content = serde_yaml::to_string(&repaired).unwrap();
    tokio::fs::write(&path, &repaired_content).await.unwrap();

    let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);
    let shutdown = crate::conf_mgr::sync_runtime::ShutdownHandle::new();
    let controller = FileSystemResourceController::new(
        "HTTPRoute",
        processor.clone(),
        tmp.path().to_path_buf(),
        registry,
        event_rx,
    )
    .with_shutdown(shutdown.signal());
    let controller_task = tokio::spawn(controller.run());

    event_tx.send(FileSystemEvent::Init).unwrap();
    event_tx
        .send(FileSystemEvent::InitApply {
            path: path.clone(),
            content: repaired_content,
        })
        .unwrap();
    event_tx.send(FileSystemEvent::InitDone).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "controller did not apply repaired object during init"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert!(
        status_handler.read_status_json("HTTPRoute", key).await.is_none(),
        "init must not leave a repaired resource reporting its previous failure"
    );
    shutdown.shutdown();
    assert!(controller_task.await.unwrap().is_ok());
}

#[tokio::test]
async fn valid_to_malformed_to_repair_revokes_then_recovers_via_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let (processor, _registry, raw_store, status_handler, invalid, _worker) = fs_worker_test_harness(tmp.path());
    let key = "default/r1";
    let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();

    let v1 = make_route("default", "r1");
    tokio::fs::write(&path, serde_yaml::to_string(&v1).unwrap())
        .await
        .unwrap();
    raw_store.upsert(key.to_string(), Arc::new(v1.clone()));
    processor.save(v1, None);

    raw_store.remove(key);
    store_runtime_invalid(
        &invalid,
        key.to_string(),
        "ParseError",
        "invalid listener structure".to_string(),
    );
    processor.on_event_key(key.to_string()).await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not revoke malformed object"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let error_status = status_handler.read_status_json("HTTPRoute", key).await.unwrap();
    assert!(error_status.contains("ParseError"));

    let mut v2 = make_route("default", "r1");
    v2.spec.hostnames = Some(vec!["repaired.example.com".to_string()]);
    tokio::fs::write(&path, serde_yaml::to_string(&v2).unwrap())
        .await
        .unwrap();
    clear_runtime_invalid(&invalid, key);
    raw_store.upsert(key.to_string(), Arc::new(v2));
    processor.on_event_key(key.to_string()).await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if processor
            .get(key)
            .is_some_and(|route| route.spec.hostnames.as_deref() == Some(&["repaired.example.com".to_string()]))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not apply repaired object"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let repaired_status = status_handler.read_status_json("HTTPRoute", key).await;
    assert!(repaired_status.is_none_or(|status| !status.contains("ParseError")));
}

#[tokio::test]
async fn valid_to_identity_mismatch_to_repair_revokes_then_recovers_via_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        Arc::new(FsNoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let registry = Arc::new(PathRegistry::default());
    let key = "default/r1";
    let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    registry.record(ResourceKey::new("HTTPRoute", key), path.clone());
    let status_handler = FileSystemStatusHandler::new(tmp.path().to_path_buf()).with_registry(registry.clone());
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);
    let shutdown = crate::conf_mgr::sync_runtime::ShutdownHandle::new();
    let controller = FileSystemResourceController::new(
        "HTTPRoute",
        processor.clone(),
        tmp.path().to_path_buf(),
        registry,
        event_rx,
    )
    .with_shutdown(shutdown.signal());
    let controller_task = tokio::spawn(controller.run());

    let v1 = make_route("default", "r1");
    tokio::fs::write(&path, serde_yaml::to_string(&v1).unwrap())
        .await
        .unwrap();
    event_tx.send(FileSystemEvent::Init).unwrap();
    event_tx
        .send(FileSystemEvent::InitApply {
            path: path.clone(),
            content: serde_yaml::to_string(&v1).unwrap(),
        })
        .unwrap();
    event_tx.send(FileSystemEvent::InitDone).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "controller did not apply initial object"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let mismatched = make_route("default", "other");
    let mismatched_content = serde_yaml::to_string(&mismatched).unwrap();
    tokio::fs::write(&path, &mismatched_content).await.unwrap();
    // The watcher payload still describes the authoritative key, but the
    // file is replaced before the sole worker re-reads it. This exercises
    // the production dequeue-time identity check rather than the earlier
    // event-parser check.
    let mut queued = make_route("default", "r1");
    queued.spec.hostnames = Some(vec!["queued.example.com".to_string()]);
    event_tx
        .send(FileSystemEvent::Apply {
            path: path.clone(),
            content: serde_yaml::to_string(&queued).unwrap(),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while processor.get(key).is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not revoke identity-mismatched object"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let error_status = status_handler.read_status_json("HTTPRoute", key).await.unwrap();
    assert!(error_status.contains("IdentityMismatch"));

    let mut repaired = make_route("default", "r1");
    repaired.spec.hostnames = Some(vec!["repaired.example.com".to_string()]);
    let repaired_content = serde_yaml::to_string(&repaired).unwrap();
    tokio::fs::write(&path, &repaired_content).await.unwrap();
    event_tx
        .send(FileSystemEvent::Apply {
            path: path.clone(),
            content: repaired_content,
        })
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if processor
            .get(key)
            .is_some_and(|route| route.spec.hostnames.as_deref() == Some(&["repaired.example.com".to_string()]))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not apply repaired identity"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let repaired_status = status_handler.read_status_json("HTTPRoute", key).await;
    assert!(repaired_status.is_none_or(|status| !status.contains("IdentityMismatch")));
    shutdown.shutdown();
    assert!(controller_task.await.unwrap().is_ok());
}

#[tokio::test]
async fn worker_read_failure_removes_raw_generation_and_plain_requeue_repairs_it() {
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
    use crate::conf_mgr::sync_runtime::resource_processor::ProcessorObj;

    let tmp = tempfile::tempdir().unwrap();
    let (processor, registry, raw_store, status_handler, invalid, worker) = fs_worker_test_harness(tmp.path());
    processor.set_raw_store(raw_store.clone());
    let key = "default/r1";
    let path = tmp.path().join("source").join("HTTPRoute_default_r1.yaml");
    tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    registry.record(ResourceKey::new("HTTPRoute".to_string(), key.to_string()), path.clone());
    let prior = make_route("default", "r1");
    tokio::fs::write(&path, serde_yaml::to_string(&prior).unwrap())
        .await
        .unwrap();
    raw_store.upsert(key.to_string(), Arc::new(prior.clone()));
    processor.save(prior, None);
    assert!(processor.get_raw_any(key).is_some());
    assert!(processor.get(key).is_some());

    // The existing winner still exists, but the worker cannot decode its bytes.
    tokio::fs::write(&path, [0xff]).await.unwrap();
    processor.on_event_key(key.to_string()).await;
    timeout(Duration::from_secs(5), async {
        loop {
            let status = status_handler.read_status_json("HTTPRoute", key).await;
            if status.is_some_and(|status| status.contains("ParseError"))
                && processor.get(key).is_none()
                && raw_store.get(key).is_none()
                && processor.get_raw_any(key).is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker must withdraw both generations and persist ParseError");
    assert!(invalid.read().is_empty(), "read failures must not block later rereads");

    let mut repaired = make_route("default", "r1");
    repaired.spec.hostnames = Some(vec!["repaired.example.com".to_string()]);
    tokio::fs::write(&path, serde_yaml::to_string(&repaired).unwrap())
        .await
        .unwrap();
    // No Apply, raw upsert or invalid-envelope clear: ordinary requeue must recover.
    processor.requeue(key.to_string()).await;
    timeout(Duration::from_secs(5), async {
        loop {
            let matches_repair = |route: &HTTPRoute| route.spec.hostnames == repaired.spec.hostnames;
            let raw = processor
                .get_raw_any(key)
                .and_then(|obj| obj.downcast::<HTTPRoute>().ok());
            if processor.get(key).as_ref().is_some_and(matches_repair)
                && raw.as_deref().is_some_and(matches_repair)
                && status_handler.read_status_json("HTTPRoute", key).await.is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("plain requeue must restore raw/cache state and clear the error sidecar");
    assert!(invalid.read().is_empty());
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
}
