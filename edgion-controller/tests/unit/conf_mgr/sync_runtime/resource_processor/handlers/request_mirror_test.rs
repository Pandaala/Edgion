use super::super::{EdgionPluginsHandler, HttpRouteHandler};
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use edgion_resources::resources::refs::plugin_fields::PluginMirrorRefs;
use edgion_resources::resources::refs::route_fields::HttpRouteMirrorRefs;
use edgion_resources::resources::ReferenceGrant;
use edgion_resources::ResourceKind;
use std::sync::Arc;

fn grant(group: &str, kind: &str, name: &str) -> ReferenceGrant {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "ReferenceGrant",
        "metadata": {"name": "mirror", "namespace": "target"},
        "spec": {
            "from": [{"group": group, "kind": kind, "namespace": "app"}],
            "to": [{"group": "", "kind": "Service", "name": name}]
        }
    }))
    .unwrap()
}

#[test]
fn mirror_grant_matches_declaring_identity_and_target_name() {
    let runtime = ControllerRuntime::default();
    let mut backend: BackendObjectReference = serde_json::from_value(serde_json::json!({
        "name": "shadow", "namespace": "target", "port": 80
    }))
    .unwrap();
    for (group, kind, name, http_allowed, plugin_allowed) in [
        ("gateway.networking.k8s.io", "HTTPRoute", "shadow", true, false),
        ("gateway.networking.k8s.io", "EdgionPlugins", "shadow", false, false),
        ("edgion.io", "EdgionPlugins", "shadow", false, true),
        ("edgion.io", "EdgionPlugins", "other", false, false),
    ] {
        runtime
            .ref_grant_store
            .upsert("target/mirror".to_string(), Arc::new(grant(group, kind, name)));
        for (owner, permitted) in [
            (ResourceKind::HTTPRoute, http_allowed),
            (ResourceKind::EdgionPlugins, plugin_allowed),
        ] {
            assert_eq!(
                resolve_owned(&mut backend, owner, &runtime, |query| runtime
                    .ref_grant_store
                    .check_query(query)),
                Some("target".to_string())
            );
            assert_eq!(backend.ref_denied.is_none(), permitted);
        }
    }
    runtime.ref_grant_store.remove("target/mirror");
    resolve_owned(&mut backend, ResourceKind::HTTPRoute, &runtime, |query| {
        runtime.ref_grant_store.check_query(query)
    });
    assert!(backend.ref_denied.is_some());
    backend.namespace = None;
    assert_eq!(
        resolve_owned(&mut backend, ResourceKind::HTTPRoute, &runtime, |_| panic!(
            "same namespace needs no grant"
        )),
        None
    );
    assert!(backend.ref_denied.is_none());
    assert_eq!(backend.namespace.as_deref(), Some("app"));
}

