//! In-memory transport and lifecycle regressions for `resource_controller.rs`.
//! The transport models atomic RV comparison and Merge Patch, not CRD admission or SSA ownership.

use super::*;
use crate::conf_mgr::conf_center::kubernetes::leader_election::{LeaderElection, LeaderElectionConfig};
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, LocalRawStore, ProcessorHandler, ProcessorObj,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::*;
use edgion_resources::resource::defs::ALL_RESOURCE_INFOS;
use k8s_openapi::api::core::v1::Service;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

struct ApiState {
    object: Value,
    requests: Vec<(String, Value)>,
    error_code: Option<u16>,
    sequence: u64,
}

fn api_client(object: Value) -> (Client, Arc<Mutex<ApiState>>) {
    let state = Arc::new(Mutex::new(ApiState {
        object,
        requests: Vec::new(),
        error_code: None,
        sequence: 0,
    }));
    let shared = state.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let shared = shared.clone();
        async move {
            assert_eq!(request.method(), http::Method::PATCH);
            assert_eq!(request.headers()["content-type"], "application/merge-patch+json");
            let (parts, body) = request.into_parts();
            let patch: Value = serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
            let mut state = shared.lock();
            state.requests.push((parts.uri.path().to_owned(), patch.clone()));
            // Compare and mutate under one lock: model the API server's atomic conditional write.
            let error = state.error_code.or_else(|| {
                (patch["metadata"]["resourceVersion"] != state.object["metadata"]["resourceVersion"]).then_some(409)
            });
            let (code, value) = if let Some(code) = error {
                (
                    code,
                    json!({"kind": "Status", "apiVersion": "v1", "status": "Failure",
                    "reason": "Rejected", "message": "Rejected test status write", "code": code}),
                )
            } else {
                json_patch::merge(&mut state.object, &patch);
                state.sequence += 1;
                state.object["metadata"]["resourceVersion"] = json!(format!("server-{}", state.sequence));
                (200, state.object.clone())
            };
            Ok::<_, Infallible>(
                http::Response::builder()
                    .status(code)
                    .body(kube::client::Body::from(serde_json::to_vec(&value).unwrap()))
                    .unwrap(),
            )
        }
    });
    (Client::new(transport, "default"), state)
}

fn scope(cluster: bool) -> ApiScope {
    if cluster {
        ApiScope::ClusterScoped
    } else {
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces)
    }
}

#[tokio::test]
async fn gateway_class_beta_status_uses_selected_path_and_original_rv() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::{GatewayClassHandler, GatewayClassOwnership};
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
    let foreign = json!({"type": "Foreign", "status": "True", "reason": "OtherController",
        "message": "retained", "lastTransitionTime": "2026-01-01T00:00:00Z"});
    let initial = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
        "metadata": {"name": "edge", "uid": "uid-1", "resourceVersion": "init-rv", "generation": 1},
        "spec": {"controllerName": "example.com/edge"},
        "status": {"conditions": [foreign.clone()], "supportedFeatures": []}
    });
    let (client, state) = api_client(initial.clone());
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "example.com/edge".into(),
    });
    let processor = Arc::new(ResourceProcessor::new(
        "GatewayClass",
        8,
        Arc::new(GatewayClassHandler::new(GatewayClassOwnership::new(
            "example.com/edge".into(),
        ))),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "GatewayClass",
        client,
        processor.clone(),
        scope(true),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone())
    .with_status_version("v1beta1");
    let (tx, rx) = mpsc::channel::<watcher::Result<Event<GatewayClassBeta>>>(32);
    let stream: NamespacedWatcher<GatewayClass> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv()
            .await
            .map(|event| (event.map(canonical_gateway_class_event), rx))
    }));
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::Cluster, stream)]));
    let beta = |value: Value| serde_json::from_value::<GatewayClassBeta>(value).unwrap();
    for event in [Event::Init, Event::InitApply(beta(initial)), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready() && state.lock().requests.len() == 1).await;
    assert!(processor.get("edge").is_some());
    let raw = processor.get_raw_any("edge").unwrap();
    let raw = raw.downcast::<GatewayClass>().unwrap();
    assert_eq!(raw.metadata.resource_version.as_deref(), Some("init-rv"));
    assert_eq!(raw.spec.controller_name, "example.com/edge");
    {
        let state = state.lock();
        assert_eq!(
            state.requests[0].0,
            "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses/edge/status"
        );
        assert_eq!(state.requests[0].1["metadata"]["resourceVersion"], "init-rv");
        assert!(state.object["status"]["conditions"]
            .as_array()
            .unwrap()
            .contains(&foreign));
    }

    // A changed runtime snapshot must use its own original RV and retain the foreign condition.
    let mut runtime_input = state.lock().object.clone();
    runtime_input["metadata"]["resourceVersion"] = json!("runtime-rv");
    runtime_input["metadata"]["generation"] = json!(2);
    runtime_input["status"]["supportedFeatures"] = json!([]);
    state.lock().object = runtime_input.clone();
    tx.send(Ok(Event::Apply(beta(runtime_input)))).await.unwrap();
    eventually(|| state.lock().requests.len() == 2).await;
    {
        let state = state.lock();
        assert_eq!(
            state.requests[1].0,
            "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses/edge/status"
        );
        assert_eq!(state.requests[1].1["metadata"]["resourceVersion"], "runtime-rv");
        assert!(state.object["status"]["conditions"]
            .as_array()
            .unwrap()
            .contains(&foreign));
    }

    // A status echo and full relist converge through the same worker without another write.
    let echo = state.lock().object.clone();
    tx.send(Ok(Event::Apply(beta(echo.clone())))).await.unwrap();
    eventually(|| {
        processor
            .get("edge")
            .is_some_and(|obj| obj.metadata.resource_version.as_deref() == echo["metadata"]["resourceVersion"].as_str())
    })
    .await;
    let mut relist = echo;
    relist["metadata"]["resourceVersion"] = json!("relist-rv");
    state.lock().object = relist.clone();
    for event in [
        Event::Init,
        Event::InitApply(beta(relist)),
        Event::InitDone,
        Event::InitDone,
    ] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| {
        processor
            .get("edge")
            .is_some_and(|obj| obj.metadata.resource_version.as_deref() == Some("relist-rv"))
    })
    .await;
    assert_eq!(state.lock().requests.len(), 2);
    assert_eq!(
        tracker.len(),
        2,
        "one source driver and one resource worker after relist"
    );
    cancel.cancel();
    task.await.unwrap().unwrap();
    runtime.reset();
}

