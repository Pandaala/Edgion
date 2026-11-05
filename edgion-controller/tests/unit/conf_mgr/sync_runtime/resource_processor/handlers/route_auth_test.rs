use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::observe_bound_reads;
use crate::conf_mgr::ControllerRuntime;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

fn source(namespace: &str) -> HTTPRoute {
    serde_json::from_value(
        json!({"metadata":{"name":"route","namespace":namespace},"spec":{"rules":[{"filters":[{
            "type":"ExternalAuth","resolvedExternalAuth":{"namespace":"forged"},"externalAuth":{
                "target":{"name":"auth","port":8080},"decision":{},
                "request":{"path":{"template":"/check/${secretRef:material/token:key}"}},
                "resolvedSecrets":{"material/token:key":"forged-value"}
            }
        }]}]}}),
    )
    .unwrap()
}
fn context() -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    (
        HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone()),
        runtime,
    )
}
fn grant(namespace: &str) -> edgion_resources::ReferenceGrant {
    serde_json::from_value(json!({"metadata":{"name":"allow","namespace":"material"},"spec":{
        "from":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":namespace}],
        "to":[{"group":"","kind":"Secret","name":"token"}]
    }}))
    .unwrap()
}
fn run(root: &HTTPRoute, source: &HTTPRoute, ctx: &HandlerContext, register: bool) -> AuthDecision {
    decisions(source, |field| resolve(root, field, ctx, register)).remove(0)
}

#[test]
fn root_recovery_never_substitutes_for_child_grant_authority() {
    let (ctx, runtime) = context();
    let root = source("root");
    let child = source("child");
    let secret: Secret = serde_json::from_value(
        json!({"metadata":{"name":"token","namespace":"material"},"stringData":{"key":"fresh-value"}}),
    )
    .unwrap();
    runtime
        .secret_store
        .replace_all(HashMap::from([("material/token".to_string(), secret)]));
    for allowed_namespace in [None, Some("root"), Some("child")] {
        runtime.ref_grant_store.remove("material/allow");
        if let Some(namespace) = allowed_namespace {
            runtime
                .ref_grant_store
                .upsert("material/allow".into(), Arc::new(grant(namespace)));
        }
        let (decision, reads) = observe_bound_reads(|| run(&root, &child, &ctx, true));
        assert_eq!(decision.errors.is_empty(), allowed_namespace == Some("child"));
        assert_eq!(
            reads.iter().filter(|read| **read == "secret").count(),
            usize::from(allowed_namespace == Some("child"))
        );
        assert!(ctx
            .secret_ref_manager()
            .get_dependencies(&owner(&root).key())
            .contains(&"material/token".to_string()));
        assert!(ctx
            .secret_ref_manager()
            .get_dependencies(&owner(&child).key())
            .is_empty());
        assert!(runtime
            .cross_ns_ref_mgr
            .get_dependencies(&owner(&root).key())
            .contains(&"material".to_string()));
        if allowed_namespace == Some("child") {
            assert_eq!(
                decision.secrets.unwrap().0.get("material/token:key").unwrap(),
                "fresh-value"
            );
        }
    }
}

#[test]
fn invalid_auth_targets_and_root_sources_have_zero_reads_and_zero_new_dependencies() {
    for invalid_root in [false, true] {
        let (ctx, runtime) = context();
        let mut root = source("root");
        let mut child = source("child");
        if invalid_root {
            root.metadata.namespace = Some("".into());
        } else {
            let HTTPRouteExternalAuth::Edgion(config) = child.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()
                [0]
            .external_auth
            .as_mut()
            .unwrap() else {
                unreachable!()
            };
            config.conn.target.service.group = Some("invalid.example".into());
        }
        let (decision, reads) = observe_bound_reads(|| run(&root, &child, &ctx, true));
        assert!(!decision.errors.is_empty());
        assert!(reads.is_empty());
        assert!(ctx
            .secret_ref_manager()
            .get_dependencies(&owner(&root).key())
            .is_empty());
        assert!(runtime
            .cross_ns_ref_mgr
            .get_dependencies(&owner(&root).key())
            .is_empty());
    }
}

#[test]
fn readonly_auth_resolution_and_default_callback_do_not_retain_injected_values() {
    let (ctx, runtime) = context();
    let root = source("root");
    let mut child = source("child");
    runtime
        .ref_grant_store
        .upsert("material/allow".into(), Arc::new(grant("child")));
    let decision = run(&root, &child, &ctx, false);
    assert!(!decision.errors.is_empty());
    assert!(ctx
        .secret_ref_manager()
        .get_dependencies(&owner(&root).key())
        .is_empty());
    assert!(runtime
        .cross_ns_ref_mgr
        .get_dependencies(&owner(&root).key())
        .is_empty());
    decision.apply(&mut child.spec.rules.as_mut().unwrap()[0]);
    let filter = &child.spec.rules.as_ref().unwrap()[0].filters.as_ref().unwrap()[0];
    assert!(filter
        .external_auth
        .as_ref()
        .unwrap()
        .edgion_config()
        .unwrap()
        .resolved_secrets
        .is_none());
    assert!(filter.resolved_external_auth.as_ref().unwrap().error().is_some());
}

