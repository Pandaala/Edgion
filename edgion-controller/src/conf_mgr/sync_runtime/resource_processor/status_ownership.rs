//! Kubernetes status writer ownership, separate from local serving diagnostics.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::edgion_acme::AcmeChallenge;
use edgion_resources::{EdgionAcme, GRPCRoute, Gateway, GatewayClass, HTTPRoute};
use serde_json::{json, Value};

use crate::conf_mgr::ControllerRuntime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KubernetesStatusIdentity {
    pub controller_name: String,
}

/// Ownership requires the referenced class incarnation, not its conditions.
pub fn gateway_has_class_owner(gateway: &Gateway, controller_name: &str, runtime: &ControllerRuntime) -> bool {
    gateway_owned_class_uid(gateway, controller_name, runtime).is_some()
}

fn gateway_owned_class_uid(gateway: &Gateway, controller_name: &str, runtime: &ControllerRuntime) -> Option<String> {
    let name = &gateway.spec.gateway_class_name;
    let processor = runtime.processor_registry.get("GatewayClass")?;
    let raw = processor
        .get_raw_any(name)
        .and_then(|value| value.downcast::<GatewayClass>().ok())?;
    let published = processor
        .get_cached_any(name)
        .and_then(|value| value.downcast::<GatewayClass>().ok())?;
    let uid = raw.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    (raw.spec.controller_name == controller_name
        && published.spec.controller_name == controller_name
        && published.metadata.uid.as_deref() == Some(uid))
    .then(|| uid.to_owned())
}

pub fn gateway_is_owned(gateway: &Gateway, runtime: &ControllerRuntime) -> bool {
    runtime
        .status_identity()
        .is_none_or(|identity| gateway_has_class_owner(gateway, &identity.controller_name, runtime))
}

/// A serving attachment requires this deployment's current published incarnation.
/// Raw objects remain available separately for diagnostics, never serving authority.
pub fn serving_gateway(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    serving_gateway_with_class_uid(namespace, name, runtime).map(|(gateway, _)| gateway)
}

fn serving_gateway_with_class_uid(
    namespace: &str,
    name: &str,
    runtime: &ControllerRuntime,
) -> Option<(Arc<Gateway>, Option<String>)> {
    let processor = runtime.processor_registry.get("Gateway")?;
    let key = format!("{namespace}/{name}");
    let published = processor.get_cached_any(&key)?.downcast::<Gateway>().ok()?;
    let Some(identity) = runtime.status_identity() else {
        return Some((published, None));
    };
    let raw = processor.get_raw_any(&key)?.downcast::<Gateway>().ok()?;
    let uid = raw.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    let class_uid = gateway_owned_class_uid(&raw, &identity.controller_name, runtime)?;
    (published.metadata.uid.as_deref() == Some(uid) && published.spec.gateway_class_name == raw.spec.gateway_class_name)
        .then_some((published, Some(class_uid)))
}

