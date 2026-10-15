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
//! one Gateway and Conflicted on another. Owned ancestors are removed when no
//! route references any target; foreign entries are retained (matching the BackendTLSPolicy handler's
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
//! Requeue edges (policy publication changes notify dependent Routes only after
//! cache publication; unchanged signatures stop the Route/Policy return wave):
//! - Service or EdgionBackend appear/disappear requeues every policy ref
//!   registered under the corresponding typed dependency key.
//! - Route change -> `requeue_backend_policies_for_owner` /
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
use crate::conf_mgr::sync_runtime::resource_processor::status_pipeline::filter_current_native_attachment_status;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use chrono::{DateTime, Utc};
#[cfg(test)]
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::prelude_resources::{BackendTLSPolicy, EdgionBackendTrafficPolicy};
use edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence;
use edgion_resources::resources::common::{ParentReference, PolicyAncestorStatus};
use edgion_resources::resources::edgion_backend_traffic_policy::{
    EdgionBackendTrafficPolicyStatus, PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::resources::refs::policy_fields::{BackendTlsPolicyTargets, BackendTrafficPolicyTargets};
use edgion_resources::resources::upstream_authority::host_matches_pattern;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Service;
use std::collections::BTreeSet;
use std::sync::Arc;

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
/// snapshotted once per `validate()` call.
struct BackendTlsPolicyRecord {
    key: String,
    policy: Arc<BackendTLSPolicy>,
}

/// Load every canonical v1 BackendTLSPolicy visible in the processor registry.
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
    let Some(processor) = ctx.runtime().processor_registry.get("BackendTLSPolicy") else {
        return Vec::new();
    };

    processor
        .list_keys()
        .into_iter()
        .filter_map(|key| {
            let any = processor.get_raw_any(&key)?;
            let b = any.downcast::<BackendTLSPolicy>().ok()?;
            Some(BackendTlsPolicyRecord { key, policy: b })
        })
        .collect()
}

/// Every `(policy key, validation.hostname)` pair for BackendTLSPolicies in `records` that
/// target `namespace/service` via an exact standard core-group `Service` targetRef.
fn backend_tls_hostnames_for_service<'a>(
    namespace: &str,
    service: &str,
    records: &'a [BackendTlsPolicyRecord],
) -> Vec<(&'a str, &'a str)> {
    records
        .iter()
        .filter(|record| {
            BackendTlsPolicyTargets::from_policy(&record.policy)
                .fields()
                .any(|field| {
                    field.canonical_key().is_ok_and(|target| {
                        target.target_kind == PolicyTargetKind::Service
                            && target.namespace == namespace
                            && target.name == service
                    })
                })
        })
        .map(|record| (record.key.as_str(), record.policy.spec.validation.hostname.as_str()))
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

    let mut invalid_identity = false;
    for field in BackendTrafficPolicyTargets::from_policy(p).fields() {
        match field.canonical_key() {
            Ok(target) => evaluation.targets.push(target),
            Err(error) => {
                invalid_identity = true;
                evaluation.errors.push(error);
            }
        }
    }
    // Identity errors invalidate the whole policy before any cross-object reads.
    if invalid_identity {
        return evaluation;
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
        Some(ResolvedPolicyTargetClass::Service) | None => {}
    }
    if evaluation.errors.is_empty() && evaluation.missing_targets.is_empty() && !mixed_class {
        evaluation.target_class = class;
    }
    evaluation
}

/// TLS failures are deliberately excluded from `evaluate_policy`: a winning
/// encrypted check stays attached and fails until its own trust/identity recovers.
fn health_tls_shape_error(p: &EdgionBackendTrafficPolicy) -> Option<&'static str> {
    let active = p.spec.health_check.as_ref()?.active.as_ref()?;
    active
        .tls_validation_error()
        .or_else(|| active.tls_namespace_validation_error(p.metadata.namespace.as_deref().unwrap_or("default")))
}

