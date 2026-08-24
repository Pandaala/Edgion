//! EdgionBackendTrafficPolicy Handler (Controller side)
//!
//! Handles `EdgionBackendTrafficPolicy` resources — a per-destination traffic
//! policy (LB algorithm / active health check / outlier detection / panic
//! threshold) attached to homogeneous core Service or EdgionBackend targets.
//!
//! Status is GEP-2648 per-ancestor (`status.ancestors[]`): the ancestor of this
//! direct policy is the Gateway whose routes reach a target. One entry is
//! produced per ancestor Gateway (via the typed ancestor resolver),
//! stamped with this controller's name, so the same policy can be Accepted on
//! one Gateway and Conflicted on another. `status.ancestors` is empty when no
//! route references any target (matching the BackendTLSPolicy handler's
//! precedent) — lenient admission still applies (an invalid or unresolved policy
//! still syncs to the data plane, whose marker withdraws invalid specs):
//! - `Accepted`:
//!   - `True`/`Accepted` while an otherwise staged target is missing; the
//!     failure is carried by `ResolvedRefs=False/TargetNotFound`.
//!   - else `False`/`Invalid` when complete evaluation finds an incompatible,
//!     mixed, forbidden, or otherwise invalid policy.
//!   - else `False`/`Conflicted` when another policy in the same namespace won
//!     oldest-wins arbitration over a target reaching this ancestor.
//!   - else `True`/`Accepted`.
//! - `ResolvedRefs` — `False`/`TargetNotFound` when a target reaching this
//!   ancestor is absent from its typed cache. Target changes requeue the policy.
//!
//! GEP-2648 folds conflict into `Accepted=False/Conflicted`; there is no
//! separate `Conflicted` condition type (unlike the flat-status precedent this
//! resource used pre-GEP-2648).
//!
//! Conflict / precedence state is tracked in `ControllerRuntime`
//! (`edgion_backend_traffic_policy`, a `BackendTLSPolicyState` reused as the
//! target-conflict + creation-timestamp index) exactly as `BackendTLSPolicy`
//! does.
//!
//! Requeue edges (this handler is a sink in the requeue DAG — it never
//! requeues routes, gateways, or Services; it only requeues its own siblings):
//! - Service or EdgionBackend appear/disappear requeues every policy ref
//!   registered under the corresponding typed dependency key.
//! - Route change -> `requeue_backend_policies_for_route` /
//!   `requeue_backend_policies_for_services` (in `handlers/mod.rs`) requeue
//!   policies bound to the route's backend Services, so ancestor Gateways
//!   re-resolve.
//! - Sibling policy change (same typed target) -> `parse`'s
//!   `should_requeue_siblings` gate.

const STATUS_MESSAGE_MAX_BYTES: usize = 1024;
const STATUS_TRUNCATION_SUFFIX: &str = "...";

fn bounded_status_message(mut message: String) -> String {
    if message.len() <= STATUS_MESSAGE_MAX_BYTES {
        return message;
    }
    let keep = STATUS_MESSAGE_MAX_BYTES.saturating_sub(STATUS_TRUNCATION_SUFFIX.len());
    let mut boundary = keep.min(message.len());
    while boundary > 0 && !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message.push_str(STATUS_TRUNCATION_SUFFIX);
    message
}

use crate::conf_mgr::sync_runtime::resource_processor::handlers::backend_capability::classify_raw_backend;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::resolve_policy_ancestor_gateways_for_targets;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use chrono::{DateTime, Utc};
#[cfg(test)]
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
use edgion_resources::resources::common::{ParentReference, PolicyAncestorStatus};
use edgion_resources::resources::edgion_backend_traffic_policy::{
    EdgionBackendTrafficPolicyStatus, PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::resources::upstream_authority::host_matches_pattern;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Service;
use std::collections::BTreeSet;

/// EdgionBackendTrafficPolicy-specific condition reasons.
///
/// This is an Edgion extension, not a Gateway API upstream reason.
/// `TargetNotFound` is the ResolvedRefs failure reason for either supported
/// policy target kind.
mod ebtp_reasons {
    /// The referenced target does not exist (or is not yet cached).
    pub const TARGET_NOT_FOUND: &str = "TargetNotFound";
}

#[derive(Default)]
struct PolicyEvaluation {
    targets: Vec<PolicyTargetKey>,
    missing_targets: BTreeSet<PolicyTargetKey>,
    errors: Vec<String>,
    target_class: Option<ResolvedPolicyTargetClass>,
}

/// Decide whether sibling policies (those targeting the same typed target) must be
/// requeued after this policy was processed.
///
/// Mirrors the `BackendTLSPolicy` handler: requeue only when a precedence-relevant
/// input changed (the target set, or the creation timestamp on a delete+recreate
/// within one watch tick). Skipping no-op updates prevents an A->B->A->B ping-pong
/// between conflicting policies that would otherwise reprocess each other forever.
///
/// Validity flips are folded into the target-set signal: `parse` registers conflict
/// keys only for a VALID spec, so `old_target_keys`/`new_target_keys` differ whenever
/// a policy flips valid<->invalid (an invalid policy contributes the empty set),
/// which requeues siblings so a stale `Conflicted` cannot survive the flip.
fn should_requeue_siblings(
    old_target_keys: &[String],
    new_target_keys: &[String],
    old_timestamp: Option<DateTime<Utc>>,
    new_timestamp: Option<DateTime<Utc>>,
) -> bool {
    old_target_keys != new_target_keys || old_timestamp != new_timestamp
}

/// A single BackendTLSPolicy's resource key, targetRefs, and `validation.hostname`,
/// snapshotted once per `validate()` call regardless of resource version.
struct BackendTlsPolicyRecord {
    key: String,
    target_refs: Vec<edgion_resources::resources::backend_tls_policy::BackendTLSPolicyTargetRef>,
    hostname: String,
}

/// Load every BackendTLSPolicy visible in the processor registry, v1 or v1alpha3.
///
/// The processor is registered under the single string key `"BackendTLSPolicy"` with a
/// **version-dependent concrete type** decided once at controller startup (see
/// `conf_center::kubernetes::controller`'s `BackendTLSPolicyVersion` match): a v1 cluster
/// stores `Arc<BackendTLSPolicy>`, a v1alpha3 cluster stores `Arc<BackendTLSPolicyAlpha3>`.
/// `Arc<dyn Any>::downcast` compares TypeIds, so a single hard-coded `downcast::<BackendTLSPolicy>()`
/// silently returns `None` on every key on a v1alpha3 cluster — the cross-object admission rule
/// would then see zero BackendTLSPolicies and never fire. Try both concrete types via
/// `BtpSpecView` instead of guessing one.
///
/// Empty when no BackendTLSPolicy processor is registered (test runtimes).
///
/// Mirrors `route_utils::service_exists`'s cross-kind cache lookup shape: reach the
/// BackendTLSPolicy processor through the type-erased `ProcessorObj` in
/// `ctx.runtime().processor_registry`, then `list_keys()` + `get_raw_any(..).downcast()` per key
/// (the registry has no typed "list all values" accessor — see `route_utils::lookup_gateway` for
/// the single-key form of the same pattern). Hoisted out of the per-targetRef loop in `validate()`
/// so the registry lookup and the full `list_keys()` scan happen once per `validate()` call, not
/// once per `EdgionBackendTrafficPolicy` targetRef.
fn load_backend_tls_policies(ctx: &HandlerContext) -> Vec<BackendTlsPolicyRecord> {
    use edgion_resources::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3};
    use edgion_resources::resources::backend_tls_policy::BtpSpecView;

    let Some(processor) = ctx.runtime().processor_registry.get("BackendTLSPolicy") else {
        return Vec::new();
    };

    processor
        .list_keys()
        .into_iter()
        .filter_map(|key| {
            let any = processor.get_raw_any(&key)?;
            match any.downcast::<BackendTLSPolicy>() {
                Ok(b) => Some(BackendTlsPolicyRecord {
                    key,
                    target_refs: b.spec.target_refs().to_vec(),
                    hostname: b.spec.validation().hostname.clone(),
                }),
                Err(any) => any
                    .downcast::<BackendTLSPolicyAlpha3>()
                    .ok()
                    .map(|b| BackendTlsPolicyRecord {
                        key,
                        target_refs: b.spec.target_refs().to_vec(),
                        hostname: b.spec.validation().hostname.clone(),
                    }),
            }
        })
        .collect()
}