#[test]
fn child_auth_target_in_root_namespace_still_requires_child_grant() {
    let (ctx, runtime) = context();
    let root = source("material");
    let child = source("child");
    runtime.secret_store.replace_all(HashMap::from([(
        "material/token".into(),
        serde_json::from_value(
            json!({"metadata":{"name":"token","namespace":"material"},"stringData":{"key":"fresh"}}),
        )
        .unwrap(),
    )]));
    for allowed in ["material", "child"] {
        runtime
            .ref_grant_store
            .upsert("material/allow".into(), Arc::new(grant(allowed)));
        let (decision, reads) = observe_bound_reads(|| run(&root, &child, &ctx, true));
        assert_eq!(decision.errors.is_empty(), allowed == "child");
        assert_eq!(
            reads.iter().filter(|read| **read == "secret").count(),
            usize::from(allowed == "child")
        );
        assert_eq!(
            runtime.secret_ref_manager.get_refs("material/token"),
            vec![owner(&root)]
        );
        assert!(runtime
            .cross_ns_ref_mgr
            .get_dependencies(&owner(&root).key())
            .contains(&"material".into()));
    }
}

#[test]
fn route_auth_configmap_ca_acquisition_preserves_order_provenance_and_recovery() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    use k8s_openapi::api::core::v1::ConfigMap;
    let (ctx, runtime) = context();
    let root = source("root");
    let child: HTTPRoute = serde_json::from_value(json!({"metadata":{"name":"route","namespace":"child"},"spec":{"rules":[{"filters":[{"type":"ExternalAuth","externalAuth":{
        "target":{"name":"auth","port":443},"decision":{},"tls":{"enabled":true,"validation":{"caCertificateRefs":[
            {"kind":"ConfigMap","name":"first","namespace":"material"},
            {"kind":"Secret","name":"second","namespace":"material"},
            {"kind":"ConfigMap","name":"third","namespace":"material"}]}}
    }}]}]}})).unwrap();
    let permission: edgion_resources::ReferenceGrant =
        serde_json::from_value(json!({"metadata":{"name":"allow","namespace":"material"},"spec":{
        "from":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":"child"}],
        "to":[{"group":"","kind":"ConfigMap"},{"group":"","kind":"Secret"}]}}))
        .unwrap();
    runtime
        .ref_grant_store
        .upsert("material/allow".into(), Arc::new(permission));
    let (missing, reads) = observe_bound_reads(|| run(&root, &child, &ctx, true));
    assert_eq!(missing.errors.len(), 3);
    assert!(missing.ca.is_none());
    assert_eq!(reads.iter().filter(|read| **read == "configmap").count(), 2);
    for key in [
        "ConfigMap/material/first",
        "material/second",
        "ConfigMap/material/third",
    ] {
        assert_eq!(runtime.secret_ref_manager.get_refs(key), vec![owner(&root)]);
    }
    runtime.configmap_store.replace_all(
        [("first", "first-ca"), ("third", "third-ca")]
            .into_iter()
            .map(|(name, pem)| {
                let cm: ConfigMap = serde_json::from_value(
                    json!({"metadata":{"name":name,"namespace":"material"},"data":{"ca.crt":pem}}),
                )
                .unwrap();
                (format!("material/{name}"), cm)
            })
            .collect(),
    );
    runtime.secret_store.replace_all(HashMap::from([(
        "material/second".into(),
        serde_json::from_value(
            json!({"metadata":{"name":"second","namespace":"material"},"data":{"ca.crt":"c2Vjb25kLWNh"}}),
        )
        .unwrap(),
    )]));
    let recovered = run(&root, &child, &ctx, true);
    assert!(recovered.errors.is_empty());
    assert!(recovered.stamp.as_ref().unwrap().error().is_none());
    let material = recovered.ca.unwrap();
    assert_eq!(
        material
            .iter()
            .map(|ca| ca.metadata.name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    for (index, pem) in [(0, b"first-ca".as_slice()), (2, b"third-ca".as_slice())] {
        assert_eq!(
            material[index]
                .metadata
                .annotations
                .as_ref()
                .unwrap()
                .get(CONTROLLER_CA_SOURCE)
                .map(String::as_str),
            Some("ConfigMap")
        );
        assert_eq!(material[index].data.as_ref().unwrap()["ca.crt"].0, pem);
    }
    assert!(material[1]
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(CONTROLLER_CA_SOURCE))
        .is_none());
    runtime.ref_grant_store.remove("material/allow");
    let (denied, reads) = observe_bound_reads(|| run(&root, &child, &ctx, true));
    assert_eq!(denied.errors.len(), 3);
    assert!(denied.ca.is_none());
    assert!(!reads.iter().any(|read| matches!(*read, "secret" | "configmap")));
    assert!(denied.stamp.unwrap().error().is_some());
    assert_eq!(
        runtime.secret_ref_manager.get_refs("ConfigMap/material/first"),
        vec![owner(&root)]
    );
}