fn object(status: Value) -> Value {
    json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "resource", "namespace": "apps", "uid": "original", "resourceVersion": "opaque-rv"},
        "spec": {}, "status": status})
}

fn parent(owner: &str) -> Value {
    json!({"parentRef": {"name": "gateway"}, "controllerName": owner,
        "conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted",
            "message": "Accepted", "observedGeneration": 1, "lastTransitionTime": "2000-01-01T00:00:00Z"}]})
}

fn assert_conflict(result: Result<(), StatusWriteError>) {
    assert!(matches!(result, Err(StatusWriteError::Kubernetes(kube::Error::Api(ref e))) if e.code == 409));
}

#[tokio::test]
async fn matching_gateway_class_patch_updates_features_and_watch_echo_converges() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::{GatewayClassHandler, GatewayClassOwnership};
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "example.com/controller".into(),
    });
    let processor = ResourceProcessor::new(
        "GatewayClass",
        8,
        Arc::new(GatewayClassHandler::new(GatewayClassOwnership::new(
            "example.com/controller".into(),
        ))),
        runtime,
    );
    let initial = json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
        "metadata": {"name": "resource", "uid": "uid", "resourceVersion": "opaque-rv", "generation": 1},
        "spec": {"controllerName": "example.com/controller"},
        "status": {"supportedFeatures": [{"name": "Gateway"}], "conditions": []}});
    let (client, state) = api_client(initial);
    for round in 0..4 {
        let snapshot = state.lock().object.clone();
        let obj = serde_json::from_value(snapshot.clone()).unwrap();
        let WorkItemResult::Processed {
            status_changed,
            persisted_status,
            ..
        } = processor.on_init_apply(obj, None).await
        else {
            panic!("missing class status");
        };
        assert_eq!(status_changed, round == 0);
        if status_changed {
            persist_k8s_status::<GatewayClass>(
                &client,
                &scope(true),
                None,
                "resource",
                snapshot["metadata"]["resourceVersion"].as_str(),
                &persisted_status.unwrap(),
            )
            .await
            .unwrap();
        }
        assert!(processor.get("resource").is_some());
        assert!(!state.lock().object["status"]["supportedFeatures"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    assert_eq!(state.lock().requests.len(), 1);
}

#[tokio::test]
async fn canonical_observation_patch_removes_only_retired_native_fields_and_converges() {
    use crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed;
    use crate::conf_mgr::sync_runtime::resource_processor::status_observation::{
        merge_status, project_status, retired_native_fields, ObserverIdentity,
    };
    let identity = ObserverIdentity {
        controller_name: "example/controller".into(),
    };
    let old_native = json!({"conditions": [], "observedGeneration": 1,
        "credentialState": "Valid", "publishedGeneration": "2", "issuedAt": "2026-01-01T00:00:00Z",
        "validUntil": "2026-02-01T00:00:00Z", "lastSuccessfulRefreshTime": "2026-01-01T00:00:00Z",
        "nextRefreshTime": "2026-01-20T00:00:00Z", "desiredConsumerCount": 3,
        "publishedConsumerCount": 3, "conflictedConsumerCount": 0});
    assert_eq!(project_status("LinkSys", &old_native, &identity).unwrap(), None);
    let local = json!({"conditions": []});
    let logical = merge_status("LinkSys", Some(&old_native), &local, &identity)
        .unwrap()
        .unwrap();
    let mut wire = logical.clone();
    for field in retired_native_fields("LinkSys", &old_native).unwrap() {
        wire[field] = Value::Null;
    }
    let mut initial = object(old_native);
    initial["apiVersion"] = json!("edgion.io/v1");
    initial["kind"] = json!("LinkSys");
    let (client, state) = api_client(initial.clone());
    assert_conflict(
        persist_k8s_status::<LinkSys>(&client, &scope(false), Some("apps"), "resource", Some("stale"), &wire).await,
    );
    assert_eq!(state.lock().object, initial);
    persist_k8s_status::<LinkSys>(
        &client,
        &scope(false),
        Some("apps"),
        "resource",
        Some("opaque-rv"),
        &wire,
    )
    .await
    .unwrap();
    let echo = state.lock().object["status"].clone();
    assert_eq!(
        echo, logical,
        "retired lifecycle fields must not remain beside controllers"
    );
    let next = merge_status("LinkSys", Some(&echo), &local, &identity)
        .unwrap()
        .unwrap();
    assert!(!status_semantically_changed(
        "LinkSys",
        &echo.to_string(),
        &next.to_string(),
        None
    ));
    assert_eq!(state.lock().requests.len(), 2);
}

#[test]
fn native_empty_fields_elided_by_typed_ingest_are_still_retired_on_the_wire() {
    use crate::conf_mgr::sync_runtime::resource_processor::status_observation::{
        merge_status, retired_native_fields, ObserverIdentity,
    };
    use edgion_resources::resources::link_sys::LinkSysStatus;
    let identity = ObserverIdentity {
        controller_name: "example/controller".into(),
    };
    let api_status = json!({"conditions": [], "publishedGeneration": null, "nextRefreshTime": null});
    let typed: LinkSysStatus = serde_json::from_value(api_status.clone()).unwrap();
    let captured = serde_json::to_value(typed).unwrap();
    assert_eq!(captured, json!({}), "typed serialization omits empty and null fields");
    let logical = merge_status("LinkSys", Some(&captured), &json!({}), &identity)
        .unwrap()
        .unwrap();
    let mut wire = logical.clone();
    for field in retired_native_fields("LinkSys", &captured).unwrap() {
        wire[field] = Value::Null;
    }
    let mut echo = api_status;
    json_patch::merge(&mut echo, &wire);
    assert_eq!(echo, logical);
    let next = merge_status("LinkSys", Some(&echo), &json!({}), &identity)
        .unwrap()
        .unwrap();
    assert!(
        !crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed(
            "LinkSys",
            &echo.to_string(),
            &next.to_string(),
            None
        )
    );
}

async fn check_kind<K>(api_version: &str, plural: &str, cluster: bool) -> String
where
    K: Resource + Clone + Debug + DeserializeOwned + Serialize,
    K::DynamicType: Default,
{
    let kind = K::kind(&K::DynamicType::default()).to_string();
    let desired = match kind.as_str() {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" | "EdgionTls" => {
            json!({"parents": [parent("edgion.io/controller")]})
        }
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => json!({"ancestors": []}),
        "Gateway" => json!({"conditions": [], "listeners": [], "addresses": []}),
        "GatewayClass" => json!({"conditions": [], "supportedFeatures": []}),
        "EdgionAcme" => json!({"conditions": [], "phase": "Pending"}),
        _ => json!({"conditions": []}),
    };
    let mut current = object(json!({"unmanaged": "preserve"}));
    current["apiVersion"] = json!(api_version);
    current["kind"] = json!(kind);
    if cluster {
        current["metadata"].as_object_mut().unwrap().remove("namespace");
    }
    let (client, state) = api_client(current.clone());
    let ns = (!cluster).then_some("apps");
    let result = persist_k8s_status::<K>(&client, &scope(cluster), ns, "resource", Some("older-rv"), &desired).await;
    assert_conflict(result);
    assert_eq!(
        state.lock().object,
        current,
        "{kind}: rejected write mutated the object"
    );
    persist_k8s_status::<K>(&client, &scope(cluster), ns, "resource", Some("opaque-rv"), &desired)
        .await
        .unwrap();
    let state = state.lock();
    let expected_path = if cluster {
        format!("/apis/{api_version}/{plural}/resource/status")
    } else {
        format!("/apis/{api_version}/namespaces/apps/{plural}/resource/status")
    };
    assert_eq!(state.requests.len(), 2, "{kind}: each call makes exactly one attempt");
    assert_eq!(state.requests[1].0, expected_path);
    assert_eq!(
        state.requests[1].1,
        json!({"metadata": {"resourceVersion": "opaque-rv"}, "status": desired})
    );
    assert_eq!(state.object["status"]["unmanaged"], "preserve");
    for (field, value) in desired.as_object().unwrap() {
        assert_eq!(&state.object["status"][field], value);
    }
    kind
}

#[tokio::test]
async fn every_registered_status_kind_inherits_conditional_persistence() {
    let mut covered = BTreeSet::new();
    macro_rules! check {
        ($ty:ty, $version:literal, $plural:literal, $cluster:literal) => {
            assert!(covered.insert(check_kind::<$ty>($version, $plural, $cluster).await));
        };
    }
    check!(HTTPRoute, "gateway.networking.k8s.io/v1", "httproutes", false);
    check!(GRPCRoute, "gateway.networking.k8s.io/v1", "grpcroutes", false);
    check!(TCPRoute, "gateway.networking.k8s.io/v1", "tcproutes", false);
    check!(TLSRoute, "gateway.networking.k8s.io/v1", "tlsroutes", false);
    check!(UDPRoute, "gateway.networking.k8s.io/v1", "udproutes", false);
    check!(EdgionTls, "edgion.io/v1", "edgiontls", false);
    check!(
        BackendTLSPolicy,
        "gateway.networking.k8s.io/v1",
        "backendtlspolicies",
        false
    );
    check!(
        EdgionBackendTrafficPolicy,
        "edgion.io/v1",
        "edgionbackendtrafficpolicies",
        false
    );
    check!(Gateway, "gateway.networking.k8s.io/v1", "gateways", false);
    check!(GatewayClass, "gateway.networking.k8s.io/v1", "gatewayclasses", true);
    check!(
        EdgionGatewayConfig,
        "edgion.io/v1alpha1",
        "edgiongatewayclassconfigs",
        true
    );
    check!(EdgionPlugins, "edgion.io/v1", "edgionplugins", false);
    check!(EdgionStreamPlugins, "edgion.io/v1", "edgionstreamplugins", false);
    check!(EdgionConfigData, "edgion.io/v1", "edgionconfigdata", false);
    check!(EdgionBackend, "edgion.io/v1", "edgionbackends", false);
    check!(LinkSys, "edgion.io/v1", "linksys", false);
    check!(EdgionAcme, "edgion.io/v1", "edgionacmes", false);
    let observed_only: BTreeSet<_> = ["Secret", "ConfigMap", "Service", "EndpointSlice", "ReferenceGrant"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert!(covered.is_disjoint(&observed_only));
    covered.extend(observed_only);
    let registered: BTreeSet<_> = ALL_RESOURCE_INFOS
        .iter()
        .map(|info| info.kind.as_str().to_owned())
        .collect();
    assert_eq!(
        covered, registered,
        "Classify new resources and add status transport coverage"
    );
}

#[tokio::test]
async fn missing_version_never_sends_an_unconditional_patch() {
    let (client, state) = api_client(object(json!({})));
    for rv in [None, Some("")] {
        let result = persist_k8s_status::<HTTPRoute>(
            &client,
            &scope(false),
            Some("apps"),
            "resource",
            rv,
            &json!({"parents": []}),
        )
        .await;
        assert!(matches!(result, Err(StatusWriteError::MissingResourceVersion)));
    }
    assert!(state.lock().requests.is_empty());
}

#[tokio::test]
async fn generic_persistence_has_no_status_kind_allowlist() {
    // Service has no production status writer here; a future handler still inherits the guard.
    let mut initial = object(json!({}));
    initial["kind"] = json!("Service");
    initial["apiVersion"] = json!("v1");
    let (client, state) = api_client(initial);
    persist_k8s_status::<Service>(
        &client,
        &scope(false),
        Some("apps"),
        "resource",
        Some("opaque-rv"),
        &json!({"loadBalancer": {}}),
    )
    .await
    .unwrap();
    assert_eq!(
        state.lock().requests[0].0,
        "/api/v1/namespaces/apps/services/resource/status"
    );
    assert_conflict(
        persist_k8s_status::<Service>(
            &client,
            &scope(false),
            Some("apps"),
            "resource",
            Some("opaque-rv"),
            &json!({"loadBalancer": {}}),
        )
        .await,
    );
}

#[tokio::test]
async fn stale_status_cannot_overwrite_foreign_spec_metadata_or_recreated_object() {
    for change in ["status", "spec", "metadata", "recreated"] {
        let old = object(json!({"parents": [parent("edgion.io/controller")]}));
        let mut current = old.clone();
        current["metadata"]["resourceVersion"] = json!("new-rv");
        match change {
            "status" => current["status"]["parents"]
                .as_array_mut()
                .unwrap()
                .push(parent("foreign.example/controller")),
            "spec" => current["spec"]["hostnames"] = json!(["new.example"]),
            "metadata" => current["metadata"]["labels"] = json!({"owner": "new"}),
            "recreated" => {
                current["metadata"]["uid"] = json!("replacement");
                current["status"] = json!({});
            }
            _ => unreachable!(),
        }
        let (client, state) = api_client(current.clone());
        assert_conflict(
            persist_k8s_status::<HTTPRoute>(
                &client,
                &scope(false),
                Some("apps"),
                "resource",
                Some("opaque-rv"),
                &old["status"],
            )
            .await,
        );
        assert_eq!(state.lock().object, current, "{change}");
    }
}

#[tokio::test]
async fn explicit_empty_status_arrays_still_clear_owned_entries() {
    for field in ["parents", "ancestors"] {
        let (client, state) = api_client(object(json!({field: [parent("edgion.io/controller")]})));
        let status = json!({field: []});
        if field == "parents" {
            persist_k8s_status::<HTTPRoute>(
                &client,
                &scope(false),
                Some("apps"),
                "resource",
                Some("opaque-rv"),
                &status,
            )
            .await
            .unwrap();
        } else {
            persist_k8s_status::<BackendTLSPolicy>(
                &client,
                &scope(false),
                Some("apps"),
                "resource",
                Some("opaque-rv"),
                &status,
            )
            .await
            .unwrap();
        }
        assert_eq!(state.lock().object["status"][field], json!([]));
    }
}

#[tokio::test]
async fn status_api_errors_are_returned_after_one_attempt() {
    for code in [403, 404, 409, 422, 503] {
        let initial = object(json!({}));
        let (client, state) = api_client(initial.clone());
        state.lock().error_code = Some(code);
        let error = persist_k8s_status::<HTTPRoute>(
            &client,
            &scope(false),
            Some("apps"),
            "resource",
            Some("opaque-rv"),
            &json!({"parents": []}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.is_not_found(), code == 404);
        assert!(matches!(error, StatusWriteError::Kubernetes(kube::Error::Api(e)) if e.code == code));
        assert_eq!(state.lock().requests.len(), 1);
        assert_eq!(state.lock().object, initial);
    }
}

#[tokio::test]
async fn generic_acme_write_cannot_roll_back_lifecycle_or_revive_cleared_challenges() {
    let old = object(json!({"phase": "Issuing", "activeChallenges": [{"token": "old-token"}], "conditions": []}));
    for update in [
        json!({"phase": "Ready", "certificateSerial": "new-serial"}),
        json!({"activeChallenges": []}),
    ] {
        // Model the successful dedicated writer's result, not SSA managedFields semantics.
        let mut current = old.clone();
        current["apiVersion"] = json!("edgion.io/v1");
        current["kind"] = json!("EdgionAcme");
        json_patch::merge(&mut current["status"], &update);
        current["metadata"]["resourceVersion"] = json!("after-service-write");
        let (client, state) = api_client(current.clone());
        assert_conflict(
            persist_k8s_status::<EdgionAcme>(
                &client,
                &scope(false),
                Some("apps"),
                "resource",
                Some("opaque-rv"),
                &old["status"],
            )
            .await,
        );
        assert_eq!(state.lock().object, current);
    }
}

#[derive(Default)]
struct ChangingMetadataHandler {
    applied: AtomicUsize,
}

#[async_trait::async_trait]
impl ProcessorHandler<HTTPRoute> for ChangingMetadataHandler {
    fn update_status(&self, obj: &mut HTTPRoute, _: &HandlerContext, _: &[String]) {
        let status = obj.status.get_or_insert_with(Default::default);
        status
            .parents
            .retain(|entry| entry.controller_name != "edgion.io/controller");
        status
            .parents
            .push(serde_json::from_value(parent("edgion.io/controller")).unwrap());
    }

    fn finalize_transport(&self, obj: &mut HTTPRoute, _: &HandlerContext) {
        obj.metadata.resource_version = Some("processed-rv-must-not-be-used".into());
    }

    async fn on_change(&self, _: &HTTPRoute, _: &HandlerContext) {
        self.applied.fetch_add(1, Ordering::SeqCst);
    }
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[derive(Default)]
struct UnpublishedClassHandler(AtomicUsize);

#[async_trait::async_trait]
impl ProcessorHandler<GatewayClass> for UnpublishedClassHandler {
    fn filter(&self, _obj: &GatewayClass, _ctx: &HandlerContext) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        false
    }
}

#[tokio::test]
async fn raw_foreign_class_commits_requeue_without_status_echo_fanout() {
    for filtered in [false, true] {
        let runtime = Arc::new(ControllerRuntime::new());
        let handler = Arc::new(UnpublishedClassHandler::default());
        let classes = Arc::new(ResourceProcessor::new(
            "GatewayClass",
            100,
            handler.clone(),
            runtime.clone(),
        ));
        runtime.processor_registry.register(classes.clone());
        let routes = Arc::new(ResourceProcessor::new(
            "HTTPRoute",
            100,
            Arc::new(ChangingMetadataHandler::default()),
            runtime.clone(),
        ));
        let route_raw = Arc::new(LocalRawStore::new());
        route_raw.upsert(
            "apps/resource".into(),
            Arc::new(serde_json::from_value(object(json!({}))).unwrap()),
        );
        routes.set_raw_store(route_raw);
        runtime.processor_registry.register(routes.clone());
        let (client, state) = api_client(json!({}));
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let mut config = watcher::Config::default();
        if filtered {
            config = config.labels("deployment=selected");
        }
        let controller = ResourceController::new("GatewayClass", client, classes.clone(), scope(true), config)
            .with_epoch(cancel.clone(), tracker.clone());
        let (tx, rx) = mpsc::channel(16);
        let stream: NamespacedWatcher<GatewayClass> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        }));
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::Cluster, stream)]));
        let mut class: GatewayClass = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": "foreign", "uid": "uid-1", "resourceVersion": "1"},
            "spec": {"controllerName": "foreign/controller"}
        }))
        .unwrap();
        for event in [Event::Init, Event::InitApply(class.clone()), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| classes.is_ready()).await;
        assert!(classes.get("foreign").is_none());
        let queue = routes.workqueue();
        assert_eq!(queue.len(), 1, "never-published foreign class must wake raw dependents");
        let item = queue.dequeue().await.unwrap();
        queue.done(&item.key);
        class.metadata.resource_version = Some("2".into());
        tx.send(Ok(Event::Apply(class.clone()))).await.unwrap();
        eventually(|| handler.0.load(Ordering::SeqCst) == 2).await;
        assert!(queue.is_empty(), "status/RV-only echo must not requeue dependents");
        class.metadata.uid = Some("uid-2".into());
        tx.send(Ok(Event::Apply(class.clone()))).await.unwrap();
        eventually(|| handler.0.load(Ordering::SeqCst) == 3).await;
        assert_eq!(queue.len(), 1, "coalesced recreation must requeue dependents");
        let item = queue.dequeue().await.unwrap();
        queue.done(&item.key);
        // A full relist deletion must retain the committed view during staging.
        tx.send(Ok(Event::Init)).await.unwrap();
        tx.send(Ok(Event::InitDone)).await.unwrap();
        eventually(|| !queue.is_empty()).await;
        assert!(classes.get_raw_any("foreign").is_none());
        assert!(state.lock().requests.is_empty());
        cancel.cancel();
        task.await.unwrap().unwrap();
        runtime.reset();
        assert!(runtime.status_identity().is_none());
    }
}

