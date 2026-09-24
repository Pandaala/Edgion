//! Mirror reference authorization shared by native filters and EdgionPlugins.

use std::collections::HashSet;

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::{BackendObjectReference, HTTPRouteFilterType, HTTPRouteRule};
use edgion_resources::ResourceKind;

use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;
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
    runtime: &ControllerRuntime,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for backend in refs {
        if !backend.is_service() {
            errors.extend(super::route_utils::collect_ref_denied_errors(
                from_ns,
                from_kind,
                &[backend.ref_denied.as_ref()],
            ));
            errors.push(ResolvedRefsError::InvalidKind {
                kind: format!("{}/{}", backend.group, backend.kind.as_deref().unwrap_or("Service")),
                name: backend.name.clone(),
            });
        } else if backend.name.is_empty()
            || backend
                .port
                .is_none_or(|port| !edgion_resources::gwapi_types::is_valid_port(port as i64))
        {
            // Structural validation owns these diagnostics. Do not add a
            // misleading BackendNotFound condition for an invalid object key.
        } else {
            let namespace = backend.namespace.as_deref().unwrap_or(from_ns);
            if !super::route_utils::service_exists(namespace, &backend.name, runtime) {
                errors.push(ResolvedRefsError::BackendNotFound {
                    namespace: namespace.to_string(),
                    name: backend.name.clone(),
                });
            } else {
                errors.extend(super::route_utils::collect_ref_denied_errors(
                    from_ns,
                    from_kind,
                    &[backend.ref_denied.as_ref()],
                ));
            }
        }
    }
    errors
}

/// Register valid core-Service mirrors in the existing Service dependency
/// manager under a role-qualified key. The caller owns clear-before-add so all
/// Service roles for one resource are replaced atomically within reconciliation.
pub(super) fn register_service_refs<'a>(
    owner: &ResourceRef,
    from_ns: &str,
    refs: impl Iterator<Item = &'a BackendObjectReference>,
    runtime: &ControllerRuntime,
) {
    for backend in refs {
        if !backend.is_service()
            || backend.name.is_empty()
            || backend
                .port
                .is_none_or(|port| !edgion_resources::gwapi_types::is_valid_port(port as i64))
        {
            continue;
        }
        let namespace = backend.namespace.as_deref().unwrap_or(from_ns);
        runtime
            .service_ref_mgr
            .add_ref(mirror_service_key(namespace, &backend.name), owner.clone());
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/request_mirror_test.rs"]
mod tests;