pub fn gateway_publication_identity(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<(String, String)> {
    let namespace = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let name = gateway.metadata.name.as_deref()?;
    let (published, class_uid) = serving_gateway_with_class_uid(namespace, name, runtime)?;
    let uid = published.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    if gateway.metadata.uid.as_deref() != Some(uid) {
        return None;
    }
    // Use the exact class incarnation proved with the returned Gateway. A new
    // raw lookup here could stamp a not-yet-published class and suppress recovery.
    Some((uid.into(), class_uid?))
}

fn raw_gateway(parent: &ParentReference, namespace: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    let identity = parent.canonical_identity(Some(namespace));
    if !identity.is_gateway() {
        return None;
    }
    runtime
        .processor_registry
        .get("Gateway")?
        .get_raw_any(&format!("{}/{}", identity.namespace, identity.name))?
        .downcast::<Gateway>()
        .ok()
}

pub fn parent_is_owned(parent: &ParentReference, namespace: &str, runtime: &ControllerRuntime) -> bool {
    runtime.status_identity().is_none()
        || raw_gateway(parent, namespace, runtime).is_some_and(|gateway| gateway_is_owned(&gateway, runtime))
}

pub fn acme_is_owned(acme: &EdgionAcme, runtime: &ControllerRuntime) -> bool {
    let AcmeChallenge::Http01 { gateway_ref } = &acme.spec.challenge;
    parent_is_owned(
        gateway_ref,
        acme.metadata.namespace.as_deref().unwrap_or("default"),
        runtime,
    )
}

type ParentKey = (String, String, String, String, Option<String>, Option<i32>);

fn parent_key(parent: &ParentReference, namespace: &str) -> ParentKey {
    let identity = parent.canonical_identity(Some(namespace));
    (
        identity.group.into(),
        identity.kind.into(),
        identity.namespace.into(),
        identity.name.into(),
        identity.section_name.map(str::to_owned),
        identity.port,
    )
}

fn is_reverse_lock(parent: &ParentReference, kind: &str, namespace: &str) -> bool {
    let identity = parent.canonical_identity(Some(namespace));
    matches!(kind, "HTTPRoute" | "GRPCRoute") && identity.group == "gateway.networking.k8s.io" && identity.kind == kind
}

/// Pick from the complete explicit spec BEFORE looking up the selected dependency.
/// Missing or unsupported first choices never fall back to another visible branch.
fn attachment_is_owned(parent: &ParentReference, kind: &str, namespace: &str, runtime: &ControllerRuntime) -> bool {
    let mut selected = parent.clone();
    let mut namespace = namespace.to_owned();
    let mut visited = HashSet::new();
    for depth in 0..=32 {
        let identity = selected.canonical_identity(Some(&namespace));
        if identity.is_gateway() {
            return parent_is_owned(&selected, &namespace, runtime);
        }
        if depth == 32 || !is_reverse_lock(&selected, kind, &namespace) {
            return false;
        }
        let key = format!("{}/{}", identity.namespace, identity.name);
        if !visited.insert(key.clone()) {
            return false;
        }
        let Some(processor) = runtime.processor_registry.get(kind) else {
            return false;
        };
        let Some(raw) = processor.get_raw_any(&key) else {
            return false;
        };
        let refs = match kind {
            "HTTPRoute" => raw
                .downcast::<HTTPRoute>()
                .ok()
                .and_then(|route| route.spec.parent_refs.clone()),
            "GRPCRoute" => raw
                .downcast::<GRPCRoute>()
                .ok()
                .and_then(|route| route.spec.parent_refs.clone()),
            _ => None,
        };
        namespace = identity.namespace.to_owned();
        let Some(first) = refs.and_then(|refs| refs.into_iter().min_by_key(|parent| parent_key(parent, &namespace)))
        else {
            return false;
        };
        selected = first;
    }
    false
}

/// Return distinct local and persisted native status snapshots.
/// Unknown ownership preserves historical API diagnostics, never serving authority.
pub fn native_status_projection(
    kind: &str,
    namespace: &str,
    raw_resource: &Value,
    observed_status: &Value,
    computed_status: &Value,
    runtime: &ControllerRuntime,
) -> Option<(Value, Value)> {
    let owner = runtime.status_identity()?;
    let (list, reference) = match kind {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" | "EdgionTls" => ("parents", "parentRef"),
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => ("ancestors", "ancestorRef"),
        _ => return None,
    };
    let raw_refs: Vec<ParentReference> = raw_resource
        .pointer("/spec/parentRefs")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let own_entry =
        |entry: &Value| entry.get("controllerName").and_then(Value::as_str) == Some(owner.controller_name.as_str());
    let parse_ref = |entry: &Value| {
        entry
            .get(reference)
            .and_then(|value| serde_json::from_value::<ParentReference>(value.clone()).ok())
    };
    let mut local_entries = Vec::new();
    let mut own_entries = Vec::new();
    let mut seen = HashSet::new();
    let mut ownership = HashMap::new();
    for entry in computed_status
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !own_entry(entry) {
            continue;
        }
        let Some(parent) = parse_ref(entry) else {
            continue;
        };
        let owned = *ownership
            .entry(parent_key(&parent, namespace))
            .or_insert_with(|| attachment_is_owned(&parent, kind, namespace, runtime));
        if owned || is_reverse_lock(&parent, kind, namespace) {
            local_entries.push(entry.clone());
        }
        if owned && seen.insert(parent_key(&parent, namespace)) {
            own_entries.push(entry.clone());
        }
    }
    let mut persisted_entries = Vec::new();
    for entry in observed_status
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if own_entry(entry) {
            if let Some(parent) = parse_ref(entry) {
                let removed_from_spec = list == "parents"
                    && !raw_refs
                        .iter()
                        .any(|current| parent_key(current, namespace) == parent_key(&parent, namespace));
                let owned = *ownership
                    .entry(parent_key(&parent, namespace))
                    .or_insert_with(|| attachment_is_owned(&parent, kind, namespace, runtime));
                if removed_from_spec || owned {
                    continue;
                }
            }
        }
        persisted_entries.push(entry.clone());
    }
    persisted_entries.extend(own_entries);
    let mut local = computed_status.as_object().cloned().unwrap_or_default();
    local.insert(list.into(), Value::Array(local_entries));
    let mut persisted = observed_status.as_object().cloned().unwrap_or_default();
    persisted.insert(list.into(), Value::Array(persisted_entries));
    Some((Value::Object(local), Value::Object(persisted)))
}

