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
use std::collections::{BTreeSet, HashMap, HashSet};
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

struct Candidates<T> {
    prepared: Vec<Arc<T>>,
    pending: bool,
}

/// One invocation-local indexed/raw union. Initial raw policies have no dependency
/// receipt yet; old receipts also keep removed/moved targets visible until publish.
fn policies<T: kube::Resource + Any + Send + Sync>(
    kind: &str,
    targets: &HashSet<PolicyTargetKey>,
    target_keys: impl Fn(&T) -> HashSet<PolicyTargetKey>,
    ctx: &HandlerContext,
) -> HashMap<PolicyTargetKey, Candidates<T>> {
    let mut result: HashMap<_, _> = targets
        .iter()
        .cloned()
        .map(|target| {
            (
                target,
                Candidates {
                    prepared: Vec::new(),
                    pending: false,
                },
            )
        })
        .collect();
    let Some(processor) = ctx.runtime().processor_registry.get(kind) else {
        return result;
    };
    let mut key_targets: HashMap<String, HashSet<PolicyTargetKey>> = HashMap::new();
    for target in targets {
        let references = match target.target_kind {
            PolicyTargetKind::Service => ctx
                .runtime()
                .service_ref_mgr
                .get_refs(&format!("{}/{}", target.namespace, target.name)),
            PolicyTargetKind::EdgionBackend => ctx.runtime().backend_ref_mgr.get_refs(&target.to_string()),
        };
        for reference in references.into_iter().filter(|reference| reference.kind_str() == kind) {
            key_targets
                .entry(reference.resource_key())
                .or_default()
                .insert(target.clone());
        }
    }
    let mut raw_by_key: HashMap<String, Option<Arc<T>>> = HashMap::new();
    for key in processor.raw_keys() {
        let raw = processor.get_raw_any(&key).and_then(|raw| raw.downcast::<T>().ok());
        if let Some(raw) = &raw {
            for target in target_keys(raw).intersection(targets) {
                key_targets.entry(key.clone()).or_default().insert(target.clone());
            }
        }
        if key_targets.contains_key(&key) {
            raw_by_key.insert(key, raw);
        }
    }
    for (key, affected) in key_targets {
        let raw = raw_by_key
            .remove(&key)
            .unwrap_or_else(|| processor.get_raw_any(&key).and_then(|raw| raw.downcast::<T>().ok()));
        let cached = processor
            .get_cached_any(&key)
            .and_then(|cached| cached.downcast::<T>().ok());
        let cached_targets = cached.as_ref().map(|policy| target_keys(policy)).unwrap_or_default();
        let same_generation = match (&raw, &cached) {
            (Some(raw), Some(cached)) => {
                raw.meta().uid == cached.meta().uid && raw.meta().generation == cached.meta().generation
            }
            _ => true,
        };
        for target in affected {
            if let Some(bucket) = result.get_mut(&target) {
                match &cached {
                    Some(policy) if same_generation && cached_targets.contains(&target) => {
                        bucket.prepared.push(Arc::clone(policy))
                    }
                    _ => bucket.pending = true,
                }
            }
        }
    }
    result
}

struct DiagnosticBackend {
    rule: usize,
    backend: usize,
    target: PolicyTargetKey,
    port: Option<i32>,
}

struct PolicyDecision {
    pending: bool,
    tls: Option<Arc<BackendTLSPolicy>>,
    traffic: Option<Arc<EdgionBackendTrafficPolicy>>,
}

struct PreparedDiagnostics {
    backends: Vec<DiagnosticBackend>,
    decisions: HashMap<(PolicyTargetKey, Option<i32>), PolicyDecision>,
}