#[tokio::test]
async fn unchanged_status_and_non_leaders_skip_bootstrap_and_worker_writes() {
    for unchanged in [false, true] {
        let status = if unchanged {
            json!({"parents": [parent("edgion.io/controller")]})
        } else {
            json!({"parents": []})
        };
        let initial = object(status);
        let (client, state) = api_client(initial.clone());
        let leadership = if unchanged {
            None
        } else {
            let (sender, _) = tokio::sync::watch::channel(false);
            Some(
                LeaderElection::new(
                    client.clone(),
                    LeaderElectionConfig {
                        lease_name: "controller".into(),
                        lease_namespace: "apps".into(),
                        pod_namespace: "apps".into(),
                        identity: "follower".into(),
                        lease_duration_secs: 15,
                        renew_deadline_secs: 10,
                        retry_period_secs: 2,
                    },
                    Arc::new(sender),
                )
                .handle(),
            )
        };
        let handler = Arc::new(ChangingMetadataHandler::default());
        let processor = Arc::new(ResourceProcessor::new(
            "HTTPRoute",
            100,
            handler.clone(),
            Arc::new(ControllerRuntime::new()),
        ));
        let raw = Arc::new(LocalRawStore::new());
        let route: HTTPRoute = serde_json::from_value(initial.clone()).unwrap();
        raw.upsert("apps/resource".into(), Arc::new(route.clone()));
        processor.set_raw_store(raw.clone());
        processor.on_init();
        assert!(
            run_init_apply_once(
                route,
                &processor,
                &client,
                &scope(false),
                &leadership,
                None,
                &None,
                "HTTPRoute",
                "test-init"
            )
            .await
        );
        processor.on_init_done();
        assert_eq!(handler.applied.load(Ordering::SeqCst), 1);
        assert!(state.lock().requests.is_empty());
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let worker = spawn_worker(
            processor.clone(),
            raw,
            "HTTPRoute",
            None,
            client,
            scope(false),
            leadership,
            None,
            cancel.clone(),
            &tracker,
        );
        processor.on_event_key("apps/resource".into()).await;
        eventually(|| handler.applied.load(Ordering::SeqCst) == 2).await;
        cancel.cancel();
        worker.await.unwrap();
        assert!(state.lock().requests.is_empty());
        assert_eq!(state.lock().object, initial);
        assert!(processor.get("apps/resource").is_some());
    }
}