#[tokio::test]
async fn mirror_http_parse_tracks_rule_and_backend_refs_and_clears_old_dependencies() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let mirror = serde_json::json!({"type": "RequestMirror", "requestMirror": {
        "backendRef": {"name": "shadow", "namespace": "target", "port": 80,
            "refDenied": {"targetNamespace": "forged", "targetName": "forged"}}
    }});
    let route: edgion_resources::resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "mirrors", "namespace": "app"},
        "spec": {"rules": [{"filters": [mirror.clone()], "backendRefs": [
            {"name": "primary", "port": 80, "filters": [mirror]}
        ]}, {"backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "missing-child"}]}]}
    }))
    .unwrap();
    let original = route.clone();
    let handler = HttpRouteHandler::default();
    let ProcessResult::Continue(mut route) = handler.parse(route, &ctx).await else {
        panic!("parse")
    };
    let owner = ResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "mirrors".to_string());
    assert!(runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
    assert!(runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
    let mirrors: Vec<_> = route
        .spec
        .rules
        .as_ref()
        .into_iter()
        .flatten()
        .flat_map(mirror_backends)
        .collect();
    assert_eq!(mirrors.len(), 2);
    assert!(mirrors
        .iter()
        .all(|b| b.ref_denied.as_ref().unwrap().target_namespace == "target"));
    assert_eq!(
        errors(HttpRouteMirrorRefs::from_route(&route).unwrap().effective(), &runtime).len(),
        2
    );
    runtime.ref_grant_store.upsert(
        "target/mirror".to_string(),
        Arc::new(grant("gateway.networking.k8s.io", "HTTPRoute", "shadow")),
    );
    let ProcessResult::Continue(parsed) = handler.parse(route, &ctx).await else {
        panic!("parse")
    };
    route = parsed;
    assert!(route
        .spec
        .rules
        .as_ref()
        .into_iter()
        .flatten()
        .flat_map(mirror_backends)
        .all(|b| b.ref_denied.is_none()));
    route.spec.rules = None;
    let ProcessResult::Continue(_) = handler.parse(route, &ctx).await else {
        panic!("parse")
    };
    assert!(!runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
    assert!(!runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));

    let ProcessResult::Continue(restored) = handler.parse(original, &ctx).await else {
        panic!("parse")
    };
    assert!(runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
    handler.on_delete(&restored, &ctx).await;
    assert!(!runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
}

#[tokio::test]
async fn mirror_http_missing_namespace_uses_one_owner_for_replace_and_delete() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let original: edgion_resources::resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "mirrors"},
        "spec": {"rules": [{"filters": [{
            "type": "RequestMirror",
            "requestMirror": {"backendRef": {"name": "shadow", "port": 80}}
        }]}]}
    }))
    .unwrap();
    let replacement: edgion_resources::resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "mirrors"},
        "spec": {"rules": []}
    }))
    .unwrap();
    let owner = ResourceRef::new(ResourceKind::HTTPRoute, None, "mirrors".to_string());
    let service_key = mirror_service_key("default", "shadow");
    let handler = HttpRouteHandler::default();

    let ProcessResult::Continue(_) = handler.parse(original.clone(), &ctx).await else {
        panic!("parse")
    };
    assert_eq!(runtime.service_ref_mgr.get_refs(&service_key), vec![owner.clone()]);
    assert_eq!(
        runtime.service_ref_mgr.get_dependencies(&owner.key()),
        vec![service_key.clone()]
    );

    let ProcessResult::Continue(_) = handler.parse(replacement, &ctx).await else {
        panic!("parse")
    };
    assert!(runtime.service_ref_mgr.get_refs(&service_key).is_empty());
    assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());

    let ProcessResult::Continue(restored) = handler.parse(original, &ctx).await else {
        panic!("parse")
    };
    assert_eq!(runtime.service_ref_mgr.get_refs(&service_key), vec![owner.clone()]);
    handler.on_delete(&restored, &ctx).await;
    assert!(runtime.service_ref_mgr.get_refs(&service_key).is_empty());
    assert!(runtime.service_ref_mgr.get_dependencies(&owner.key()).is_empty());
}

