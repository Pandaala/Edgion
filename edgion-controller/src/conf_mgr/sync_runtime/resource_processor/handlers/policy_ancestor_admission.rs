//! Policy capacity diagnostics and changed-publication dependency fanout.
use super::super::{condition_false, update_condition, HandlerContext};
use edgion_resources::resources::common::policy_ancestor_admission::policy_ancestor_permitted;
use edgion_resources::resources::common::policy_candidates::*;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::refs::policy_fields::{BackendTlsPolicyTargets, BackendTrafficPolicyTargets};
use edgion_resources::resources::refs::route_fields::*;
use edgion_resources::resources::refs::targets::BackendTarget;
use edgion_resources::{
    BackendTLSPolicy, EdgionBackendTrafficPolicy, GRPCRoute, HTTPRoute, TCPRoute, TLSRoute, UDPRoute,
};
use k8s_openapi::api::core::v1::Service;
use serde_json::{json, Value};
use std::any::Any;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

fn signature(value: Option<&(dyn Any + Send + Sync)>) -> Option<(Value, BTreeSet<PolicyTargetKey>)> {
    let value = value?;
    if let Some(p) = value.downcast_ref::<BackendTLSPolicy>() {
        let targets = BackendTlsPolicyTargets::from_policy(p)
            .fields()
            .filter_map(|f| f.canonical_key().ok())
            .collect();
        return Some((
            json!([
                p.spec.target_refs,
                p.spec.prepared_precedence,
                p.spec.prepared_ancestor_admission,
                prepared_backend_tls_target(p).is_some(),
                p.metadata.uid,
                p.metadata.generation
            ]),
            targets,
        ));
    }
    if let Some(p) = value.downcast_ref::<EdgionBackendTrafficPolicy>() {
        let targets = BackendTrafficPolicyTargets::from_policy(p)
            .fields()
            .filter_map(|f| f.canonical_key().ok())
            .collect();
        return Some((
            json!([
                p.spec.target_refs,
                p.spec.prepared_precedence,
                p.spec.prepared_ancestor_admission,
                prepared_backend_traffic_targets(p).is_some(),
                p.metadata.uid,
                p.metadata.generation
            ]),
            targets,
        ));
    }
    None
}

pub(crate) async fn notify_policy_ancestor_change(
    old: Option<&(dyn Any + Send + Sync)>,
    new: Option<&(dyn Any + Send + Sync)>,
    ctx: &HandlerContext,
) {
    let old = signature(old);
    let new = signature(new);
    if old == new {
        return;
    }
    let mut routes = HashSet::new();
    for target in old.iter().chain(new.iter()).flat_map(|(_, targets)| targets) {
        let refs = match target.target_kind {
            PolicyTargetKind::Service => ctx
                .runtime()
                .service_ref_mgr
                .get_refs(&format!("{}/{}", target.namespace, target.name)),
            PolicyTargetKind::EdgionBackend => ctx.runtime().backend_ref_mgr.get_refs(&target.to_string()),
        };
        for reference in refs {
            if matches!(
                reference.kind_str(),
                "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute"
            ) {
                routes.insert((reference.kind_str().to_owned(), reference.resource_key()));
            }
        }
    }
    for (kind, key) in routes {
        ctx.requeue(&kind, key).await;
    }
}

fn policies<T: kube::Resource + Any + Send + Sync>(
    kind: &str,
    target: &PolicyTargetKey,
    matches: impl Fn(&T) -> bool,
    ctx: &HandlerContext,
) -> (Vec<Arc<T>>, bool) {
    let Some(processor) = ctx.runtime().processor_registry.get(kind) else {
        return (Vec::new(), false);
    };
    let references = match target.target_kind {
        PolicyTargetKind::Service => ctx
            .runtime()
            .service_ref_mgr
            .get_refs(&format!("{}/{}", target.namespace, target.name)),
        PolicyTargetKind::EdgionBackend => ctx.runtime().backend_ref_mgr.get_refs(&target.to_string()),
    };
    let mut keys: HashSet<_> = references
        .into_iter()
        .filter(|reference| reference.kind_str() == kind)
        .map(|reference| reference.resource_key())
        .collect();
    // Include pending raw candidates before their first parse registered dependencies.
    for key in processor.raw_keys() {
        if processor
            .get_raw_any(&key)
            .and_then(|raw| raw.downcast::<T>().ok())
            .is_some_and(|raw| matches(&raw))
        {
            keys.insert(key);
        }
    }
    let mut pending = false;
    let mut prepared = Vec::new();
    for key in keys {
        match processor
            .get_cached_any(&key)
            .and_then(|cached| cached.downcast::<T>().ok())
        {
            Some(policy) if matches(&policy) => {
                let raw = processor.get_raw_any(&key).and_then(|raw| raw.downcast::<T>().ok());
                if raw.as_ref().is_some_and(|raw| {
                    raw.meta().uid != policy.meta().uid || raw.meta().generation != policy.meta().generation
                }) {
                    pending = true;
                } else {
                    prepared.push(policy);
                }
            }
            _ => pending = true,
        }
    }
    (prepared, pending)
}