#[tokio::test]
async fn bootstrap_and_worker_use_input_version_and_recover_on_later_watch_events() {
    let initial = object(json!({"parents": []}));
    let mut live = initial.clone();
    live["metadata"]["resourceVersion"] = json!("after-bootstrap-snapshot");
    let (client, state) = api_client(live.clone());
    let handler = Arc::new(ChangingMetadataHandler::default());
    let processor = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        100,
        handler.clone(),
        Arc::new(ControllerRuntime::new()),
    ));
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "HTTPRoute",
        client,
        processor.clone(),
        scope(false),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    let (tx, rx) = mpsc::channel(32);
    let stream: NamespacedWatcher<HTTPRoute> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event, rx))
    }));
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    for event in [
        Event::Init,
        Event::InitApply(serde_json::from_value(initial).unwrap()),
        Event::InitDone,
    ] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready()).await;
    assert_eq!(state.lock().requests.len(), 1);
    assert_eq!(state.lock().requests[0].1["metadata"]["resourceVersion"], "opaque-rv");
    assert_eq!(state.lock().object, live);
    assert!(
        processor.get("apps/resource").is_some(),
        "status conflict must not suppress config publication"
    );

    tx.send(Ok(Event::Apply(serde_json::from_value(live).unwrap())))
        .await
        .unwrap();
    eventually(|| state.lock().object["status"]["parents"].as_array().unwrap().len() == 1).await;
    assert_eq!(state.lock().requests.len(), 2);
    assert_eq!(
        state.lock().requests[1].1["metadata"]["resourceVersion"],
        "after-bootstrap-snapshot"
    );

    // A later runtime event also conflicts if the server advanced after its snapshot.
    let mut stale = state.lock().object.clone();
    stale["status"]["parents"] = json!([]);
    stale["metadata"]["resourceVersion"] = json!("runtime-old");
    let mut current = stale.clone();
    current["metadata"]["resourceVersion"] = json!("runtime-new");
    current["status"]["parents"] = json!([parent("foreign.example/controller")]);
    state.lock().object = current.clone();
    tx.send(Ok(Event::Apply(serde_json::from_value(stale).unwrap())))
        .await
        .unwrap();
    eventually(|| state.lock().requests.len() == 3).await;
    assert_eq!(state.lock().requests[2].1["metadata"]["resourceVersion"], "runtime-old");
    assert_eq!(state.lock().object, current);
    tx.send(Ok(Event::Apply(serde_json::from_value(current.clone()).unwrap())))
        .await
        .unwrap();
    eventually(|| state.lock().object["status"]["parents"].as_array().unwrap().len() == 2).await;
    assert_eq!(state.lock().requests.len(), 4);
    assert_eq!(state.lock().requests[3].1["metadata"]["resourceVersion"], "runtime-new");
    assert_eq!(
        state.lock().object["status"]["parents"][0],
        current["status"]["parents"][0]
    );

    // A successful watch echo with unchanged computed status produces no extra PATCH.
    let echo = state.lock().object.clone();
    tx.send(Ok(Event::Apply(serde_json::from_value(echo).unwrap())))
        .await
        .unwrap();
    eventually(|| handler.applied.load(Ordering::SeqCst) == 5).await;
    cancel.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(state.lock().requests.len(), 4);
}