#[tokio::test]
async fn mirror_plugin_parse_recomputes_denial_and_status_on_grant_changes() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
    let original: edgion_resources::resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "mirrors", "namespace": "app"},
        "spec": {"requestPlugins": [{"type": "RequestMirror", "config": {
            "backendRef": {"name": "shadow", "namespace": "target", "port": 80}
        }}]}
    }))
    .unwrap();
    let delete_fixture = original.clone();
    let owner = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("app".to_string()),
        "mirrors".to_string(),
    );
    let handler = EdgionPluginsHandler::new();
    let mut input = original;
    for permitted in [false, true, false] {
        if permitted {
            runtime.ref_grant_store.upsert(
                "target/mirror".to_string(),
                Arc::new(grant("edgion.io", "EdgionPlugins", "shadow")),
            );
        } else {
            runtime.ref_grant_store.remove("target/mirror");
        }
        let ProcessResult::Continue(mut ep) = handler.parse(input, &ctx).await else {
            panic!("parse")
        };
        handler.update_status(&mut ep, &ctx, &[]);
        let wire = serde_json::to_value(&ep).unwrap();
        assert_eq!(
            wire["spec"]["requestPlugins"][0]["config"]["backendRef"]["refDenied"].is_null(),
            permitted
        );
        let conditions = wire["status"]["conditions"].as_array().unwrap();
        let resolved = conditions.iter().find(|c| c["type"] == "ResolvedRefs").unwrap();
        assert_eq!(resolved["status"], if permitted { "True" } else { "False" });
        if !permitted {
            assert_eq!(resolved["reason"], "RefNotPermitted");
        }
        assert!(runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
        assert!(runtime
            .service_ref_mgr
            .get_refs(&mirror_service_key("target", "shadow"))
            .contains(&owner));
        input = ep;
    }
    input.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let ProcessResult::Continue(mut disabled) = handler.parse(input, &ctx).await else {
        panic!("parse")
    };
    handler.update_status(&mut disabled, &ctx, &[]);
    let wire = serde_json::to_value(&disabled).unwrap();
    assert!(wire["status"]["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| { c["type"] == "ResolvedRefs" && c["status"] == "True" }));
    assert!(!runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
    input = disabled;
    input.spec.request_plugins = None;
    let ProcessResult::Continue(_) = handler.parse(input, &ctx).await else {
        panic!("parse")
    };
    assert!(!runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));

    let ProcessResult::Continue(restored) = handler.parse(delete_fixture, &ctx).await else {
        panic!("parse")
    };
    assert!(runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
    handler.on_delete(&restored, &ctx).await;
    assert!(!runtime
        .service_ref_mgr
        .get_refs(&mirror_service_key("target", "shadow"))
        .contains(&owner));
}

#[tokio::test]
async fn mirror_missing_service_reports_backend_not_found() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use k8s_openapi::api::core::v1::Service;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        16,
        Arc::new(ServiceHandler::new()),
        runtime.clone(),
    ));
    processor.set_raw_store(Arc::new(LocalRawStore::new()));
    runtime.processor_registry.register(processor);
    let backend: BackendObjectReference = serde_json::from_value(serde_json::json!({
        "name": "missing", "namespace": "target", "port": 80,
        "refDenied": {"targetNamespace": "target", "targetName": "missing"}
    }))
    .unwrap();

    let owner = mirror_route(&backend);
    let errors = errors(HttpRouteMirrorRefs::from_route(&owner).unwrap().effective(), &runtime);
    assert_eq!(
        errors,
        vec![ResolvedRefsError::BackendNotFound {
            namespace: "target".to_string(),
            name: "missing".to_string(),
        }]
    );
    assert_eq!(errors[0].reason(), "BackendNotFound");

    let mut plugins: edgion_resources::resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "mirrors", "namespace": "app"},
        "spec": {"requestPlugins": [{"type": "RequestMirror", "config": {
            "backendRef": {"name": "missing", "port": 80}
        }}]}
    }))
    .unwrap();
    EdgionPluginsHandler::new().update_status(
        &mut plugins,
        &HandlerContext::new(None, None, Default::default(), 5, 20, runtime),
        &[],
    );
    let resolved = plugins
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, "BackendNotFound");
}

#[test]
fn invalid_mirror_standard_fields_set_plugin_accepted_false() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime);
    let handler = EdgionPluginsHandler::new();
    let mut plugins: edgion_resources::resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": "mirrors", "namespace": "app"},
        "spec": {"requestPlugins": [{"type": "RequestMirror", "config": {
            "backendRef": {"name": "shadow", "port": 8080},
            "fraction": {"numerator": 1}, "percent": 50
        }}]}
    }))
    .unwrap();

    let validation_errors = handler.preparse(&mut plugins, &ctx);
    assert_eq!(validation_errors.len(), 1);
    assert!(validation_errors[0].contains("only one of fraction or percent"));
    handler.update_status(&mut plugins, &ctx, &validation_errors);
    let accepted = plugins
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
}

