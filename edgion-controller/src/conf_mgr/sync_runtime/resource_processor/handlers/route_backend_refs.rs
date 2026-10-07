//! Native Route terminal authority and dependency receipts. Receipts never authorize reads.
use super::super::ref_grant::read_after_registering_cross_ns_dependency;
use super::route_utils::service_exists;
use crate::conf_mgr::sync_runtime::resource_processor::{ResolvedRefsError, ResourceRef};
use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::refs::route_fields::{
    AuthoredBackendReference, AuthoredRouteTarget, EffectiveBackendReference, RouteBackendSlot,
    TerminalBackendReference,
};
use edgion_resources::resources::refs::targets::BackendTarget;
use edgion_resources::resources::refs::{GrantQuery, ReferenceError};
use edgion_resources::ResourceKind;

/// Cold dependency evidence only; constructors require a valid actual terminal loan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BackendDependencyReceipt {
    source_kind: ResourceKind,
    source_namespace: String,
    source_name: String,
    slot: RouteBackendSlot,
    port: Option<i32>,
    target: PolicyTargetKey,
    key: String,
}
impl BackendDependencyReceipt {
    pub(super) fn from_terminal(terminal: TerminalBackendReference<'_>) -> Self {
        let (target_kind, identity) = match terminal.target() {
            BackendTarget::Service(identity) => (PolicyTargetKind::Service, identity),
            BackendTarget::EdgionBackend(identity) => (PolicyTargetKind::EdgionBackend, identity),
            _ => unreachable!("terminal loans contain only Service or EdgionBackend targets"),
        };
        let target = PolicyTargetKey {
            target_kind,
            namespace: identity.scope().namespace().unwrap_or_default().to_string(),
            name: identity.name().to_string(),
        };
        let key = match target_kind {
            PolicyTargetKind::Service => format!("{}/{}", target.namespace, target.name),
            PolicyTargetKind::EdgionBackend => target.to_string(),
        };
        Self {
            source_kind: terminal.source_kind(),
            source_namespace: terminal.source().scope().namespace().unwrap_or_default().to_string(),
            source_name: terminal.source_name().to_string(),
            slot: terminal.slot(),
            port: terminal.port(),
            target,
            key,
        }
    }
    pub(super) fn service_key(&self) -> Option<&str> {
        (self.target.target_kind == PolicyTargetKind::Service).then_some(self.key.as_str())
    }
    pub(super) fn backend_key(&self) -> Option<&str> {
        (self.target.target_kind == PolicyTargetKind::EdgionBackend).then_some(self.key.as_str())
    }
    pub(super) fn cross_namespace(&self) -> Option<&str> {
        (self.source_namespace != self.target.namespace).then_some(self.target.namespace.as_str())
    }
}

pub(super) fn collect_dependencies<'a>(
    references: impl IntoIterator<Item = AuthoredBackendReference<'a>>,
) -> Vec<BackendDependencyReceipt> {
    references
        .into_iter()
        .filter_map(|reference| match reference.validated() {
            Ok(AuthoredRouteTarget::Terminal(terminal)) => Some(BackendDependencyReceipt::from_terminal(terminal)),
            _ => None,
        })
        .collect()
}

pub(super) fn register_cross_namespace(
    receipts: &[BackendDependencyReceipt],
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
) {
    for namespace in receipts.iter().filter_map(BackendDependencyReceipt::cross_namespace) {
        runtime.cross_ns_ref_mgr.add_ref(namespace.to_string(), owner.clone());
    }
}

pub(super) fn authorize_terminal(
    terminal: TerminalBackendReference<'_>,
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
) -> Option<RefDenied> {
    authorize_terminal_with(
        terminal,
        owner,
        runtime,
        is_reference_grant_validation_enabled,
        |query| runtime.ref_grant_store.check_query(query),
    )
}

fn authorize_terminal_with<'a>(
    terminal: TerminalBackendReference<'a>,
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
    enabled: impl FnOnce() -> bool,
    allowed: impl FnOnce(GrantQuery<'a>) -> bool,
) -> Option<RefDenied> {
    let query = terminal.grant_query();
    let target_namespace = query.target().scope().namespace().unwrap_or_default();
    if query.source().scope() == query.target().scope() {
        return None;
    }
    let permitted =
        read_after_registering_cross_ns_dependency(runtime, target_namespace, owner, || !enabled() || allowed(query));
    (!permitted).then(|| RefDenied {
        target_namespace: target_namespace.to_string(),
        target_name: query.target().name().to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    })
}

pub(super) fn resolve_denials<'a>(
    references: impl IntoIterator<Item = AuthoredBackendReference<'a>>,
    owner: &ResourceRef,
    runtime: &ControllerRuntime,
) -> Vec<(RouteBackendSlot, Option<RefDenied>)> {
    references
        .into_iter()
        .map(|reference| {
            let denial = match reference.validated() {
                Ok(AuthoredRouteTarget::Terminal(terminal)) => authorize_terminal(terminal, owner, runtime),
                _ => None,
            };
            (reference.slot(), denial)
        })
        .collect()
}

pub(super) fn source_error_message(error: ReferenceError) -> String {
    format!("Invalid route reference source: {error}")
}

pub(super) fn validate_targets<'a>(
    references: Result<impl Iterator<Item = EffectiveBackendReference<'a>>, ReferenceError>,
    runtime: &ControllerRuntime,
) -> Vec<ResolvedRefsError> {
    validate_targets_with(references, |target| match target {
        BackendTarget::Service(identity) => service_exists(
            identity.scope().namespace().unwrap_or_default(),
            identity.name(),
            runtime,
        ),
        BackendTarget::EdgionBackend(identity) => {
            runtime.processor_registry.get("EdgionBackend").is_none_or(|processor| {
                processor.contains_key(&format!(
                    "{}/{}",
                    identity.scope().namespace().unwrap_or_default(),
                    identity.name()
                ))
            })
        }
        _ => true,
    })
}

fn validate_targets_with<'a>(
    references: Result<impl Iterator<Item = EffectiveBackendReference<'a>>, ReferenceError>,
    mut exists: impl FnMut(BackendTarget<'a>) -> bool,
) -> Vec<ResolvedRefsError> {
    let references = match references {
        Ok(references) => references,
        Err(error) => return vec![ResolvedRefsError::InvalidReference { error }],
    };
    let mut errors = Vec::new();
    for reference in references {
        match reference.target() {
            Err(error) => errors.push(ResolvedRefsError::InvalidReference { error }),
            Ok(_) if reference.ref_denied().is_some() => {}
            Ok(target @ BackendTarget::Service(identity)) if !exists(target) => {
                errors.push(ResolvedRefsError::BackendNotFound {
                    namespace: identity.scope().namespace().unwrap_or_default().to_string(),
                    name: identity.name().to_string(),
                })
            }
            Ok(target @ BackendTarget::EdgionBackend(identity)) if !exists(target) => {
                errors.push(ResolvedRefsError::EdgionBackendNotFound {
                    namespace: identity.scope().namespace().unwrap_or_default().to_string(),
                    name: identity.name().to_string(),
                })
            }
            Ok(_) => {}
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_backend_refs_test.rs"]
mod tests;
