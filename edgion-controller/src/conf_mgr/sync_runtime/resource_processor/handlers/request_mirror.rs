//! Owner-bound mirror authorization and dependency registration.
use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::http_route::{BackendObjectReference, HTTPRouteRule};
use edgion_resources::resources::refs::route_fields::{
    AuthoredMirrorField, EffectiveMirrorField, MirrorSlot, TerminalMirrorReference,
};
use edgion_resources::resources::refs::GrantQuery;

/// A cold decision can only be minted from an actual owning field loan.
#[derive(Clone)]
pub(super) struct MirrorDecision {
    slot: MirrorSlot,
    namespace: Option<String>,
    denied: Option<RefDenied>,
}
impl MirrorDecision {
    pub(super) fn apply_backend(&self, backend: &mut BackendObjectReference) {
        backend.ref_denied = self.denied.clone();
        if let Some(namespace) = &self.namespace {
            backend.namespace = Some(namespace.clone());
        }
    }
    pub(super) fn slot(&self) -> MirrorSlot {
        self.slot
    }
    pub(super) fn apply_rule(&self, rule: &mut HTTPRouteRule) {
        let MirrorSlot::Route(slot) = self.slot else { return };
        let filters = match slot.backend() {
            Some(index) => rule
                .backend_refs
                .as_mut()
                .and_then(|refs| refs.get_mut(index))
                .and_then(|backend| backend.filters.as_mut()),
            None => rule.filters.as_mut(),
        };
        if let Some(mirror) = filters
            .and_then(|filters| filters.get_mut(slot.filter()))
            .and_then(|filter| filter.request_mirror.as_mut())
        {
            self.apply_backend(&mut mirror.backend_ref);
        }
    }
}

pub(super) fn resolve(
    field: AuthoredMirrorField<'_>,
    authorize: impl FnOnce(TerminalMirrorReference<'_>) -> Option<RefDenied>,
) -> MirrorDecision {
    let reference = field.validated().ok().flatten();
    MirrorDecision {
        slot: field.field().slot(),
        namespace: reference.map(|reference| {
            reference
                .reference()
                .target()
                .scope()
                .namespace()
                .unwrap_or_default()
                .to_string()
        }),
        denied: reference.and_then(authorize),
    }
}

pub(super) fn authorize(
    terminal: TerminalMirrorReference<'_>,
    root: &ResourceRef,
    service_enabled: bool,
    runtime: &ControllerRuntime,
) -> Option<RefDenied> {
    authorize_with(terminal, root, service_enabled, runtime, |query| {
        !crate::config::is_reference_grant_validation_enabled() || runtime.ref_grant_store.check_query(query)
    })
}
fn authorize_with<'a>(
    terminal: TerminalMirrorReference<'a>,
    root: &ResourceRef,
    service_enabled: bool,
    runtime: &ControllerRuntime,
    allowed: impl FnOnce(GrantQuery<'a>) -> bool,
) -> Option<RefDenied> {
    let query = terminal.grant_query();
    let target = query.target();
    let namespace = target.scope().namespace().unwrap_or_default();
    if service_enabled {
        runtime
            .service_ref_mgr
            .add_ref(mirror_service_key(namespace, target.name()), root.clone());
    }
    if query.source().scope() == target.scope() {
        return None;
    }
    let permitted =
        super::super::ref_grant::read_after_registering_cross_ns_dependency(runtime, namespace, root, || {
            allowed(query)
        });
    (!permitted).then(|| RefDenied {
        target_namespace: namespace.to_string(),
        target_name: target.name().to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    })
}

/// Status may inspect cached presence only after parse has authorized this field.
/// Presence is a boolean: absence intentionally precedes an existing denial.
pub(super) fn errors<'a>(
    fields: impl Iterator<Item = EffectiveMirrorField<'a>>,
    runtime: &ControllerRuntime,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for field in fields {
        let Some(config) = field.config() else { continue };
        let backend = &config.backend_ref;
        let Ok(source) = field.source() else { continue };
        let from_ns = source.scope().namespace().unwrap_or_default();
        match field.validated() {
            Ok(Some(reference)) => {
                let namespace = reference.target().scope().namespace().unwrap_or_default();
                if !super::route_utils::service_exists(namespace, reference.target().name(), runtime) {
                    errors.push(ResolvedRefsError::BackendNotFound {
                        namespace: namespace.to_string(),
                        name: reference.target().name().to_string(),
                    });
                } else {
                    errors.extend(super::route_utils::collect_ref_denied_errors(
                        from_ns,
                        field.source_kind(),
                        &[backend.ref_denied.as_ref()],
                    ));
                }
            }
            Err(_) if !backend.is_service() => {
                errors.push(ResolvedRefsError::InvalidKind {
                    kind: format!(
                        "{}/{}",
                        backend.group.as_deref().unwrap_or_default(),
                        backend.kind.as_deref().unwrap_or("Service")
                    ),
                    name: backend.name.clone(),
                });
            }
            _ => {}
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/request_mirror_test.rs"]
mod tests;