/// Source-commit dependency fingerprints exclude status/resourceVersion churn.
pub fn ownership_dependency_signature(kind: &str, resource: &Value) -> Option<Value> {
    let uid = resource.pointer("/metadata/uid");
    match kind {
        "GatewayClass" => Some(json!([uid, resource.pointer("/spec/controllerName")])),
        "Gateway" => Some(json!([uid, resource.pointer("/spec/gatewayClassName")])),
        "HTTPRoute" | "GRPCRoute" => {
            let namespace = resource
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .unwrap_or("default");
            let refs: Vec<ParentReference> = resource
                .pointer("/spec/parentRefs")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default();
            let mut keys: Vec<_> = refs.iter().map(|parent| parent_key(parent, namespace)).collect();
            keys.sort_unstable();
            keys.dedup();
            Some(json!([uid, keys]))
        }
        _ => None,
    }
}

pub fn ownership_dependency_kinds(kind: &str) -> &'static [&'static str] {
    match kind {
        "GatewayClass" => &[
            "Gateway",
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
            "BackendTLSPolicy",
            "EdgionBackendTrafficPolicy",
            "EdgionAcme",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "EdgionBackend",
            "LinkSys",
        ],
        "Gateway" => &[
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
            "BackendTLSPolicy",
            "EdgionBackendTrafficPolicy",
            "EdgionAcme",
        ],
        "HTTPRoute" => &["HTTPRoute"],
        "GRPCRoute" => &["GRPCRoute"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::{
        LocalRawStore, ProcessorHandler, RawStore, ResourceProcessor,
    };

    struct Passthrough;
    #[async_trait::async_trait]
    impl ProcessorHandler<GatewayClass> for Passthrough {}
    #[async_trait::async_trait]
    impl ProcessorHandler<Gateway> for Passthrough {}
    #[async_trait::async_trait]
    impl ProcessorHandler<HTTPRoute> for Passthrough {}

    struct Fixture {
        runtime: Arc<ControllerRuntime>,
        classes: Arc<ResourceProcessor<GatewayClass>>,
        class_raw: Arc<LocalRawStore<GatewayClass>>,
        gateway_processor: Arc<ResourceProcessor<Gateway>>,
        route_processor: Arc<ResourceProcessor<HTTPRoute>>,
        gateways: Arc<LocalRawStore<Gateway>>,
        routes: Arc<LocalRawStore<HTTPRoute>>,
    }

    impl Fixture {
        async fn new(selected: &str) -> Self {
            let runtime = Arc::new(ControllerRuntime::default());
            runtime.set_status_identity(KubernetesStatusIdentity {
                controller_name: "example.com/controller".into(),
            });
            let classes = Arc::new(ResourceProcessor::new(
                "GatewayClass",
                16,
                Arc::new(Passthrough),
                runtime.clone(),
            ));
            let class_raw = Arc::new(LocalRawStore::new());
            classes.set_raw_store(class_raw.clone());
            runtime.processor_registry.register(classes.clone());
            for name in ["a", "b"] {
                let class: GatewayClass = serde_json::from_value(json!({"metadata":{"name":name,"uid":name},
                    "spec":{"controllerName":if name == selected {"example.com/controller"} else {"example.com/foreign"}}}))
                .unwrap();
                class_raw.upsert(name.into(), Arc::new(class.clone()));
                classes.on_init_apply(class, None).await;
            }
            let gateway_processor = Arc::new(ResourceProcessor::new(
                "Gateway",
                16,
                Arc::new(Passthrough),
                runtime.clone(),
            ));
            let gateways = Arc::new(LocalRawStore::new());
            gateway_processor.set_raw_store(gateways.clone());
            runtime.processor_registry.register(gateway_processor.clone());
            for name in ["a", "b"] {
                let gateway: Gateway =
                    serde_json::from_value(json!({"metadata":{"namespace":"apps","name":name,"uid":name},
                    "spec":{"gatewayClassName":name,"listeners":[{"name":"http","port":80,"protocol":"HTTP"}]}}))
                    .unwrap();
                gateways.upsert(format!("apps/{name}"), Arc::new(gateway.clone()));
                gateway_processor.on_init_apply(gateway, None).await;
            }
            let route_processor = Arc::new(ResourceProcessor::new(
                "HTTPRoute",
                16,
                Arc::new(Passthrough),
                runtime.clone(),
            ));
            let routes = Arc::new(LocalRawStore::new());
            route_processor.set_raw_store(routes.clone());
            runtime.processor_registry.register(route_processor.clone());
            Self {
                runtime,
                classes,
                class_raw,
                gateway_processor,
                route_processor,
                gateways,
                routes,
            }
        }

        fn route(&self, name: &str, parents: Value) {
            let route = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":name,"uid":name},
                "spec":{"parentRefs":parents}}))
            .unwrap();
            self.routes.upsert(format!("apps/{name}"), Arc::new(route));
        }
    }

    fn parent(name: &str) -> ParentReference {
        serde_json::from_value(json!({"name":name})).unwrap()
    }

    fn entry(name: &str, message: &str) -> Value {
        json!({"parentRef":{"name":name},"controllerName":"example.com/controller",
            "conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":message}]})
    }

    #[tokio::test]
    async fn one_controller_owns_parents_across_both_matching_classes() {
        let fixture = Fixture::new("a").await;
        let mut second = (*fixture.class_raw.get("b").unwrap()).clone();
        second.spec.controller_name = "example.com/controller".into();
        fixture.class_raw.upsert("b".into(), Arc::new(second.clone()));
        fixture.classes.on_init_apply(second, None).await;
        let resource = json!({"spec":{"parentRefs":[{"name":"a"},{"name":"b"}]}});
        let computed = json!({"parents":[entry("a", "local a"),entry("b", "local b")]});
        let (local, persisted) = native_status_projection(
            "HTTPRoute",
            "apps",
            &resource,
            &json!({"parents":[]}),
            &computed,
            &fixture.runtime,
        )
        .unwrap();
        assert_eq!(local, computed);
        assert_eq!(persisted, computed);
        assert!(serving_gateway("apps", "a", &fixture.runtime).is_some());
        assert!(serving_gateway("apps", "b", &fixture.runtime).is_some());
    }

    #[tokio::test]
    async fn missing_owner_is_historical_only_and_removed_spec_cleans_our_controller() {
        let fixture = Fixture::new("a").await;
        fixture.gateways.remove("apps/a");
        let resource = json!({"spec":{"parentRefs":[{"name":"a"}]}});
        let mut foreign = entry("a", "foreign");
        foreign["controllerName"] = json!("other.example/controller");
        let observed = json!({"parents":[entry("a", "historical"),foreign.clone()]});
        let computed = json!({"parents":[entry("a", "missing")]});
        let (local, persisted) =
            native_status_projection("HTTPRoute", "apps", &resource, &observed, &computed, &fixture.runtime).unwrap();
        assert_eq!(local["parents"], json!([]));
        assert_eq!(persisted, observed);
        let (_, cleared) = native_status_projection(
            "HTTPRoute",
            "apps",
            &json!({"spec":{}}),
            &observed,
            &json!({"parents":[]}),
            &fixture.runtime,
        )
        .unwrap();
        assert_eq!(cleared["parents"], json!([foreign]));
    }

    #[tokio::test]
    async fn reverse_writer_uses_first_explicit_choice_without_visible_fallback() {
        let fixture = Fixture::new("b").await;
        fixture.route("root", json!([{"name":"b"},{"name":"a"}]));
        let lock: ParentReference = serde_json::from_value(json!({"name":"root","kind":"HTTPRoute"})).unwrap();
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.gateways.remove("apps/a");
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.route(
            "root",
            json!([{"group":"","kind":"Unsupported","name":"first"},{"name":"b"}]),
        );
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
    }

    #[tokio::test]
    async fn three_level_reverse_chain_missing_create_transfer_delete_and_cycle() {
        let fixture = Fixture::new("a").await;
        let lock: ParentReference = serde_json::from_value(json!({"name":"leaf","kind":"HTTPRoute"})).unwrap();
        fixture.route("leaf", json!([{"name":"middle","kind":"HTTPRoute"}]));
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.route("middle", json!([{"name":"root","kind":"HTTPRoute"}]));
        fixture.route("root", json!([{"name":"a"}]));
        assert!(attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.route("root", json!([{"name":"b"}]));
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.routes.remove("apps/root");
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.route("root", json!([{"name":"leaf","kind":"HTTPRoute"}]));
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
    }

    #[tokio::test]
    async fn reverse_nonwriter_keeps_local_diagnostics_without_overwriting_api_entry() {
        let fixture = Fixture::new("b").await;
        fixture.route("root", json!([{"name":"a"},{"name":"b"}]));
        let mut raw = entry("root", "persisted owner");
        raw["parentRef"]["kind"] = json!("HTTPRoute");
        let mut computed = raw.clone();
        computed["conditions"][0]["message"] = json!("local diagnosis");
        let resource = json!({"spec":{"parentRefs":[{"name":"root","kind":"HTTPRoute"}]}});
        let (local, persisted) = native_status_projection(
            "HTTPRoute",
            "apps",
            &resource,
            &json!({"parents":[raw.clone()]}),
            &json!({"parents":[computed.clone()]}),
            &fixture.runtime,
        )
        .unwrap();
        assert_eq!(local["parents"], json!([computed]));
        assert_eq!(persisted["parents"], json!([raw]));
    }

    #[tokio::test]
    async fn serving_selector_rejects_foreign_class_and_stale_published_incarnation() {
        let fixture = Fixture::new("a").await;
        assert!(serving_gateway("apps", "a", &fixture.runtime).is_some());
        assert!(serving_gateway("apps", "b", &fixture.runtime).is_none());
        let mut recreated = (*fixture.gateways.get("apps/a").unwrap()).clone();
        recreated.metadata.uid = Some("new-uid".into());
        fixture.gateways.upsert("apps/a".into(), Arc::new(recreated));
        assert!(serving_gateway("apps", "a", &fixture.runtime).is_none());
        assert!(parent_is_owned(&parent("a"), "apps", &fixture.runtime));
    }

    #[tokio::test]
    async fn publication_recovers_route_that_ran_between_raw_and_cache_incarnations() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::route_utils::resolve_listener_keys;
        use crate::conf_mgr::sync_runtime::resource_processor::{GatewayHandler, HandlerContext};
        use crate::conf_mgr::sync_runtime::workqueue::Workqueue;
        use std::sync::atomic::Ordering;
        use std::time::Duration;
        async fn expect_replay(queue: &Workqueue, scheduled: u64) {
            // Cross-resource requeues first enter the delay heap, not `len()`.
            assert_eq!(queue.metrics().delayed_total.load(Ordering::Relaxed), scheduled);
            let item = tokio::time::timeout(Duration::from_secs(2), queue.dequeue())
                .await
                .expect("publication must schedule recovery")
                .expect("queue must remain open");
            assert_eq!(item.key, "apps/route");
            queue.done(&item.key);
        }
        let fixture = Fixture::new("a").await;
        fixture.route("route", json!([{"name":"a"}]));
        let ctx = HandlerContext::new(None, None, Default::default(), 3, 16, fixture.runtime.clone());
        let handler = GatewayHandler::new(None, None);
        let first = (*fixture.gateways.get("apps/a").unwrap()).clone();
        handler.after_publish(&first, &ctx).await;
        let queue = fixture.route_processor.workqueue();
        expect_replay(&queue, 1).await;
        assert_eq!(
            resolve_listener_keys(&[parent("a")], "apps", Some("apps"), &fixture.runtime).len(),
            1
        );

        // Pause publication after committing raw UID2; the dependent runs first.
        let mut recreated = first.clone();
        recreated.metadata.uid = Some("gateway-uid2".into());
        fixture.gateways.upsert("apps/a".into(), Arc::new(recreated.clone()));
        assert!(resolve_listener_keys(&[parent("a")], "apps", Some("apps"), &fixture.runtime).is_empty());
        fixture.gateway_processor.save(recreated.clone(), None);
        handler.after_publish(&recreated, &ctx).await;
        expect_replay(&queue, 2).await;
        assert_eq!(
            resolve_listener_keys(&[parent("a")], "apps", Some("apps"), &fixture.runtime).len(),
            1
        );
        for _ in 0..6 {
            handler.after_publish(&recreated, &ctx).await;
            assert_eq!(
                queue.metrics().delayed_total.load(Ordering::Relaxed),
                2,
                "unchanged publication must not schedule another delayed replay"
            );
            assert!(queue.is_empty());
        }

        // A class incarnation changes while Gateway UID/listeners stay identical.
        let mut class = (*fixture.class_raw.get("a").unwrap()).clone();
        class.metadata.uid = Some("class-uid2".into());
        fixture.class_raw.upsert("a".into(), Arc::new(class.clone()));
        assert!(resolve_listener_keys(&[parent("a")], "apps", Some("apps"), &fixture.runtime).is_empty());
        assert_eq!(gateway_publication_identity(&recreated, &fixture.runtime), None);
        handler.after_publish(&recreated, &ctx).await;
        expect_replay(&queue, 3).await;
        fixture.classes.save(class, None);
        handler.after_publish(&recreated, &ctx).await;
        expect_replay(&queue, 4).await;
        assert_eq!(
            resolve_listener_keys(&[parent("a")], "apps", Some("apps"), &fixture.runtime).len(),
            1
        );
        fixture.class_raw.remove("a");
        handler.after_publish(&recreated, &ctx).await;
        expect_replay(&queue, 5).await;
    }

    #[tokio::test]
    async fn every_native_status_family_uses_the_same_class_boundary() {
        let fixture = Fixture::new("a").await;
        for kind in [
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
            "BackendTLSPolicy",
            "EdgionBackendTrafficPolicy",
        ] {
            let ancestors = kind.ends_with("Policy");
            let list = if ancestors { "ancestors" } else { "parents" };
            let reference = if ancestors { "ancestorRef" } else { "parentRef" };
            let convert = |name: &str, message: &str| {
                let mut value = entry(name, message);
                if ancestors {
                    let parent = value.as_object_mut().unwrap().remove("parentRef").unwrap();
                    value[reference] = parent;
                }
                value
            };
            let mut observed = json!({});
            observed[list] = json!([convert("a", "previous"), convert("b", "foreign")]);
            let mut computed = json!({});
            computed[list] = json!([convert("a", "local"), convert("b", "must not publish")]);
            let resource = json!({"spec":{"parentRefs":[{"name":"a"},{"name":"b"}]}});
            let (local, persisted) =
                native_status_projection(kind, "apps", &resource, &observed, &computed, &fixture.runtime).unwrap();
            assert_eq!(local[list], json!([convert("a", "local")]), "{kind}");
            assert_eq!(
                persisted[list],
                json!([convert("b", "foreign"), convert("a", "local")]),
                "{kind}"
            );
            let (_, cleared) =
                native_status_projection(kind, "apps", &resource, &observed, &json!({}), &fixture.runtime).unwrap();
            assert_eq!(cleared[list], json!([convert("b", "foreign")]), "{kind}");
        }
    }

    #[tokio::test]
    async fn real_http_handler_pipeline_keeps_full_status_and_projects_current_status() {
        use crate::conf_mgr::sync_runtime::resource_processor::{HttpRouteHandler, WorkItemResult};
        let fixture = Fixture::new("a").await;
        let processor = Arc::new(ResourceProcessor::new(
            "HTTPRoute",
            16,
            Arc::new(HttpRouteHandler::new("example.com/controller".into())),
            fixture.runtime.clone(),
        ));
        processor.set_raw_store(fixture.routes.clone());
        fixture.runtime.processor_registry.register(processor.clone());
        let mut foreign = entry("b", "foreign status must stay byte-identical");
        foreign["conditions"][0]["lastTransitionTime"] = json!("2026-09-21T00:00:00Z");
        foreign["conditions"][0]["observedGeneration"] = json!(1);
        let mut route: HTTPRoute = serde_json::from_value(json!({
            "metadata":{"namespace":"apps","name":"route","uid":"route","generation":1,"resourceVersion":"1"},
            "spec":{"parentRefs":[{"name":"a"},{"name":"b"}]},"status":{"parents":[foreign.clone()]}
        }))
        .unwrap();
        for turn in 0..3 {
            fixture.routes.upsert("apps/route".into(), Arc::new(route.clone()));
            let WorkItemResult::Processed {
                obj,
                status_changed,
                persisted_status: Some(persisted),
            } = processor.on_init_apply(route.clone(), None).await
            else {
                panic!("HTTPRoute must be processed");
            };
            assert_eq!(status_changed, turn == 0);
            let parents = obj.spec.current_status.as_ref().unwrap().parents.as_slice();
            assert_eq!(parents.len(), 1);
            assert_eq!(parents[0].parent_ref.name, "a");
            assert_eq!(obj.spec.resolved_listeners.as_ref().unwrap().len(), 1);
            assert_eq!(
                processor
                    .get("apps/route")
                    .unwrap()
                    .spec
                    .current_status
                    .as_ref()
                    .unwrap()
                    .parents
                    .len(),
                1
            );
            let entries = persisted["parents"].as_array().unwrap();
            assert!(entries.contains(&foreign));
            assert_eq!(entries.len(), 2);
            assert_eq!(serde_json::to_value(&obj.status).unwrap(), persisted);
            route.status = Some(serde_json::from_value(persisted).unwrap());
        }
    }

    #[tokio::test]
    async fn reverse_chain_has_exactly_32_route_dereferences() {
        let fixture = Fixture::new("a").await;
        for index in 0..32 {
            fixture.route(
                &format!("r{index}"),
                if index == 31 {
                    json!([{"name":"a"}])
                } else {
                    json!([{"name":format!("r{}",index+1),"kind":"HTTPRoute"}])
                },
            );
        }
        let lock = serde_json::from_value(json!({"name":"r0","kind":"HTTPRoute"})).unwrap();
        assert!(attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
        fixture.route("r31", json!([{"name":"r32","kind":"HTTPRoute"}]));
        fixture.route("r32", json!([{"name":"a"}]));
        assert!(!attachment_is_owned(&lock, "HTTPRoute", "apps", &fixture.runtime));
    }

    #[test]
    fn dependency_signature_ignores_echo_order_and_explicit_defaults() {
        let raw = json!({"metadata":{"namespace":"apps","uid":"one","resourceVersion":"1"},
            "spec":{"parentRefs":[{"name":"b"},{"name":"a"}]}});
        let echo = json!({"metadata":{"namespace":"apps","uid":"one","resourceVersion":"2"},
            "status":{"parents":[]},"spec":{"parentRefs":[{"name":"a","namespace":"apps","kind":"Gateway","group":"gateway.networking.k8s.io"},{"name":"b"}]}});
        assert_eq!(
            ownership_dependency_signature("HTTPRoute", &raw),
            ownership_dependency_signature("HTTPRoute", &echo)
        );
        let mut recreated = echo.clone();
        recreated["metadata"]["uid"] = json!("two");
        assert_ne!(
            ownership_dependency_signature("HTTPRoute", &raw),
            ownership_dependency_signature("HTTPRoute", &recreated)
        );
    }
}