fn resolve_health_tls(p: &mut EdgionBackendTrafficPolicy, ctx: &HandlerContext, owner: &ResourceRef) {
    use super::super::credential::ca_refs::{configmap_ca_material, ordinary_secret_material};
    use super::super::credential::pipeline::fetch_bound_configmap;
    use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
    use edgion_common::tls_validation::{check_key_matches_cert, parse_canonical_ca_bundle};
    use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;

    ctx.secret_ref_manager().clear_value_refs(owner);
    let Some(active) = p.spec.health_check.as_mut().and_then(|h| h.active.as_mut()) else {
        return;
    };
    active.resolved_ca_certificates = None;
    active.resolved_client_certificate = None;
    active.resolved_tls_error = None;
    let namespace = p.metadata.namespace.as_deref().unwrap_or("default");
    if let Some(error) = active.tls_namespace_validation_error(namespace) {
        active.resolved_tls_error = Some(error.into());
        return;
    }
    if active.tls_validation_error().is_some() {
        return;
    }
    let (ca, client, error) = {
        let refs = HealthTlsRefs::from_policy(p);
        let field = match refs.field() {
            Ok(Some(field)) => field,
            Ok(None) => return,
            Err(_) => {
                p.spec
                    .health_check
                    .as_mut()
                    .and_then(|h| h.active.as_mut())
                    .unwrap()
                    .resolved_tls_error = Some("health check TLS reference is invalid".into());
                return;
            }
        };
        let mut ca = Vec::new();
        let mut error = None;
        let mut missing_ca = false;
        field.visit_ca(|token| {
            let material = match token.validated().map(|r| r.target().group_kind().kind()) {
                Ok("Secret") => match fetch_bound_secret(ctx, owner, token) {
                    BoundFetchOutcome::Ok(value) => Some(ordinary_secret_material(value.secret)),
                    _ => None,
                },
                Ok("ConfigMap") => match fetch_bound_configmap(ctx, owner, token) {
                    BoundFetchOutcome::Ok(value) => configmap_ca_material(&value),
                    _ => None,
                },
                _ => None,
            };
            match material {
                Some(secret) => {
                    if secret
                        .data
                        .as_ref()
                        .and_then(|d| d.get("ca.crt"))
                        .is_none_or(|pem| parse_canonical_ca_bundle(&pem.0).is_err())
                    {
                        error.get_or_insert_with(|| "health check CA certificate material is invalid".to_string());
                    }
                    ca.push(secret);
                }
                None => {
                    missing_ca = true;
                }
            }
        });
        if missing_ca {
            error = Some("health check CA reference is unresolved".to_string());
        }
        let client = field
            .client_reference()
            .and_then(|token| match fetch_bound_secret(ctx, owner, token) {
                BoundFetchOutcome::Ok(value) => {
                    let secret = ordinary_secret_material(value.secret);
                    let valid = secret.type_.as_deref() == Some("kubernetes.io/tls")
                        && secret.data.as_ref().is_some_and(|data| {
                            let Some(cert) = data.get("tls.crt").and_then(|p| std::str::from_utf8(&p.0).ok()) else {
                                return false;
                            };
                            let Some(key) = data.get("tls.key").and_then(|p| std::str::from_utf8(&p.0).ok()) else {
                                return false;
                            };
                            check_key_matches_cert(cert, key).is_ok()
                        });
                    if !valid {
                        error.get_or_insert_with(|| "health check client certificate material is invalid".to_string());
                    }
                    Some(secret)
                }
                _ => {
                    error.get_or_insert_with(|| "health check client certificate reference is unresolved".to_string());
                    None
                }
            });
        (ca, client, error)
    };
    let active = p.spec.health_check.as_mut().and_then(|h| h.active.as_mut()).unwrap();
    active.resolved_ca_certificates = (!ca.is_empty()).then_some(ca);
    active.resolved_client_certificate = client;
    active.resolved_tls_error = error;
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
    fn finalize_transport(&self, policy: &mut EdgionBackendTrafficPolicy, _ctx: &HandlerContext) {
        filter_current_native_attachment_status(policy, &self.controller_name);
    }

    fn validate(&self, p: &EdgionBackendTrafficPolicy, ctx: &HandlerContext) -> Vec<String> {
        let evaluation = evaluate_policy(p, ctx);
        let mut errors = evaluation.errors;
        errors.extend(
            evaluation
                .missing_targets
                .into_iter()
                .map(|target| format!("policy target {target} was not found")),
        );
        if let Some(error) = health_tls_shape_error(p) {
            errors.push(error.to_string());
        }
        if let Some(error) = p
            .spec
            .health_check
            .as_ref()
            .and_then(|health| health.active.as_ref())
            .and_then(|active| active.resolved_tls_error.as_ref())
        {
            errors.push(error.clone());
        }
        errors
    }

    async fn parse(
        &self,
        mut p: EdgionBackendTrafficPolicy,
        ctx: &HandlerContext,
    ) -> ProcessResult<EdgionBackendTrafficPolicy> {
        let resource_ref = Self::resource_ref(&p);
        resolve_health_tls(&mut p, ctx, &resource_ref);
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
        p.spec.prepared_ancestor_admission = None;
        p.spec.prepared_precedence = Some(PreparedPolicyPrecedence {
            creation_timestamp: p.metadata.creation_timestamp.as_ref().map(|time| time.0),
        });

        // Clear old registrations, then re-register with the current targetRefs.
        // Two managers are updated:
        //   * `target_mgr` — conflict/precedence index (siblings, is_winner).
        //   * typed Service/backend ref manager — drives target-change requeue so
        //     ResolvedRefs re-evaluates when a target appears/disappears.
        state.target_mgr().clear_value_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_value_refs(&resource_ref);

        // Register every syntactically canonical dependency before any
        // existence/capability lookup so target creation can wake a policy
        // that is currently unresolved.
        for target in BackendTrafficPolicyTargets::from_policy(&p)
            .fields()
            .filter_map(|field| field.canonical_key().ok())
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

        let new_timestamp = p
            .spec
            .prepared_precedence
            .and_then(|prepared| prepared.creation_timestamp);
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

        state.target_mgr().clear_value_refs(&resource_ref);
        state.remove_timestamp(&resource_ref.resource_key());
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_value_refs(&resource_ref);

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
        let policy_key = Self::resource_ref(p).resource_key();

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
        let tls_shape_error = health_tls_shape_error(p);
        let tls_resolution_error = p
            .spec
            .health_check
            .as_ref()
            .and_then(|health| health.active.as_ref())
            .and_then(|active| active.resolved_tls_error.clone());

        let status = p.status.get_or_insert_with(EdgionBackendTrafficPolicyStatus::default);

        // Rebuild the ancestors list, carrying each surviving ancestor's prior
        // conditions forward so update_condition can keep lastTransitionTime
        // stable when the status does not actually flip (mirrors the
        // BackendTLSPolicy handler post-ffe29c28).
        let old_ancestors = std::mem::take(&mut status.ancestors);
        status.ancestors.extend(
            old_ancestors
                .iter()
                .filter(|ancestor| ancestor.controller_name != self.controller_name)
                .cloned(),
        );

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
            } else if let Some(error) = tls_shape_error {
                condition_false(condition_types::ACCEPTED, condition_reasons::INVALID, error, generation)
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
            let resolved = if let Some(error) = &tls_resolution_error {
                condition_false(
                    condition_types::RESOLVED_REFS,
                    "InvalidCertificateRef",
                    error.clone(),
                    generation,
                )
            } else if missing_here.is_empty() {
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
        if ctx.runtime().status_identity().is_none() {
            super::super::policy_ancestor_capacity::compose_native(
                p.metadata.namespace.as_deref().unwrap_or("default"),
                &self.controller_name,
                &old_ancestors,
                &mut status.ancestors,
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy_test.rs"]
mod tests;