#[tokio::test]
async fn gateway_beta_watcher_reports_unsupported_fields_and_recovers_on_selected_status_path() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
    let foreign = json!({"type": "Foreign", "status": "True", "reason": "OtherController",
        "message": "retained", "lastTransitionTime": "2026-01-01T00:00:00Z"});
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo", "uid": "uid-1",
                     "resourceVersion": "init-rv", "generation": 1},
        "spec": {"gatewayClassName": "edgion",
                 "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]},
        "status": {"conditions": [foreign.clone()]}
    });
    let (client, state) = api_client(source.clone());
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "example.com/edge".into(),
    });
    let processor = Arc::new(ResourceProcessor::new(
        "Gateway",
        8,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "Gateway",
        client,
        processor.clone(),
        scope(false),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone())
    .with_status_version("v1beta1");
    let (tx, rx) = mpsc::channel::<watcher::Result<Event<GatewayBeta>>>(32);
    let stream: NamespacedWatcher<Gateway> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event.map(canonical_gateway_event), rx))
    }));
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let beta = |value: Value| serde_json::from_value::<GatewayBeta>(value).unwrap();
    for event in [Event::Init, Event::InitApply(beta(source)), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready() && state.lock().requests.len() == 1).await;
    assert_eq!(
        state.lock().requests[0].0,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/gateways/edge/status"
    );
    assert_eq!(state.lock().requests[0].1["metadata"]["resourceVersion"], "init-rv");
    assert!(state.lock().object["status"]["conditions"]
        .as_array()
        .unwrap()
        .contains(&foreign));
    assert_eq!(
        processor
            .get("demo/edge")
            .unwrap()
            .spec
            .listeners
            .as_ref()
            .unwrap()
            .len(),
        1
    );

    let mut unsupported = state.lock().object.clone();
    unsupported["metadata"]["resourceVersion"] = json!("unsupported-rv");
    unsupported["metadata"]["generation"] = json!(2);
    unsupported["spec"]["allowedListeners"] = json!({"namespaces": {"from": "All"}});
    state.lock().object = unsupported.clone();
    tx.send(Ok(Event::Apply(beta(unsupported)))).await.unwrap();
    eventually(|| state.lock().requests.len() == 2).await;
    assert_eq!(
        state.lock().requests[1].1["metadata"]["resourceVersion"],
        "unsupported-rv"
    );
    assert!(state.lock().object["status"]["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition["type"] == "Accepted" && condition["status"] == "False"));
    assert!(state.lock().object["status"]["conditions"]
        .as_array()
        .unwrap()
        .contains(&foreign));
    let published = processor.get("demo/edge").unwrap();
    assert!(published.spec.listeners.is_none());
    assert!(published.spec.allowed_listeners.is_none());

    let mut corrected = state.lock().object.clone();
    corrected["metadata"]["resourceVersion"] = json!("corrected-rv");
    corrected["metadata"]["generation"] = json!(3);
    corrected["spec"] = json!({"gatewayClassName": "edgion",
        "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]});
    state.lock().object = corrected.clone();
    tx.send(Ok(Event::Apply(beta(corrected)))).await.unwrap();
    eventually(|| state.lock().requests.len() == 3).await;
    assert!(processor.get("demo/edge").unwrap().spec.listeners.is_some());
    assert!(state.lock().object["status"]["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True"));
    assert_eq!(tracker.len(), 2);
    cancel.cancel();
    task.await.unwrap().unwrap();
    runtime.reset();
}

#[tokio::test]
async fn http_route_beta_status_preserves_foreign_parent_and_resource_version() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::HttpRouteHandler;

    let foreign = json!({"parentRef": {"name": "other"}, "controllerName": "example.com/other",
        "conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted", "message": "foreign",
            "lastTransitionTime": "2026-01-01T00:00:00Z"}]});
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "demo", "uid": "uid-1", "resourceVersion": "init-rv", "generation": 1},
        "spec": {"parentRefs": [{"name": "edge"}], "rules": [{"backendRefs": [{"name": "api", "port": 80}]}]},
        "status": {"parents": [foreign.clone()]}
    });
    let (client, state) = api_client(source.clone());
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        8,
        Arc::new(HttpRouteHandler::new("example.com/edge".into())),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "HTTPRoute",
        client,
        processor.clone(),
        scope(false),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone())
    .with_status_version("v1beta1");
    let (tx, rx) = mpsc::channel::<watcher::Result<Event<HTTPRouteBeta>>>(32);
    let stream: NamespacedWatcher<HTTPRoute> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event.map(canonical_http_route_event), rx))
    }));
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let beta = |value: Value| serde_json::from_value::<HTTPRouteBeta>(value).unwrap();
    for event in [Event::Init, Event::InitApply(beta(source)), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready() && !state.lock().requests.is_empty()).await;
    assert_eq!(
        state.lock().requests[0].0,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/httproutes/route/status"
    );
    assert_eq!(state.lock().requests[0].1["metadata"]["resourceVersion"], "init-rv");
    assert!(state.lock().object["status"]["parents"]
        .as_array()
        .unwrap()
        .contains(&foreign));

    let mut changed = state.lock().object.clone();
    changed["metadata"]["resourceVersion"] = json!("runtime-rv");
    changed["metadata"]["generation"] = json!(2);
    changed["spec"]["rules"][0]["filters"] = json!([{"type": "ExternalAuth", "externalAuth": {
        "backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP", "http": {"path": "/check"}}}]);
    state.lock().object = changed.clone();
    tx.send(Ok(Event::Apply(beta(changed)))).await.unwrap();
    eventually(|| {
        state
            .lock()
            .requests
            .iter()
            .any(|(_, patch)| patch["metadata"]["resourceVersion"] == "runtime-rv")
    })
    .await;
    assert_eq!(
        state.lock().requests.last().unwrap().1["metadata"]["resourceVersion"],
        "runtime-rv"
    );
    assert!(state.lock().object["status"]["parents"]
        .as_array()
        .unwrap()
        .contains(&foreign));
    assert!(processor.get("demo/route").unwrap().spec.invalid_rule_indices.is_some());
    cancel.cancel();
    task.await.unwrap().unwrap();
    runtime.reset();
}