/// Every `(policy key, validation.hostname)` pair for BackendTLSPolicies in `records` that
/// target `namespace/service` via a core-group `Service` targetRef. The group check mirrors
/// `EdgionBackendTrafficPolicySpec::get_validation_errors`'s own targetRef acceptance
/// (`edgion-resources/src/resources/edgion_backend_traffic_policy.rs`): `group.is_empty() ||
/// group == "core"`.
fn backend_tls_hostnames_for_service<'a>(
    namespace: &str,
    service: &str,
    records: &'a [BackendTlsPolicyRecord],
) -> Vec<(&'a str, &'a str)> {
    let prefix = format!("{namespace}/");
    records
        .iter()
        .filter(|r| r.key.starts_with(&prefix))
        .filter(|r| {
            r.target_refs
                .iter()
                .any(|tr| tr.kind == "Service" && (tr.group.is_empty() || tr.group == "core") && tr.name == service)
        })
        .map(|r| (r.key.as_str(), r.hostname.as_str()))
        .collect()
}

fn backend_tls_hostnames_for_edgion_backend<'a>(
    namespace: &str,
    backend: &str,
    records: &'a [BackendTlsPolicyRecord],
) -> Vec<(&'a str, &'a str)> {
    let prefix = format!("{namespace}/");
    records
        .iter()
        .filter(|record| record.key.starts_with(&prefix))
        .filter(|record| {
            record.target_refs.iter().any(|target_ref| {
                target_ref.kind == "EdgionBackend" && target_ref.group == "edgion.io" && target_ref.name == backend
            })
        })
        .map(|record| (record.key.as_str(), record.hostname.as_str()))
        .collect()
}

fn raw_service_exists(target: &PolicyTargetKey, ctx: &HandlerContext) -> bool {
    let Some(processor) = ctx.runtime().processor_registry.get("Service") else {
        return true;
    };
    let key = format!("{}/{}", target.namespace, target.name);
    processor
        .get_raw_any(&key)
        .and_then(|raw| raw.downcast::<Service>().ok())
        .is_some()
}

/// Complete, side-effect-free eligibility evaluation shared by validation,
/// publication staging, and status construction.
fn evaluate_policy(p: &EdgionBackendTrafficPolicy, ctx: &HandlerContext) -> PolicyEvaluation {
    let mut evaluation = PolicyEvaluation {
        errors: p.spec.get_validation_errors(),
        ..PolicyEvaluation::default()
    };
    let namespace = p.metadata.namespace.as_deref().unwrap_or("default");

    for target_ref in &p.spec.target_refs {
        let Ok(target) = target_ref.canonical_key(namespace) else {
            continue;
        };
        evaluation.targets.push(target);
    }

    if let Some(authority) = &p.spec.upstream_authority {
        let records = load_backend_tls_policies(ctx);
        for target in evaluation
            .targets
            .iter()
            .filter(|target| target.target_kind == PolicyTargetKind::Service)
        {
            for (key, hostname) in backend_tls_hostnames_for_service(namespace, &target.name, &records) {
                if !host_matches_pattern(hostname, &authority.pattern) {
                    evaluation.errors.push(format!(
                        "BackendTLSPolicy {} validation.hostname '{}' on Service {}/{} is outside upstreamAuthority.pattern '{}'",
                        key, hostname, namespace, target.name, authority.pattern
                    ));
                }
            }
        }
    }

    let mut class = None;
    let mut mixed_class = false;
    let mut observe_class = |observed| match class {
        None => class = Some(observed),
        Some(previous) if previous != observed => mixed_class = true,
        Some(_) => {}
    };
    for target in &evaluation.targets {
        match target.target_kind {
            PolicyTargetKind::Service => {
                if raw_service_exists(target, ctx) {
                    observe_class(ResolvedPolicyTargetClass::Service);
                } else {
                    evaluation.missing_targets.insert(target.clone());
                }
            }
            PolicyTargetKind::EdgionBackend => match classify_raw_backend(target, ctx) {
                Ok(Some(capability)) => {
                    observe_class(capability.target_class);
                }
                Ok(None) => {
                    evaluation.missing_targets.insert(target.clone());
                }
                Err(error) => evaluation.errors.push(error),
            },
        }
    }

    if mixed_class {
        evaluation
            .errors
            .push("targetRefs must resolve to one homogeneous target capability".to_string());
    }
    match class {
        Some(ResolvedPolicyTargetClass::Ai) => {
            if p.spec.load_balancer.is_some() {
                evaluation
                    .errors
                    .push("loadBalancer is not supported for AI EdgionBackend targets".to_string());
            }
            if p.spec.upstream_authority.is_some() {
                evaluation
                    .errors
                    .push("upstreamAuthority is not supported for AI EdgionBackend targets".to_string());
            }
        }
        Some(ResolvedPolicyTargetClass::A2aService) => {
            if p.spec
                .load_balancer
                .as_ref()
                .is_some_and(|load_balancer| load_balancer.panic_threshold.is_some())
            {
                evaluation
                    .errors
                    .push("loadBalancer.panicThreshold is not supported for A2A Service targets".to_string());
            }
            if p.spec.outlier_detection.is_some() {
                evaluation
                    .errors
                    .push("outlierDetection is not supported for A2A Service targets".to_string());
            }
            if let Some(authority) = &p.spec.upstream_authority {
                let records = load_backend_tls_policies(ctx);
                for target in evaluation
                    .targets
                    .iter()
                    .filter(|target| target.target_kind == PolicyTargetKind::EdgionBackend)
                {
                    for (key, hostname) in backend_tls_hostnames_for_edgion_backend(namespace, &target.name, &records) {
                        if !host_matches_pattern(hostname, &authority.pattern) {
                            evaluation.errors.push(format!(
                                "BackendTLSPolicy {} validation.hostname '{}' on EdgionBackend {}/{} is outside upstreamAuthority.pattern '{}'",
                                key, hostname, namespace, target.name, authority.pattern
                            ));
                        }
                    }
                }
            }
        }
        Some(ResolvedPolicyTargetClass::A2aExternal) => {
            if p.spec.load_balancer.is_some() {
                evaluation
                    .errors
                    .push("loadBalancer is not supported for A2A external targets".to_string());
            }
            if p.spec.outlier_detection.is_some() {
                evaluation
                    .errors
                    .push("outlierDetection is not supported for A2A external targets".to_string());
            }
            if p.spec.upstream_authority.is_some() {
                evaluation
                    .errors
                    .push("upstreamAuthority is not supported for A2A external targets".to_string());
            }
        }
        Some(ResolvedPolicyTargetClass::Service) | None => {}
    }
    if evaluation.errors.is_empty() && evaluation.missing_targets.is_empty() && !mixed_class {
        evaluation.target_class = class;
    }
    evaluation
}

/// EdgionBackendTrafficPolicy handler.
pub struct EdgionBackendTrafficPolicyHandler {
    /// Stamped into every `PolicyAncestorStatus.controller_name`; per
    /// GEP-2648 (ancestorRef, controllerName) is the entry's composite key,
    /// letting multiple controllers co-own one policy's status.
    controller_name: String,
}

impl EdgionBackendTrafficPolicyHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn resource_ref(p: &EdgionBackendTrafficPolicy) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            p.metadata.namespace.clone(),
            p.metadata.name.clone().unwrap_or_default(),
        )
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionBackendTrafficPolicy> for EdgionBackendTrafficPolicyHandler {
    fn validate(&self, p: &EdgionBackendTrafficPolicy, ctx: &HandlerContext) -> Vec<String> {
        let evaluation = evaluate_policy(p, ctx);
        let mut errors = evaluation.errors;
        errors.extend(
            evaluation
                .missing_targets
                .into_iter()
                .map(|target| format!("policy target {target} was not found")),
        );
        errors
    }

    async fn parse(
        &self,
        mut p: EdgionBackendTrafficPolicy,
        ctx: &HandlerContext,
    ) -> ProcessResult<EdgionBackendTrafficPolicy> {
        let resource_ref = Self::resource_ref(&p);
        let state = &ctx.runtime().edgion_backend_traffic_policy;

        // Snapshot precedence-relevant inputs BEFORE mutating any state so we can
        // detect whether the target set or the creation timestamp changed.
        let old_target_keys = {
            let mut keys = state.target_mgr().get_dependencies(&resource_ref.key());
            keys.sort();
            keys
        };
        let old_siblings = state.get_siblings(&resource_ref);
        let policy_resource_key = resource_ref.resource_key();
        let old_timestamp = state.timestamp_of(&policy_resource_key);

        // The transport marker is controller-owned and never trusted from any
        // input backend. Clear it before the complete current-generation stage.
        p.spec.resolved_target_class = None;

        // Clear old registrations, then re-register with the current targetRefs.
        // Two managers are updated:
        //   * `target_mgr` — conflict/precedence index (siblings, is_winner).
        //   * typed Service/backend ref manager — drives target-change requeue so
        //     ResolvedRefs re-evaluates when a target appears/disappears.
        state.target_mgr().clear_resource_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_resource_refs(&resource_ref);

        // Register every syntactically canonical dependency before any
        // existence/capability lookup so target creation can wake a policy
        // that is currently unresolved.
        let namespace = p.metadata.namespace.as_deref().unwrap_or("default");
        for target in p
            .spec
            .target_refs
            .iter()
            .filter_map(|target_ref| target_ref.canonical_key(namespace).ok())
        {
            match target.target_kind {
                PolicyTargetKind::Service => ctx
                    .runtime()
                    .service_ref_mgr
                    .add_ref(format!("{}/{}", target.namespace, target.name), resource_ref.clone()),
                PolicyTargetKind::EdgionBackend => ctx
                    .runtime()
                    .backend_ref_mgr
                    .add_ref(target.to_string(), resource_ref.clone()),
            }
        }

        let evaluation = evaluate_policy(&p, ctx);
        let mut new_target_keys = Vec::with_capacity(evaluation.targets.len());
        for target in &evaluation.targets {
            let target_key = target.to_string();
            // Conflict participation and the transport marker are all-or-none.
            if evaluation.target_class.is_some() {
                state.target_mgr().add_ref(target_key.clone(), resource_ref.clone());
                new_target_keys.push(target_key);
            }
        }
        p.spec.resolved_target_class = evaluation.target_class;
        new_target_keys.sort();
        new_target_keys.dedup();

        let new_timestamp = p.metadata.creation_timestamp.as_ref().map(|t| t.0);
        state.set_timestamp(&policy_resource_key, new_timestamp);

        if should_requeue_siblings(&old_target_keys, &new_target_keys, old_timestamp, new_timestamp) {
            for sibling in &old_siblings {
                ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
            }
            let new_siblings = state.get_siblings(&resource_ref);
            for sibling in &new_siblings {
                if !old_siblings.contains(sibling) {
                    ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
                }
            }
        }

        ProcessResult::Continue(p)
    }

    async fn on_change(&self, _p: &EdgionBackendTrafficPolicy, _ctx: &HandlerContext) {
        // Sibling requeue is handled in `parse` (gated on precedence-relevant
        // changes only) to avoid A->B->A->B ping-pong between conflicting policies.
    }

    async fn on_delete(&self, p: &EdgionBackendTrafficPolicy, ctx: &HandlerContext) {
        let resource_ref = Self::resource_ref(p);
        let state = &ctx.runtime().edgion_backend_traffic_policy;

        // Find siblings BEFORE clearing — a previously Conflicted sibling may now win.
        let siblings = state.get_siblings(&resource_ref);

        state.target_mgr().clear_resource_refs(&resource_ref);
        state.remove_timestamp(&resource_ref.resource_key());
        ctx.runtime().service_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_resource_refs(&resource_ref);

        tracing::debug!(
            policy = %resource_ref.key(),
            sibling_count = siblings.len(),
            "Cleared references on EdgionBackendTrafficPolicy delete"
        );

        for sibling in siblings {
            ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
        }
    }

    fn update_status(&self, p: &mut EdgionBackendTrafficPolicy, ctx: &HandlerContext, _validation_errors: &[String]) {
        let generation = p.metadata.generation;
        let policy_ns = p.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let policy_name = p.metadata.name.clone().unwrap_or_default();
        let policy_key = format!("{}/{}", policy_ns, policy_name);

        // GEP-2648: one status entry per ancestor Gateway. The resolver also
        // reports WHICH target Services reach each gateway, so Conflicted /
        // ResolvedRefs are scoped to the ancestors actually affected.
        let evaluation = evaluate_policy(p, ctx);
        let targets = evaluation.targets.clone();
        let ancestors_map = resolve_policy_ancestor_gateways_for_targets(&targets, &policy_key, ctx);

        // Per-target signals, computed once from typed canonical identities.
        //
        // Conflict participation is all-or-none and uses the same complete
        // evaluation as parse. Missing, incompatible, mixed, forbidden, and
        // cross-object-invalid policies publish no conflict keys.
        let lost_targets: BTreeSet<PolicyTargetKey> = if evaluation.target_class.is_some() {
            targets
                .iter()
                .filter_map(|target| {
                    let target_key = target.to_string();
                    (!ctx
                        .runtime()
                        .edgion_backend_traffic_policy
                        .is_winner(&policy_key, &target_key))
                    .then_some(target.clone())
                })
                .collect()
        } else {
            BTreeSet::new()
        };
        let missing_targets = evaluation.missing_targets.clone();

        let status = p.status.get_or_insert_with(EdgionBackendTrafficPolicyStatus::default);

        // Rebuild the ancestors list, carrying each surviving ancestor's prior
        // conditions forward so update_condition can keep lastTransitionTime
        // stable when the status does not actually flip (mirrors the
        // BackendTLSPolicy handler post-ffe29c28).
        let old_ancestors = std::mem::take(&mut status.ancestors);

        for (gw_key, targets) in &ancestors_map {
            let Some((gw_ns, gw_name)) = gw_key.split_once('/') else {
                continue;
            };
            let conflicted_here: Vec<String> = targets
                .iter()
                .filter(|target| lost_targets.contains(*target))
                .map(ToString::to_string)
                .collect();
            let missing_here: Vec<String> = targets
                .iter()
                .filter(|target| missing_targets.contains(*target))
                .map(ToString::to_string)
                .collect();

            // Match on controller_name + kind as well: in K8s mode the watched
            // status.ancestors may hold entries from other controllers (or
            // GAMMA Service ancestors sharing ns/name) whose conditions must
            // not be adopted. EBTP always stamps kind: Some("Gateway"),
            // namespace: Some(gw_ns).
            let mut conditions = old_ancestors
                .iter()
                .find(|a| {
                    a.controller_name == self.controller_name
                        && a.ancestor_ref.kind.as_deref() == Some("Gateway")
                        && a.ancestor_ref.namespace.as_deref() == Some(gw_ns)
                        && a.ancestor_ref.name == gw_name
                })
                .map(|a| a.conditions.clone())
                .unwrap_or_default();

            // Accepted precedence is missing > invalid > conflict > accepted.
            // Missing is reported through ResolvedRefs while Accepted remains
            // true; GEP-2648 folds conflict into Accepted=False/Conflicted.
            let accepted = if !missing_here.is_empty() {
                condition_true(
                    condition_types::ACCEPTED,
                    condition_reasons::ACCEPTED,
                    "Policy accepted pending target resolution",
                    generation,
                )
            } else if !evaluation.errors.is_empty() {
                condition_false(
                    condition_types::ACCEPTED,
                    condition_reasons::INVALID,
                    bounded_status_message(evaluation.errors.join("; ")),
                    generation,
                )
            } else if !conflicted_here.is_empty() {
                condition_false(
                    condition_types::ACCEPTED,
                    condition_reasons::POLICY_CONFLICTED,
                    bounded_status_message(format!(
                        "Conflicts with an older policy targeting: {}",
                        conflicted_here.join(", ")
                    )),
                    generation,
                )
            } else {
                condition_true(
                    condition_types::ACCEPTED,
                    condition_reasons::ACCEPTED,
                    "Policy accepted",
                    generation,
                )
            };
            update_condition(&mut conditions, accepted);

            // ResolvedRefs — existence of the targets reaching THIS
            // ancestor (a route may reference a Service that does not exist).
            let resolved = if missing_here.is_empty() {
                condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "All targets resolved",
                    generation,
                )
            } else {
                condition_false(
                    condition_types::RESOLVED_REFS,
                    ebtp_reasons::TARGET_NOT_FOUND,
                    format!("target(s) not found: {}", missing_here.join(", ")),
                    generation,
                )
            };
            update_condition(&mut conditions, resolved);

            status.ancestors.push(PolicyAncestorStatus {
                ancestor_ref: ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(gw_ns.to_string()),
                    name: gw_name.to_string(),
                    section_name: None,
                    port: None,
                },
                controller_name: self.controller_name.clone(),
                conditions,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use serde_json::json;
    use std::sync::Arc;

    const TEST_CONTROLLER: &str = "test.edgion.io/controller";

    fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

    fn make_handler() -> EdgionBackendTrafficPolicyHandler {
        EdgionBackendTrafficPolicyHandler::new(TEST_CONTROLLER.to_string())
    }

    /// Wire "route (ns/route) -> gateway (ns/gw)" and "svc -> route" so the
    /// ancestor resolver finds one ancestor Gateway for the Service.
    fn wire_service_to_gateway(rt: &ControllerRuntime, ns: &str, svc: &str, route: &str, gw: &str) {
        rt.service_ref_mgr.add_ref(
            format!("{ns}/{svc}"),
            ResourceRef::new(
                edgion_resources::ResourceKind::HTTPRoute,
                Some(ns.to_string()),
                route.to_string(),
            ),
        );
        rt.gateway_route_index.update_route(
            edgion_resources::ResourceKind::HTTPRoute,
            &format!("{ns}/{route}"),
            &[ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(ns.to_string()),
                name: gw.to_string(),
                section_name: None,
                port: None,
            }],
            ns,
        );
    }

    fn wire_backend_to_gateway(rt: &ControllerRuntime, ns: &str, ai: &str, route: &str, gw: &str) {
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: ns.to_string(),
            name: ai.to_string(),
        };
        rt.backend_ref_mgr.add_ref(
            target.to_string(),
            ResourceRef::new(ResourceKind::HTTPRoute, Some(ns.to_string()), route.to_string()),
        );
        rt.gateway_route_index.update_route(
            ResourceKind::HTTPRoute,
            &format!("{ns}/{route}"),
            &[ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(ns.to_string()),
                name: gw.to_string(),
                section_name: None,
                port: None,
            }],
            ns,
        );
    }

    /// Condition of type `type_` on the ancestor whose gateway name is `gw`.
    fn ancestor_condition<'a>(
        p: &'a EdgionBackendTrafficPolicy,
        gw: &str,
        type_: &str,
    ) -> &'a edgion_resources::resources::common::Condition {
        p.status
            .as_ref()
            .map(|s| s.ancestors.as_slice())
            .unwrap_or_default()
            .iter()
            .find(|a| a.ancestor_ref.name == gw)
            .unwrap_or_else(|| panic!("ancestor for gateway {gw} present"))
            .conditions
            .iter()
            .find(|c| c.type_ == type_)
            .unwrap_or_else(|| panic!("condition {type_} present on ancestor {gw}"))
    }

    fn make_policy(name: &str, target_svc: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": {
                "name": name,
                "namespace": "ns",
                "creationTimestamp": creation_ts,
            },
            "spec": {
                "targetRefs": [ { "kind": "Service", "name": target_svc } ],
            },
        }))
        .expect("valid EdgionBackendTrafficPolicy JSON")
    }

    fn make_ai_policy(name: &str, target: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": {
                "name": name,
                "namespace": "ns",
                "creationTimestamp": creation_ts,
            },
            "spec": {
                "targetRefs": [
                    { "group": "edgion.io", "kind": "EdgionBackend", "name": target }
                ],
                "connection": { "connectTimeout": "2s" },
                "retryConstraint": {},
                "circuitBreaker": { "maxParallelRequests": 20 },
                "outlierDetection": { "consecutiveErrors": 5 }
            },
        }))
        .expect("valid AI-target EdgionBackendTrafficPolicy JSON")
    }

    fn install_raw_ai_backend(runtime: &Arc<ControllerRuntime>, name: &str) {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let backend: EdgionBackend = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": "ns", "name": name},
            "spec": {"ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }}
        }))
        .unwrap();
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert(format!("ns/{name}"), Arc::new(backend));
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);
    }

    #[tokio::test]
    async fn same_named_service_and_backend_do_not_conflict() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "shared", "service-route", "gw");
        wire_backend_to_gateway(&rt, "ns", "shared", "ai-route", "gw");

        let service_policy = make_policy("service-policy", "shared", "2026-01-01T00:00:00Z");
        let ai_policy = make_ai_policy("ai-policy", "shared", "2026-06-01T00:00:00Z");
        let _ = handler.parse(service_policy.clone(), &ctx).await;
        let _ = handler.parse(ai_policy.clone(), &ctx).await;

        for mut policy in [service_policy, ai_policy] {
            handler.update_status(&mut policy, &ctx, &[]);
            let accepted = ancestor_condition(&policy, "gw", condition_types::ACCEPTED);
            assert_eq!(accepted.status, "True");
            assert_eq!(accepted.reason, condition_reasons::ACCEPTED);
        }
    }

    #[tokio::test]
    async fn backend_policies_conflict_and_resolve_ancestors_by_typed_key() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        install_raw_ai_backend(&rt, "provider");
        wire_backend_to_gateway(&rt, "ns", "provider", "ai-route", "gw-ai");

        let older = make_ai_policy("older", "provider", "2026-01-01T00:00:00Z");
        let younger = make_ai_policy("younger", "provider", "2026-06-01T00:00:00Z");
        let _ = handler.parse(older, &ctx).await;
        let _ = handler.parse(younger.clone(), &ctx).await;

        let mut younger = younger;
        handler.update_status(&mut younger, &ctx, &[]);
        let accepted = ancestor_condition(&younger, "gw-ai", condition_types::ACCEPTED);
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
        assert!(accepted.message.contains("edgion.io/EdgionBackend/ns/provider"));
    }

    #[tokio::test]
    async fn missing_backend_sets_target_not_found() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::prelude_resources::EdgionBackend;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_backend_to_gateway(&rt, "ns", "missing", "ai-route", "gw-ai");
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            128,
            Arc::new(EdgionBackendHandler::new()),
            rt.clone(),
        ));
        rt.processor_registry.register(processor);

        let policy = make_ai_policy("policy", "missing", "2026-01-01T00:00:00Z");
        let _ = handler.parse(policy.clone(), &ctx).await;
        let mut policy = policy;
        handler.update_status(&mut policy, &ctx, &[]);

        let resolved = ancestor_condition(&policy, "gw-ai", condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, ebtp_reasons::TARGET_NOT_FOUND);
        assert!(resolved.message.contains("edgion.io/EdgionBackend/ns/missing"));
    }

    #[tokio::test]
    async fn forged_resolved_target_class_is_cleared_before_staging() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            128,
            Arc::new(EdgionBackendHandler::new()),
            rt.clone(),
        ));
        rt.processor_registry.register(processor);

        let mut forged = make_ai_policy("forged", "missing", "2026-01-01T00:00:00Z");
        forged.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Ai);
        let parsed = match handler.parse(forged, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        assert!(parsed.spec.resolved_target_class.is_none());
        assert!(rt
            .edgion_backend_traffic_policy
            .target_mgr()
            .get_dependencies(&EdgionBackendTrafficPolicyHandler::resource_ref(&parsed).key())
            .is_empty());
    }

    #[tokio::test]
    async fn complete_ai_policy_sets_resolved_target_class() {
        let (ctx, rt) = make_ctx();
        install_raw_ai_backend(&rt, "provider");
        let handler = make_handler();
        let policy = make_ai_policy("complete", "provider", "2026-01-01T00:00:00Z");
        let parsed = match handler.parse(policy, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(parsed.spec.resolved_target_class, Some(ResolvedPolicyTargetClass::Ai));
    }

    #[tokio::test]
    async fn a2a_service_policy_uses_raw_generation_and_clears_invalid_staging() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let (ctx, runtime) = make_ctx();
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw_a2a: EdgionBackend = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"name": "agent", "namespace": "ns"},
            "spec": {
                "a2a": {"target": {"serviceRef": {"name": "agent-svc", "port": 8080}}}
            }
        }))
        .unwrap();
        let processed_ai: EdgionBackend = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"name": "agent", "namespace": "ns"},
            "spec": {"ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }}
        }))
        .unwrap();
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert("ns/agent".to_string(), Arc::new(raw_a2a));
        processor.set_raw_store(raw.clone());
        processor.save(processed_ai, None);
        runtime.processor_registry.register(processor);

        let handler = make_handler();
        let mut policy = make_ai_policy("a2a-service", "agent", "2026-01-01T00:00:00Z");
        policy.spec.outlier_detection = None;
        let parsed = match handler.parse(policy.clone(), &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(
            parsed.spec.resolved_target_class,
            Some(ResolvedPolicyTargetClass::A2aService)
        );

        let policy_key = EdgionBackendTrafficPolicyHandler::resource_ref(&parsed).key();
        assert_eq!(
            runtime
                .edgion_backend_traffic_policy
                .target_mgr()
                .get_dependencies(&policy_key),
            vec!["edgion.io/EdgionBackend/ns/agent".to_string()]
        );

        let mut invalid = policy.clone();
        invalid.spec.outlier_detection = serde_json::from_value(json!({"consecutiveErrors": 3})).unwrap();
        assert!(handler
            .validate(&invalid, &ctx)
            .iter()
            .any(|error| error.contains("outlierDetection is not supported for A2A Service")));

        // A new invalid raw generation must withdraw the controller-owned
        // marker and conflict dependency even though ServerCache still holds
        // the older, valid AI generation.
        for invalid_raw in [
            json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionBackend",
                "metadata": {"name": "agent", "namespace": "ns"},
                "spec": {}
            }),
            json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionBackend",
                "metadata": {"name": "agent", "namespace": "ns"},
                "spec": {
                    "ai": {
                        "provider": "OpenAI",
                        "credentialPool": {"credentials": [{
                            "name": "primary", "secretRef": {"name": "provider-key"}
                        }]},
                        "models": [{"name": "model-a"}]
                    },
                    "a2a": {"target": {"endpoint": "https://agent.example.test"}}
                }
            }),
            json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionBackend",
                "metadata": {"name": "agent", "namespace": "ns"},
                "spec": {"a2a": {"target": {
                    "serviceRef": {"name": "agent-svc", "port": 8080},
                    "endpoint": "https://agent.example.test"
                }}}
            }),
        ] {
            let invalid_backend: EdgionBackend = serde_json::from_value(invalid_raw).unwrap();
            raw.upsert("ns/agent".to_string(), Arc::new(invalid_backend));
            let ProcessResult::Continue(withdrawn) = handler.parse(policy.clone(), &ctx).await else {
                panic!("invalid target policy should continue with withdrawn staging");
            };
            assert!(withdrawn.spec.resolved_target_class.is_none());
            assert!(runtime
                .edgion_backend_traffic_policy
                .target_mgr()
                .get_dependencies(&policy_key)
                .is_empty());
        }
    }

    /// `validate()` returns the spec's validation errors verbatim.
    #[test]
    fn validate_returns_spec_validation_errors_verbatim() {
        let (ctx, _rt) = make_ctx();
        let handler = make_handler();

        // Empty targetRefs — spec is invalid.
        let mut invalid = make_policy("bad", "svc", "2026-01-01T00:00:00Z");
        invalid.spec.target_refs.clear();
        let expected = invalid.spec.get_validation_errors();
        assert!(!expected.is_empty(), "empty targetRefs must be a validation error");
        assert_eq!(handler.validate(&invalid, &ctx), expected);

        // Valid spec — no errors.
        let valid = make_policy("good", "svc", "2026-01-01T00:00:00Z");
        assert!(handler.validate(&valid, &ctx).is_empty());
    }

    /// Three policies target the same Service with mixed timestamps: the winner is
    /// the `precedence_key` minimum (oldest); the two losers are flagged
    /// `Accepted=False/Conflicted` on the ancestor the shared Service reaches.
    #[tokio::test]
    async fn conflict_evaluation_oldest_wins_others_flagged() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

        // `b` is the oldest (min precedence_key), so it wins; `a` and `c` lose.
        let a = make_policy("a", "payments", "2026-03-01T00:00:00Z");
        let b = make_policy("b", "payments", "2026-01-01T00:00:00Z");
        let c = make_policy("c", "payments", "2026-06-01T00:00:00Z");

        for p in [a.clone(), b.clone(), c.clone()] {
            let _ = handler.parse(p, &ctx).await;
        }

        let check = |mut p: EdgionBackendTrafficPolicy, expect_conflicted: bool| {
            handler.update_status(&mut p, &ctx, &[]);
            let accepted = ancestor_condition(&p, "gw-1", condition_types::ACCEPTED);
            if expect_conflicted {
                assert_eq!(
                    accepted.status,
                    "False",
                    "{} should be conflicted",
                    p.metadata.name.as_deref().unwrap()
                );
                assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
            } else {
                assert_eq!(accepted.status, "True", "winner must not be conflicted");
                assert_eq!(accepted.reason, condition_reasons::ACCEPTED);
            }
        };

        check(b, false); // oldest wins
        check(a, true); // loser
        check(c, true); // loser
    }

    /// Carry-forward: re-running `update_status` on an unchanged world must keep
    /// each surviving ancestor's condition `lastTransitionTime` stable, and a
    /// state flip must move only the condition that actually flipped. Uses a
    /// sentinel timestamp because `now_rfc3339` is second-granularity, so two
    /// fast consecutive runs would otherwise collide and mask a lost carry-forward.
    #[tokio::test]
    async fn condition_last_transition_time_carried_forward() {
        const SENTINEL: &str = "1970-01-01T00:00:00Z";

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

        // `p` alone is the winner (Accepted=True, ResolvedRefs=True).
        let p = make_policy("p", "payments", "2026-06-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;

        let mut p = p;
        handler.update_status(&mut p, &ctx, &[]);
        // Stamp every condition with a sentinel so a lost carry-forward (fresh
        // `now_rfc3339`) is visibly distinguishable from a preserved timestamp.
        for a in &mut p.status.as_mut().unwrap().ancestors {
            for c in &mut a.conditions {
                c.last_transition_time = SENTINEL.to_string();
            }
        }

        // Unchanged world: both conditions keep the sentinel (carry-forward +
        // no status flip). A `Vec::new()`-per-ancestor rebuild would drop the
        // sentinel and restamp with the current time.
        handler.update_status(&mut p, &ctx, &[]);
        assert_eq!(
            ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).last_transition_time,
            SENTINEL,
            "unchanged Accepted must keep its lastTransitionTime"
        );
        assert_eq!(
            ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS).last_transition_time,
            SENTINEL,
            "unchanged ResolvedRefs must keep its lastTransitionTime"
        );

        // Introduce an older valid policy targeting the same Service: `p` now
        // loses arbitration, so its Accepted condition flips True -> False.
        let older = make_policy("older", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(older, &ctx).await;

        handler.update_status(&mut p, &ctx, &[]);
        assert_ne!(
            ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).last_transition_time,
            SENTINEL,
            "flipped Accepted must get a fresh lastTransitionTime"
        );
        assert_eq!(
            ancestor_condition(&p, "gw-1", condition_types::ACCEPTED).reason,
            condition_reasons::POLICY_CONFLICTED
        );
        assert_eq!(
            ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS).last_transition_time,
            SENTINEL,
            "non-flipped ResolvedRefs must still keep its lastTransitionTime"
        );
    }

    #[tokio::test]
    async fn mixed_policy_registers_dependencies_but_publishes_no_subset() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        let policy: EdgionBackendTrafficPolicy = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": { "name": "mixed", "namespace": "ns" },
            "spec": {
                "targetRefs": [
                    { "kind": "Service", "name": "service" },
                    { "group": "edgion.io", "kind": "EdgionBackend", "name": "backend" }
                ],
                "connection": { "connectTimeout": "2s" }
            }
        }))
        .expect("mixed policy fixture parses under lenient admission");
        let parsed = match handler.parse(policy, &ctx).await {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let owner = EdgionBackendTrafficPolicyHandler::resource_ref(&parsed);

        assert!(parsed.spec.resolved_target_class.is_none());
        assert!(rt
            .edgion_backend_traffic_policy
            .target_mgr()
            .get_dependencies(&owner.key())
            .is_empty());
        assert_eq!(rt.service_ref_mgr.get_dependencies(&owner.key()).len(), 1);
        assert_eq!(rt.backend_ref_mgr.get_dependencies(&owner.key()).len(), 1);
    }

    /// Build an INVALID policy (ConsistentHash without a `consistentHash` section)
    /// targeting `target_svc` — mirrors `Accepted=False/Invalid` admission.
    fn make_invalid_policy(name: &str, target_svc: &str, creation_ts: &str) -> EdgionBackendTrafficPolicy {
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": {
                "name": name,
                "namespace": "ns",
                "creationTimestamp": creation_ts,
            },
            "spec": {
                "targetRefs": [ { "kind": "Service", "name": target_svc } ],
                "loadBalancer": { "type": "ConsistentHash" },
            },
        }))
        .expect("valid EdgionBackendTrafficPolicy JSON")
    }

    /// Controller conflict participation must match the gateway: an INVALID older
    /// policy is a data-plane no-op, so it must NOT compete for oldest-wins. A VALID
    /// newer policy targeting the same Service therefore wins (`Accepted=True`).
    /// When the older policy is later edited to become valid, the newer one flips to
    /// `Accepted=False/Conflicted`.
    #[tokio::test]
    async fn invalid_older_policy_does_not_conflict_valid_newer() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

        // A is older but INVALID; B is newer and valid; both target `payments`.
        let a_invalid = make_invalid_policy("a", "payments", "2026-01-01T00:00:00Z");
        let b = make_policy("b", "payments", "2026-06-01T00:00:00Z");
        assert!(
            !a_invalid.spec.get_validation_errors().is_empty(),
            "A fixture must be invalid"
        );
        assert!(b.spec.get_validation_errors().is_empty(), "B fixture must be valid");

        let _ = handler.parse(a_invalid, &ctx).await;
        let _ = handler.parse(b.clone(), &ctx).await;

        // B wins: the invalid older policy does not participate in the conflict index.
        let mut b_status = b.clone();
        handler.update_status(&mut b_status, &ctx, &[]);
        let accepted = ancestor_condition(&b_status, "gw-1", condition_types::ACCEPTED);
        assert_eq!(
            accepted.status, "True",
            "valid newer policy must win over an invalid older one"
        );
        assert_eq!(accepted.reason, condition_reasons::ACCEPTED);

        // Edit A to a valid spec (update path) — it now participates and, being older,
        // wins; B must flip to Accepted=False/Conflicted.
        let a_valid = make_policy("a", "payments", "2026-01-01T00:00:00Z");
        assert!(a_valid.spec.get_validation_errors().is_empty());
        let _ = handler.parse(a_valid, &ctx).await;

        let mut b_status = b.clone();
        handler.update_status(&mut b_status, &ctx, &[]);
        let accepted = ancestor_condition(&b_status, "gw-1", condition_types::ACCEPTED);
        assert_eq!(
            accepted.status, "False",
            "now-valid older policy wins; newer becomes conflicted"
        );
        assert_eq!(accepted.reason, condition_reasons::POLICY_CONFLICTED);
    }

    #[tokio::test]
    async fn duplicate_service_targets_do_not_requeue_unchanged_siblings() {
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use std::sync::atomic::Ordering;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        let processor = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            128,
            Arc::new(make_handler()),
            rt.clone(),
        ));
        rt.processor_registry.register(processor.clone());

        let duplicate_target = |mut policy: EdgionBackendTrafficPolicy| {
            policy.spec.target_refs.push(policy.spec.target_refs[0].clone());
            policy
        };
        let older = duplicate_target(make_policy("older", "payments", "2026-01-01T00:00:00Z"));
        let younger = duplicate_target(make_policy("younger", "payments", "2026-06-01T00:00:00Z"));
        assert!(older.spec.get_validation_errors().is_empty());
        assert!(younger.spec.get_validation_errors().is_empty());

        let _ = handler.parse(older.clone(), &ctx).await;
        let _ = handler.parse(younger, &ctx).await;
        let scheduled_after_initial_arbitration = processor.workqueue().metrics().delayed_total.load(Ordering::Relaxed);

        let _ = handler.parse(older, &ctx).await;

        assert_eq!(
            processor.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
            scheduled_after_initial_arbitration,
            "an unchanged duplicate Service target set must not requeue its sibling"
        );
    }

    /// ResolvedRefs is `False`/`TargetNotFound` when the target Service is not in
    /// the cache (no Service processor registered in the test runtime → `service_exists`
    /// returns `true`, so we assert the resolved branch here) — the policy still syncs.
    #[tokio::test]
    async fn resolved_refs_true_when_service_lookup_unavailable() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");
        let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;
        let mut p = p;
        handler.update_status(&mut p, &ctx, &[]);
        let resolved = ancestor_condition(&p, "gw-1", condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "True");
        assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
    }

    /// With a Service processor registered but its cache EMPTY, `service_exists`
    /// genuinely returns `false`, so ResolvedRefs is `False`/`TargetNotFound`.
    /// Adding the target Service to the cache and re-running flips it to `True`.
    #[tokio::test]
    async fn resolved_refs_false_then_true_when_service_appears() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use k8s_openapi::api::core::v1::Service;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

        // Register a Service processor with an empty cache: now
        // `route_utils::service_exists` consults it (instead of the no-processor
        // short-circuit that returns true) and finds the target absent.
        let svc_proc = Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            128,
            Arc::new(ServiceHandler::new()),
            rt.clone(),
        ));
        rt.processor_registry.register(svc_proc.clone());
        let raw = Arc::new(LocalRawStore::<Service>::new());
        svc_proc.set_raw_store(raw.clone());

        let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;

        // Target Service not in cache -> ResolvedRefs=False/TargetNotFound.
        let mut p_missing = p.clone();
        handler.update_status(&mut p_missing, &ctx, &[]);
        let resolved = ancestor_condition(&p_missing, "gw-1", condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False", "absent Service => ResolvedRefs False");
        assert_eq!(resolved.reason, ebtp_reasons::TARGET_NOT_FOUND);

        // Add the Service to the cache (key = "ns/payments") and re-run: flips True.
        let svc = Service {
            metadata: ObjectMeta {
                name: Some("payments".to_string()),
                namespace: Some("ns".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        raw.upsert("ns/payments".to_string(), Arc::new(svc.clone()));
        svc_proc.save(svc, None);

        let mut p_present = p.clone();
        handler.update_status(&mut p_present, &ctx, &[]);
        let resolved = ancestor_condition(&p_present, "gw-1", condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "True", "Service now present => ResolvedRefs True");
        assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
    }

    /// An invalid spec is admitted leniently: Accepted=False/Invalid but the
    /// resource still produces a status (it will still sync to the gateway).
    ///
    /// Deviation from a literal translation of the pre-GEP-2648 fixture: the
    /// original cleared `target_refs` entirely to make the spec invalid, but
    /// under GEP-2648 zero targetRefs resolves zero ancestor Gateways (there
    /// is no Service to walk Service->Route->Gateway through), so
    /// `status.ancestors` would legitimately be empty and there would be no
    /// ancestor to assert on. Using `make_invalid_policy` instead keeps a real
    /// targetRef (`svc`) while still producing a spec-local validation error
    /// (ConsistentHash without a `consistentHash` section), so an ancestor can
    /// be wired and the Accepted=False/Invalid condition asserted on it.
    #[test]
    fn invalid_spec_is_accepted_false_invalid() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "svc", "r1", "gw-1");
        let mut p = make_invalid_policy("bad", "svc", "2026-01-01T00:00:00Z");
        let errors = handler.validate(&p, &ctx);
        assert!(!errors.is_empty(), "fixture must be spec-locally invalid");
        handler.update_status(&mut p, &ctx, &errors);
        let accepted = ancestor_condition(&p, "gw-1", condition_types::ACCEPTED);
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, condition_reasons::INVALID);
    }

    /// A BackendTLSPolicy targeting the same Service with a hostname outside the
    /// authority pattern is an admission error: that hostname is either a stale SNI
    /// or a certificate that cannot cover the tenant subdomains.
    #[tokio::test]
    async fn backend_tls_hostname_outside_pattern_is_rejected() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::BackendTLSPolicy;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        install_raw_ai_backend(&rt, "sandbox-gw");

        let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
            "BackendTLSPolicy",
            128,
            Arc::new(BackendTlsPolicyHandler::new(
                crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
            )),
            rt.clone(),
        ));
        rt.processor_registry.register(btls_proc.clone());

        // `backend_tls_hostnames_for_service` mirrors `RuntimeDelegationLookup::get_route`
        // (http_route.rs): `list_keys()`/visibility come from the ServerCache (`save`),
        // but the typed value comes from the raw store (`get_raw_any`), so both must be
        // populated for the lookup to see the fixture.
        let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
        btls_proc.set_raw_store(btls_raw.clone());

        let btls: BackendTLSPolicy = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3",
            "kind": "BackendTLSPolicy",
            "metadata": { "name": "tls", "namespace": "ns" },
            "spec": {
                "targetRefs": [ { "group": "", "kind": "Service", "name": "sandbox-gw" } ],
                "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
            },
        }))
        .expect("valid BackendTLSPolicy JSON");
        btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
        btls_proc.save(btls, None);

        let mut p = make_policy("p", "sandbox-gw", "2026-01-01T00:00:00Z");
        p.spec.upstream_authority = Some(
            edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: None,
            },
        );

        let errors = handler.validate(&p, &ctx);
        assert_eq!(errors.len(), 1, "got: {errors:?}");
        assert!(errors[0].contains("stale.other.com"), "got: {}", errors[0]);
        assert!(errors[0].contains("*.sandbox.internal"), "got: {}", errors[0]);
        // Minor 1: the offending BackendTLSPolicy's resource key must be named in the
        // error so an operator with several BackendTLSPolicies on one Service can tell
        // which object to fix.
        assert!(errors[0].contains("ns/tls"), "got: {}", errors[0]);

        // The cross-object hostname rule is Service-only. An AI-target policy
        // already rejects upstreamAuthority through its field matrix, but must
        // not additionally inspect a same-named Service's BackendTLSPolicy.
        let mut ai_policy = make_ai_policy("ai", "sandbox-gw", "2026-01-01T00:00:00Z");
        ai_policy.spec.upstream_authority = p.spec.upstream_authority.clone();
        let ai_errors = handler.validate(&ai_policy, &ctx);
        assert_eq!(ai_errors.len(), 1, "got: {ai_errors:?}");
        assert!(ai_errors[0].contains("not supported for AI EdgionBackend"));
        assert!(!ai_errors[0].contains("stale.other.com"));

        // An in-pattern hostname is accepted.
        let btls_ok: BackendTLSPolicy = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3",
            "kind": "BackendTLSPolicy",
            "metadata": { "name": "tls", "namespace": "ns" },
            "spec": {
                "targetRefs": [ { "group": "", "kind": "Service", "name": "sandbox-gw" } ],
                "validation": { "hostname": "default.sandbox.internal", "wellKnownCACertificates": "System" }
            },
        }))
        .expect("valid BackendTLSPolicy JSON");
        btls_raw.upsert("ns/tls".to_string(), Arc::new(btls_ok.clone()));
        btls_proc.save(btls_ok, None);
        assert!(handler.validate(&p, &ctx).is_empty());
    }

    /// CRITICAL regression test: on a v1alpha3 cluster the BackendTLSPolicy processor is
    /// registered under the same string key `"BackendTLSPolicy"` but with the CONCRETE type
    /// `BackendTLSPolicyAlpha3` (see `conf_center::kubernetes::controller`'s
    /// `BackendTLSPolicyVersion` match, which spawns `ResourceProcessor::<BackendTLSPolicyAlpha3>`
    /// under that key). Before the fix, `backend_tls_hostnames_for_service` did a single
    /// `downcast::<BackendTLSPolicy>()`, which fails TypeId comparison against
    /// `Arc<BackendTLSPolicyAlpha3>` and silently returns `None` for every key — the rule never
    /// fires on any v1alpha3 cluster. This test registers a `BackendTLSPolicyAlpha3` processor
    /// under `"BackendTLSPolicy"` and asserts the cross-object rule still catches an
    /// out-of-pattern hostname.
    #[tokio::test]
    async fn backend_tls_hostname_outside_pattern_is_rejected_on_v1alpha3_cluster() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyAlpha3Handler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::BackendTLSPolicyAlpha3;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();

        let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicyAlpha3>::new(
            "BackendTLSPolicy",
            128,
            Arc::new(BackendTlsPolicyAlpha3Handler::new(
                crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
            )),
            rt.clone(),
        ));
        rt.processor_registry.register(btls_proc.clone());

        let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicyAlpha3>::new());
        btls_proc.set_raw_store(btls_raw.clone());

        let btls: BackendTLSPolicyAlpha3 = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3",
            "kind": "BackendTLSPolicy",
            "metadata": { "name": "tls", "namespace": "ns" },
            "spec": {
                "targetRefs": [ { "group": "", "kind": "Service", "name": "sandbox-gw" } ],
                "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
            },
        }))
        .expect("valid BackendTLSPolicyAlpha3 JSON");
        btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
        btls_proc.save(btls, None);

        let mut p = make_policy("p", "sandbox-gw", "2026-01-01T00:00:00Z");
        p.spec.upstream_authority = Some(
            edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: None,
            },
        );

        let errors = handler.validate(&p, &ctx);
        assert_eq!(
            errors.len(),
            1,
            "on a v1alpha3 cluster the cross-object rule must still see the BackendTLSPolicyAlpha3 \
             registered under the shared \"BackendTLSPolicy\" processor key; got: {errors:?}"
        );
        assert!(errors[0].contains("stale.other.com"), "got: {}", errors[0]);
        assert!(errors[0].contains("*.sandbox.internal"), "got: {}", errors[0]);
        assert!(errors[0].contains("ns/tls"), "got: {}", errors[0]);
    }

    /// Minor 2: only core-group `Service` targetRefs are considered, mirroring
    /// `EdgionBackendTrafficPolicySpec::get_validation_errors`'s own targetRef acceptance.
    /// A BackendTLSPolicy whose targetRef has a non-core group must NOT be treated as
    /// targeting the Service, even if `kind` and `name` match.
    #[tokio::test]
    async fn backend_tls_policy_with_non_core_group_targetref_is_ignored() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::BackendTLSPolicy;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();

        let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
            "BackendTLSPolicy",
            128,
            Arc::new(BackendTlsPolicyHandler::new(
                crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
            )),
            rt.clone(),
        ));
        rt.processor_registry.register(btls_proc.clone());

        let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
        btls_proc.set_raw_store(btls_raw.clone());

        // Non-core group targetRef with a hostname that WOULD be rejected if it were
        // considered — proves the group filter, not just an absent-target coincidence.
        let btls: BackendTLSPolicy = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "BackendTLSPolicy",
            "metadata": { "name": "tls", "namespace": "ns" },
            "spec": {
                "targetRefs": [ { "group": "example.io", "kind": "Service", "name": "sandbox-gw" } ],
                "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
            },
        }))
        .expect("valid BackendTLSPolicy JSON");
        btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
        btls_proc.save(btls, None);

        let mut p = make_policy("p", "sandbox-gw", "2026-01-01T00:00:00Z");
        p.spec.upstream_authority = Some(
            edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: None,
            },
        );

        assert!(
            handler.validate(&p, &ctx).is_empty(),
            "a non-core-group targetRef must not be treated as targeting the Service"
        );
    }

    /// IMPORTANT regression test: a policy that is spec-locally VALID but fails the
    /// cross-object `upstreamAuthority` rule must still participate in the conflict index
    /// (its target keys are registered in `parse`, gated on `spec.get_validation_errors()`).
    ///
    /// Priority within `Accepted` is Invalid > Conflicted > Accepted, and cross-object
    /// errors land in the widened `validation_errors` list, so while those errors are
    /// present the reported reason is `Invalid` (by design — this MASKS `Conflicted`).
    /// The test asserts that masking, then proves the policy still *participates* in
    /// arbitration underneath the mask: re-running `update_status` with the cross-object
    /// errors stripped away flips the reason to `Conflicted`, so a
    /// conflicted-and-cross-object-invalid policy is never silently reported as a plain
    /// winner once its cross-object error is fixed independently of the conflict.
    #[tokio::test]
    async fn cross_object_validation_cannot_be_bypassed_by_forged_empty_errors() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::BackendTLSPolicy;

        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "r1", "gw-1");

        // Register a BackendTLSPolicy whose hostname is OUTSIDE the pattern used below, so the
        // younger EBTP's `validate()` produces a cross-object error.
        let btls_proc = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
            "BackendTLSPolicy",
            128,
            Arc::new(BackendTlsPolicyHandler::new(
                crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string(),
            )),
            rt.clone(),
        ));
        rt.processor_registry.register(btls_proc.clone());
        let btls_raw = Arc::new(LocalRawStore::<BackendTLSPolicy>::new());
        btls_proc.set_raw_store(btls_raw.clone());
        let btls: BackendTLSPolicy = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3",
            "kind": "BackendTLSPolicy",
            "metadata": { "name": "tls", "namespace": "ns" },
            "spec": {
                "targetRefs": [ { "group": "", "kind": "Service", "name": "payments" } ],
                "validation": { "hostname": "stale.other.com", "wellKnownCACertificates": "System" }
            },
        }))
        .expect("valid BackendTLSPolicy JSON");
        btls_raw.upsert("ns/tls".to_string(), Arc::new(btls.clone()));
        btls_proc.save(btls, None);

        // Two EBTPs on the same Service, both with `upstreamAuthority`; `older` is the
        // precedence winner, `younger` is the loser.
        let mut older = make_policy("older", "payments", "2026-01-01T00:00:00Z");
        older.spec.upstream_authority = Some(
            edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: None,
            },
        );
        let mut younger = make_policy("younger", "payments", "2026-06-01T00:00:00Z");
        younger.spec.upstream_authority = older.spec.upstream_authority.clone();

        assert!(
            older.spec.get_validation_errors().is_empty(),
            "fixture must be spec-locally valid"
        );
        assert!(
            younger.spec.get_validation_errors().is_empty(),
            "fixture must be spec-locally valid"
        );

        let _ = handler.parse(older.clone(), &ctx).await;
        let _ = handler.parse(younger.clone(), &ctx).await;

        // `younger` loses oldest-wins AND fails the cross-object rule.
        let younger_errors = handler.validate(&younger, &ctx);
        assert_eq!(younger_errors.len(), 1, "got: {younger_errors:?}");

        // With the cross-object error present, Invalid masks Conflicted (priority
        // Invalid > Conflicted > Accepted).
        let mut younger_status = younger.clone();
        handler.update_status(&mut younger_status, &ctx, &younger_errors);
        let accepted = ancestor_condition(&younger_status, "gw-1", condition_types::ACCEPTED);
        assert_eq!(accepted.status, "False");
        assert_eq!(
            accepted.reason,
            condition_reasons::INVALID,
            "cross-object validation_errors takes priority over the conflict reason"
        );

        // Passing a forged empty validation vector cannot bypass the canonical
        // evaluation: cross-object-invalid policies remain withdrawn and Invalid.
        handler.update_status(&mut younger_status, &ctx, &[]);
        let accepted = ancestor_condition(&younger_status, "gw-1", condition_types::ACCEPTED);
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, condition_reasons::INVALID);
        assert!(younger_status.spec.resolved_target_class.is_none());
    }

    /// One Service reachable through two Gateways -> two ancestor entries,
    /// each stamped with our controllerName.
    #[tokio::test]
    async fn two_gateways_yield_two_ancestor_entries() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "payments", "route-a", "gw-a");
        wire_service_to_gateway(&rt, "ns", "payments", "route-b", "gw-b");

        let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;
        let mut p = p;
        handler.update_status(&mut p, &ctx, &[]);

        let ancestors = &p.status.as_ref().unwrap().ancestors;
        assert_eq!(ancestors.len(), 2);
        let names: Vec<&str> = ancestors.iter().map(|a| a.ancestor_ref.name.as_str()).collect();
        assert_eq!(names, vec!["gw-a", "gw-b"], "sorted by gateway key");
        for a in ancestors {
            assert_eq!(a.controller_name, TEST_CONTROLLER);
            assert_eq!(a.ancestor_ref.kind.as_deref(), Some("Gateway"));
        }
    }

    /// A conflicted Service marks ONLY the ancestors it reaches: policy
    /// targets svc-a (gw-a, unconflicted) + svc-b (gw-b, lost to an older
    /// sibling) -> gw-a Accepted=True, gw-b Accepted=False/Conflicted.
    #[tokio::test]
    async fn conflict_marks_only_affected_ancestor() {
        let (ctx, rt) = make_ctx();
        let handler = make_handler();
        wire_service_to_gateway(&rt, "ns", "svc-a", "route-a", "gw-a");
        wire_service_to_gateway(&rt, "ns", "svc-b", "route-b", "gw-b");

        let older = make_policy("older", "svc-b", "2026-01-01T00:00:00Z");
        let _ = handler.parse(older, &ctx).await;

        let mut loser = serde_json::from_value::<EdgionBackendTrafficPolicy>(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": { "name": "loser", "namespace": "ns", "creationTimestamp": "2026-06-01T00:00:00Z" },
            "spec": { "targetRefs": [
                { "kind": "Service", "name": "svc-a" },
                { "kind": "Service", "name": "svc-b" }
            ] },
        }))
        .unwrap();
        let _ = handler.parse(loser.clone(), &ctx).await;
        handler.update_status(&mut loser, &ctx, &[]);

        assert_eq!(
            ancestor_condition(&loser, "gw-a", condition_types::ACCEPTED).status,
            "True"
        );
        let on_b = ancestor_condition(&loser, "gw-b", condition_types::ACCEPTED);
        assert_eq!(on_b.status, "False");
        assert_eq!(on_b.reason, condition_reasons::POLICY_CONFLICTED);
        assert!(on_b.message.contains("ns/svc-b"));
    }

    #[test]
    fn policy_status_message_bound_is_utf8_safe() {
        let message = bounded_status_message("π".repeat(700));
        assert!(message.len() <= STATUS_MESSAGE_MAX_BYTES);
        assert!(message.ends_with(STATUS_TRUNCATION_SUFFIX));
        assert!(std::str::from_utf8(message.as_bytes()).is_ok());
    }
}