fn prepare_diagnostics<'a>(
    refs: impl Iterator<Item = EffectiveBackendReference<'a>>,
    ctx: &HandlerContext,
) -> PreparedDiagnostics {
    let backends: Vec<_> = refs
        .filter_map(|reference| {
            if reference.ref_denied().is_some() {
                return None;
            }
            let (target_kind, identity) = match reference.target() {
                Ok(BackendTarget::Service(identity)) => (PolicyTargetKind::Service, identity),
                Ok(BackendTarget::EdgionBackend(identity)) => (PolicyTargetKind::EdgionBackend, identity),
                _ => return None,
            };
            Some(DiagnosticBackend {
                rule: reference.slot().rule(),
                backend: reference.slot().backend(),
                port: reference.port(),
                target: PolicyTargetKey {
                    target_kind,
                    namespace: identity.scope().namespace().unwrap_or("default").into(),
                    name: identity.name().into(),
                },
            })
        })
        .collect();
    let mut decisions = HashMap::new();
    if backends.is_empty() {
        return PreparedDiagnostics { backends, decisions };
    }
    let targets = backends.iter().map(|backend| backend.target.clone()).collect();
    let tls = policies::<BackendTLSPolicy>(
        "BackendTLSPolicy",
        &targets,
        |policy| {
            BackendTlsPolicyTargets::from_policy(policy)
                .fields()
                .filter_map(|field| field.canonical_key().ok())
                .collect()
        },
        ctx,
    );
    let traffic = policies::<EdgionBackendTrafficPolicy>(
        "EdgionBackendTrafficPolicy",
        &targets,
        |policy| {
            BackendTrafficPolicyTargets::from_policy(policy)
                .fields()
                .filter_map(|field| field.canonical_key().ok())
                .collect()
        },
        ctx,
    );
    let services: HashMap<_, _> = targets
        .iter()
        .filter(|target| target.target_kind == PolicyTargetKind::Service)
        .map(|target| {
            let service = ctx
                .runtime()
                .processor_registry
                .get("Service")
                .and_then(|processor| processor.get_cached_any(&format!("{}/{}", target.namespace, target.name)))
                .and_then(|raw| raw.downcast::<Service>().ok());
            (target.clone(), service)
        })
        .collect();
    for backend in &backends {
        decisions
            .entry((backend.target.clone(), backend.port))
            .or_insert_with(|| {
                let tls = tls.get(&backend.target);
                let traffic = traffic.get(&backend.target);
                let port = services
                    .get(&backend.target)
                    .and_then(Option::as_ref)
                    .and_then(|service| service.spec.as_ref())
                    .and_then(|spec| spec.ports.as_ref())
                    .and_then(|ports| ports.iter().find(|port| Some(port.port) == backend.port));
                let section = if backend.target.target_kind == PolicyTargetKind::EdgionBackend {
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
                let tls_winner = tls.and_then(|bucket| {
                    match select_backend_tls_candidate(
                        bucket.prepared.iter().map(Arc::as_ref),
                        &backend.target,
                        section,
                    ) {
                        PolicyCandidate::Effective(winner) => bucket
                            .prepared
                            .iter()
                            .find(|policy| std::ptr::eq(policy.as_ref(), winner))
                            .cloned(),
                        _ => None,
                    }
                });
                let traffic_winner = traffic.and_then(|bucket| {
                    select_backend_traffic_candidate(bucket.prepared.iter().map(Arc::as_ref), &backend.target).and_then(
                        |winner| {
                            bucket
                                .prepared
                                .iter()
                                .find(|policy| std::ptr::eq(policy.as_ref(), winner))
                                .cloned()
                        },
                    )
                });
                PolicyDecision {
                    pending: tls.is_some_and(|bucket| bucket.pending) || traffic.is_some_and(|bucket| bucket.pending),
                    tls: tls_winner,
                    traffic: traffic_winner,
                }
            });
    }
    PreparedDiagnostics { backends, decisions }
}

impl PreparedDiagnostics {
    fn errors(&self, gateway: &str) -> Vec<String> {
        let mut errors = BTreeSet::new();
        for backend in &self.backends {
            let Some(decision) = self.decisions.get(&(backend.target.clone(), backend.port)) else {
                continue;
            };
            if decision.pending {
                errors.insert(format!(
                    "rules[{}].backendRefs[{}]: policy ancestor admission pending publication",
                    backend.rule, backend.backend
                ));
            }
            let mut refused = Vec::new();
            if let Some(policy) = &decision.tls {
                if !policy_ancestor_permitted(policy.spec.prepared_ancestor_admission.as_ref(), Some(gateway)) {
                    refused.push(("BackendTLSPolicy", policy.metadata.name.as_deref().unwrap_or_default()));
                }
            }
            if let Some(policy) = &decision.traffic {
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
                    backend.rule, backend.backend, backend.target.namespace, name
                ));
            }
        }
        errors.into_iter().collect()
    }
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
                    if !status.parents.iter().any(|parent| {
                        parent.controller_name == owner
                            && parent.parent_ref.canonical_identity(Some(namespace)).is_gateway()
                    }) {
                        return;
                    }
                    let Ok(refs) = $facade::from_route(route) else {
                        return;
                    };
                    let diagnostics = prepare_diagnostics(refs.effective(), ctx);
                    for (index, parent) in status.parents.iter().enumerate() {
                        let identity = parent.parent_ref.canonical_identity(Some(namespace));
                        if parent.controller_name != owner || !identity.is_gateway() {
                            continue;
                        }
                        {
                            let messages = diagnostics.errors(&format!("{}/{}", identity.namespace, identity.name));
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