fn mirror_route(backend: &BackendObjectReference) -> edgion_resources::HTTPRoute {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"owner","namespace":"app"},"spec":{"rules":[{"filters":[{"type":"RequestMirror","requestMirror":{"backendRef":backend}}]}]}})).unwrap()
}
fn resolve_owned(
    backend: &mut BackendObjectReference,
    kind: ResourceKind,
    runtime: &ControllerRuntime,
    check: impl FnOnce(GrantQuery<'_>) -> bool,
) -> Option<String> {
    let root = ResourceRef::new(kind, Some("app".into()), "owner".into());
    let route = mirror_route(backend);
    let plugins: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"owner","namespace":"app"},"spec":{"requestPlugins":[{"type":"RequestMirror","config":{"backendRef":backend}}]}})).unwrap();
    let field = if kind == ResourceKind::HTTPRoute {
        HttpRouteMirrorRefs::from_route(&route)
            .unwrap()
            .authored_at(0, None, 0)
            .unwrap()
    } else {
        PluginMirrorRefs::from_plugins(&plugins).authored_at(0).unwrap()
    };
    let decision = resolve(field, |terminal| authorize_with(terminal, &root, true, runtime, check));
    decision.apply_backend(backend);
    // Cross-namespace target: the resolved namespace when it differs from the owner's.
    backend.namespace.clone().filter(|namespace| namespace != "app")
}
fn mirror_backends(rule: &edgion_resources::HTTPRouteRule) -> impl Iterator<Item = &BackendObjectReference> {
    rule.filters
        .iter()
        .flatten()
        .chain(
            rule.backend_refs
                .iter()
                .flatten()
                .flat_map(|backend| backend.filters.iter().flatten()),
        )
        .filter_map(|filter| filter.request_mirror.as_ref().map(|mirror| &mirror.backend_ref))
}

#[test]
fn mirror_invalid_fields_never_read_and_valid_edges_precede_grant_reads() {
    let runtime = ControllerRuntime::default();
    let root = ResourceRef::new(ResourceKind::HTTPRoute, None, "published".into());
    for backend in [
        serde_json::json!({"name":"bad/name","namespace":"target","port":80}),
        serde_json::json!({"name":"copy","namespace":"bad/ns","port":80}),
        serde_json::json!({"name":"copy","namespace":"target","group":"core","port":80}),
        serde_json::json!({"name":"copy","namespace":"target","port":0}),
    ] {
        let route = mirror_route(&serde_json::from_value(backend).unwrap());
        let field = HttpRouteMirrorRefs::from_route(&route)
            .unwrap()
            .authored_at(0, None, 0)
            .unwrap();
        resolve(field, |_| panic!("invalid descriptor must not authorize or register"));
        assert!(runtime.service_ref_mgr.get_dependencies(&root.key()).is_empty());
        assert!(runtime.cross_ns_ref_mgr.get_dependencies(&root.key()).is_empty());
    }
    for allowed in [false, true] {
        let route = mirror_route(
            &serde_json::from_value(serde_json::json!({"name":"copy","namespace":"target","port":80})).unwrap(),
        );
        let field = HttpRouteMirrorRefs::from_route(&route)
            .unwrap()
            .authored_at(0, None, 0)
            .unwrap();
        let decision = resolve(field, |terminal| {
            authorize_with(terminal, &root, true, &runtime, |query| {
                assert_eq!(query.source().scope().namespace(), Some("app"));
                assert_eq!(
                    runtime.service_ref_mgr.get_refs(&mirror_service_key("target", "copy")),
                    vec![root.clone()]
                );
                assert_eq!(runtime.cross_ns_ref_mgr.get_refs("target"), vec![root.clone()]);
                allowed
            })
        });
        assert_eq!(decision.denied.is_none(), allowed);
    }
}