#[tokio::test]
async fn gateway_beta_init_list_isolates_unsupported_object_from_valid_peer() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
    use std::collections::HashMap;

    let source = |name: &str| {
        json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
            "metadata": {"name": name, "namespace": "demo", "uid": format!("uid-{name}"),
                         "resourceVersion": "init-rv", "generation": 1},
            "spec": {"gatewayClassName": "edgion",
                     "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
        })
    };
    let mut invalid = source("invalid");
    invalid["spec"]["infrastructure"] = json!({"labels": {"team": "web"}});
    let valid = source("valid");
    let objects = Arc::new(Mutex::new(HashMap::from([
        ("invalid".to_string(), invalid.clone()),
        ("valid".to_string(), valid.clone()),
    ])));
    let shared = objects.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let shared = shared.clone();
        async move {
            let (parts, body) = request.into_parts();
            let patch: Value = serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
            let name = parts.uri.path().split('/').nth_back(1).unwrap().to_string();
            let mut objects = shared.lock();
            let object = objects.get_mut(&name).unwrap();
            assert_eq!(
                patch["metadata"]["resourceVersion"],
                object["metadata"]["resourceVersion"]
            );
            json_patch::merge(object, &patch);
            Ok::<_, Infallible>(
                http::Response::builder()
                    .status(200)
                    .body(kube::client::Body::from(serde_json::to_vec(object).unwrap()))
                    .unwrap(),
            )
        }
    });
    let client = Client::new(transport, "default");
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "example.com/edge".into(),
    });
    let processor = Arc::new(ResourceProcessor::new(
        "Gateway",
        8,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "Gateway",
        client,
        processor.clone(),
        scope(false),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone())
    .with_status_version("v1beta1");
    let (tx, rx) = mpsc::channel::<watcher::Result<Event<GatewayBeta>>>(32);
    let stream: NamespacedWatcher<Gateway> = Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event.map(canonical_gateway_event), rx))
    }));
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    tx.send(Ok(Event::Init)).await.unwrap();
    tx.send(Ok(Event::InitApply(serde_json::from_value(invalid).unwrap())))
        .await
        .unwrap();
    tx.send(Ok(Event::InitApply(serde_json::from_value(valid).unwrap())))
        .await
        .unwrap();
    tx.send(Ok(Event::InitDone)).await.unwrap();
    eventually(|| {
        processor.is_ready() && processor.get("demo/valid").is_some() && processor.get("demo/invalid").is_some()
    })
    .await;
    assert!(processor.get("demo/invalid").unwrap().spec.listeners.is_none());
    assert_eq!(
        processor
            .get("demo/valid")
            .unwrap()
            .spec
            .listeners
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    {
        let objects = objects.lock();
        for (name, expected) in [("invalid", "False"), ("valid", "True")] {
            assert!(objects[name]["status"]["conditions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|condition| condition["type"] == "Accepted" && condition["status"] == expected));
        }
    }
    assert_eq!(tracker.len(), 2);
    cancel.cancel();
    task.await.unwrap().unwrap();
    runtime.reset();
}