fn errors<'a>(
    refs: impl Iterator<Item = EffectiveBackendReference<'a>>,
    gateway: &str,
    ctx: &HandlerContext,
) -> Vec<String> {
    let mut errors = BTreeSet::new();
    for reference in refs {
        if reference.ref_denied().is_some() {
            continue;
        }
        let (target_kind, identity) = match reference.target() {
            Ok(BackendTarget::Service(identity)) => (PolicyTargetKind::Service, identity),
            Ok(BackendTarget::EdgionBackend(identity)) => (PolicyTargetKind::EdgionBackend, identity),
            _ => continue,
        };
        let target = PolicyTargetKey {
            target_kind,
            namespace: identity.scope().namespace().unwrap_or("default").into(),
            name: identity.name().into(),
        };
        let (tls, tls_pending) = policies::<BackendTLSPolicy>(
            "BackendTLSPolicy",
            &target,
            |policy| {
                BackendTlsPolicyTargets::from_policy(policy)
                    .fields()
                    .any(|field| field.canonical_key().as_ref() == Ok(&target))
            },
            ctx,
        );
        let (traffic, traffic_pending) = policies::<EdgionBackendTrafficPolicy>(
            "EdgionBackendTrafficPolicy",
            &target,
            |policy| {
                BackendTrafficPolicyTargets::from_policy(policy)
                    .fields()
                    .any(|field| field.canonical_key().as_ref() == Ok(&target))
            },
            ctx,
        );
        if tls_pending || traffic_pending {
            errors.insert(format!(
                "rules[{}].backendRefs[{}]: policy ancestor admission pending publication",
                reference.slot().rule(),
                reference.slot().backend()
            ));
        }
        let service = (target_kind == PolicyTargetKind::Service)
            .then(|| {
                ctx.runtime()
                    .processor_registry
                    .get("Service")
                    .and_then(|processor| processor.get_cached_any(&format!("{}/{}", target.namespace, target.name)))
                    .and_then(|raw| raw.downcast::<Service>().ok())
            })
            .flatten();
        let port = service
            .as_ref()
            .and_then(|service| service.spec.as_ref())
            .and_then(|spec| spec.ports.as_ref())
            .and_then(|ports| ports.iter().find(|port| Some(port.port) == reference.port()));
        let section = if target_kind == PolicyTargetKind::EdgionBackend {
            PolicySection::NumberOnly
        } else {
            match port {
                Some(port) => port
                    .name
                    .as_deref()
                    .map(PolicySection::Named)
                    .unwrap_or(PolicySection::NumberOnly),
                None => PolicySection::Unproven,
            }
        };
        let mut refused = Vec::new();
        if let PolicyCandidate::Effective(policy) =
            select_backend_tls_candidate(tls.iter().map(Arc::as_ref), &target, section)
        {
            if !policy_ancestor_permitted(policy.spec.prepared_ancestor_admission.as_ref(), Some(gateway)) {
                refused.push(("BackendTLSPolicy", policy.metadata.name.as_deref().unwrap_or_default()));
            }
        }
        if let Some(policy) = select_backend_traffic_candidate(traffic.iter().map(Arc::as_ref), &target) {
            if !policy_ancestor_permitted(policy.spec.prepared_ancestor_admission.as_ref(), Some(gateway)) {
                refused.push((
                    "EdgionBackendTrafficPolicy",
                    policy.metadata.name.as_deref().unwrap_or_default(),
                ));
            }
        }
        for (kind, name) in refused {
            errors.insert(format!(
                "rules[{}].backendRefs[{}]: {kind} {}/{} cannot represent this Gateway ancestor",
                reference.slot().rule(),
                reference.slot().backend(),
                target.namespace,
                name
            ));
        }
    }
    errors.into_iter().collect()
}

/// Called by each Route status builder after its ordinary reference diagnostics.
pub(super) fn annotate(resource: &mut dyn Any, owner: &str, ctx: &HandlerContext) {
    if ctx.runtime().status_identity().is_none() {
        return;
    }
    macro_rules! route {
        ($ty:ty, $facade:ident) => {
            if let Some(route) = resource.downcast_mut::<$ty>() {
                let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
                let mut updates = Vec::new();
                if let Some(status) = &route.status {
                    for (index, parent) in status.parents.iter().enumerate() {
                        let identity = parent.parent_ref.canonical_identity(Some(namespace));
                        if parent.controller_name != owner || !identity.is_gateway() {
                            continue;
                        }
                        if let Ok(refs) = $facade::from_route(route) {
                            let messages = errors(
                                refs.effective(),
                                &format!("{}/{}", identity.namespace, identity.name),
                                ctx,
                            );
                            if !messages.is_empty() {
                                let mut message = messages.join("; ");
                                // Condition messages are diagnostics, not an unbounded backend inventory.
                                if message.len() > 1024 {
                                    let mut boundary = 1021;
                                    while !message.is_char_boundary(boundary) {
                                        boundary -= 1;
                                    }
                                    message.truncate(boundary);
                                    message.push_str("...");
                                }
                                updates.push((index, message));
                            }
                        }
                    }
                }
                if let Some(status) = &mut route.status {
                    for (index, message) in updates {
                        update_condition(
                            &mut status.parents[index].conditions,
                            condition_false(
                                "ResolvedRefs",
                                "PolicyAncestorLimit",
                                message,
                                route.metadata.generation,
                            ),
                        );
                    }
                }
                return;
            }
        };
    }
    route!(HTTPRoute, HttpRouteBackendRefs);
    route!(GRPCRoute, GrpcRouteBackendRefs);
    route!(TCPRoute, TcpRouteBackendRefs);
    route!(TLSRoute, TlsRouteBackendRefs);
    route!(UDPRoute, UdpRouteBackendRefs);
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/policy_ancestor_admission_test.rs"]
mod tests;
