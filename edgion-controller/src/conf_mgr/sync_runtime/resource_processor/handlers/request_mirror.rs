//! Mirror reference authorization shared by native filters and EdgionPlugins.

use std::collections::HashSet;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::{BackendObjectReference, HTTPRouteFilterType, HTTPRouteRule};
use edgion_resources::ResourceKind;

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use crate::conf_mgr::ControllerRuntime;

/// Use the canonical store with the declaring resource's actual API group.
pub(super) fn allowed(
    backend: &BackendObjectReference,
    from_ns: &str,
    from_kind: ResourceKind,
    runtime: &ControllerRuntime,
) -> bool {
    !crate::config::is_reference_grant_validation_enabled()
        || runtime.ref_grant_store.check_reference_allowed(
            from_ns,
            if from_kind == ResourceKind::EdgionPlugins {
                "edgion.io"
            } else {
                "gateway.networking.k8s.io"
            },
            from_kind.as_str(),
            backend.namespace.as_deref().unwrap_or(from_ns),
            &backend.group,
            backend.kind.as_deref().unwrap_or("Service"),
            Some(&backend.name),
        )
}

/// Replace untrusted/stale denial metadata and pin the declaring namespace
/// before a filter can be inherited by another route. Return its grant dependency.
pub(super) fn resolve(
    backend: &mut BackendObjectReference,
    from_ns: &str,
    check: impl FnOnce(&BackendObjectReference) -> bool,
) -> Option<String> {
    let target_ns = backend.namespace.get_or_insert_with(|| from_ns.to_string()).clone();
    backend.ref_denied = None;
    if target_ns == from_ns {
        return None;
    }
    if !check(backend) {
        backend.ref_denied = Some(RefDenied {
            target_namespace: target_ns.clone(),
            target_name: backend.name.clone(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
    }
    Some(target_ns)
}

pub(super) fn resolve_rule(
    rule: &mut HTTPRouteRule,
    from_ns: &str,
    check: impl Fn(&BackendObjectReference) -> bool,
) -> HashSet<String> {
    let mut targets = HashSet::new();
    for filter in rule.filters.iter_mut().flatten().chain(
        rule.backend_refs
            .iter_mut()
            .flatten()
            .flat_map(|br| br.filters.iter_mut().flatten()),
    ) {
        if filter.filter_type == HTTPRouteFilterType::RequestMirror {
            if let Some(mirror) = &mut filter.request_mirror {
                if let Some(target) = resolve(&mut mirror.backend_ref, from_ns, &check) {
                    targets.insert(target);
                }
            }
        }
    }
    targets
}

pub(super) fn refs(rule: &HTTPRouteRule) -> impl Iterator<Item = &BackendObjectReference> {
    rule.filters
        .iter()
        .flatten()
        .chain(
            rule.backend_refs
                .iter()
                .flatten()
                .flat_map(|br| br.filters.iter().flatten()),
        )
        .filter(|filter| filter.filter_type == HTTPRouteFilterType::RequestMirror)
        .filter_map(|filter| filter.request_mirror.as_ref().map(|mirror| &mirror.backend_ref))
}

pub(super) fn errors<'a>(
    from_ns: &str,
    from_kind: ResourceKind,
    refs: impl Iterator<Item = &'a BackendObjectReference>,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for backend in refs {
        errors.extend(super::route_utils::collect_ref_denied_errors(
            from_ns,
            from_kind,
            &[backend.ref_denied.as_ref()],
        ));
        if !backend.is_service() {
            errors.push(ResolvedRefsError::InvalidKind {
                kind: format!("{}/{}", backend.group, backend.kind.as_deref().unwrap_or("Service")),
                name: backend.name.clone(),
            });
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::super::{EdgionPluginsHandler, HttpRouteHandler};
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
    };
    use edgion_resources::resources::ReferenceGrant;
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
                    resolve(&mut backend, "app", |b| allowed(b, "app", owner, &runtime)),
                    Some("target".to_string())
                );
                assert_eq!(backend.ref_denied.is_none(), permitted);
            }
        }
        runtime.ref_grant_store.remove("target/mirror");
        resolve(&mut backend, "app", |b| {
            allowed(b, "app", ResourceKind::HTTPRoute, &runtime)
        });
        assert!(backend.ref_denied.is_some());
        backend.namespace = None;
        assert_eq!(
            resolve(&mut backend, "app", |_| panic!("same namespace needs no grant")),
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
        let route = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"name": "mirrors", "namespace": "app"},
            "spec": {"rules": [{"filters": [mirror.clone()], "backendRefs": [
                {"name": "primary", "port": 80, "filters": [mirror]}
            ]}, {"backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "missing-child"}]}]}
        }))
        .unwrap();
        let handler = HttpRouteHandler::default();
        let ProcessResult::Continue(mut route) = handler.parse(route, &ctx).await else {
            panic!("parse")
        };
        let owner = ResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "mirrors".to_string());
        assert!(runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
        let mirrors: Vec<_> = route.effective_rules().into_iter().flatten().flat_map(refs).collect();
        assert!(
            route.spec.resolved_rules.is_some(),
            "must exercise the compiled sibling copy"
        );
        assert_eq!(mirrors.len(), 2);
        assert!(mirrors
            .iter()
            .all(|b| b.ref_denied.as_ref().unwrap().target_namespace == "target"));
        assert_eq!(errors("app", ResourceKind::HTTPRoute, mirrors.into_iter()).len(), 2);
        runtime.ref_grant_store.upsert(
            "target/mirror".to_string(),
            Arc::new(grant("gateway.networking.k8s.io", "HTTPRoute", "shadow")),
        );
        let ProcessResult::Continue(parsed) = handler.parse(route, &ctx).await else {
            panic!("parse")
        };
        route = parsed;
        assert!(route
            .effective_rules()
            .into_iter()
            .flatten()
            .flat_map(refs)
            .all(|b| b.ref_denied.is_none()));
        route.spec.rules = None;
        let ProcessResult::Continue(_) = handler.parse(route, &ctx).await else {
            panic!("parse")
        };
        assert!(!runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
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
        input = disabled;
        input.spec.request_plugins = None;
        let ProcessResult::Continue(_) = handler.parse(input, &ctx).await else {
            panic!("parse")
        };
        assert!(!runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner));
    }
}
