//! EdgionBackendTrafficPolicy Handler (Controller side)
//!
//! Handles `EdgionBackendTrafficPolicy` resources — a per-destination traffic
//! policy (LB algorithm / active health check / outlier detection / panic
//! threshold) attached to Services via `targetRefs`. Design:
//! docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md
//!
//! Status conditions (lenient admission — an invalid or unresolved policy still
//! syncs to the data plane, which skips invalid specs itself):
//! - `Accepted`     — `False`/`Invalid` when `spec.get_validation_errors()` is
//!   non-empty, else `True`.
//! - `ResolvedRefs` — `False`/`ServiceNotFound` when a target Service is absent
//!   from the Service cache; the policy still syncs and takes effect once the
//!   Service appears (a Service change requeues the policy — see `parse`).
//! - `Conflicted`   — `True`/`LostOldestWins` when another policy in the same
//!   namespace targets one of the same Services with a lower `precedence_key`
//!   (oldest-wins); the winner gets `Conflicted=False`/`NoConflicts`.
//!
//! Conflict / precedence state is tracked in `ControllerRuntime`
//! (`edgion_backend_traffic_policy`, a `BackendTLSPolicyState` reused as the
//! target-conflict + creation-timestamp index) exactly as `BackendTLSPolicy`
//! does. Service-change requeue is wired through the shared `service_ref_mgr`
//! (the `ServiceHandler` requeues every ref registered under a Service key).

use crate::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::make_target_conflict_key;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::route_utils::service_exists;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
use edgion_resources::resources::edgion_backend_traffic_policy::EdgionBackendTrafficPolicyStatus;
use edgion_resources::resources::upstream_authority::host_matches_pattern;
use edgion_resources::ResourceKind;

/// EdgionBackendTrafficPolicy-specific condition reasons.
///
/// These are Edgion extensions, not Gateway API upstream reasons. `ServiceNotFound`
/// is the ResolvedRefs failure reason (analogous to the Gateway-API `BackendNotFound`
/// but scoped to a policy targetRef); `LostOldestWins` is the `Conflicted=True`
/// reason when an older sibling policy wins the shared Service.
mod ebtp_reasons {
    /// The referenced target Service does not exist (or is not yet cached).
    pub const SERVICE_NOT_FOUND: &str = "ServiceNotFound";
    /// Another policy targeting the same Service has an older (lower) precedence key.
    pub const LOST_OLDEST_WINS: &str = "LostOldestWins";
}

/// Decide whether sibling policies (those targeting the same Service) must be
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
    old_timestamp: Option<&str>,
    new_timestamp: Option<&str>,
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

/// EdgionBackendTrafficPolicy handler.
pub struct EdgionBackendTrafficPolicyHandler;

impl EdgionBackendTrafficPolicyHandler {
    pub fn new() -> Self {
        Self
    }

    fn resource_ref(p: &EdgionBackendTrafficPolicy) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            p.metadata.namespace.clone(),
            p.metadata.name.clone().unwrap_or_default(),
        )
    }
}

impl Default for EdgionBackendTrafficPolicyHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionBackendTrafficPolicy> for EdgionBackendTrafficPolicyHandler {
    fn validate(&self, p: &EdgionBackendTrafficPolicy, ctx: &HandlerContext) -> Vec<String> {
        let mut errors = p.spec.get_validation_errors();

        // Cross-object rule (spec §5.6): under `upstreamAuthority`, the composed
        // authority supplies the SNI and the SAN name, so a BackendTLSPolicy's
        // `validation.hostname` is inert. Requiring it to sit inside the pattern
        // keeps it from lying about which vhosts the backend serves.
        if let Some(ua) = &p.spec.upstream_authority {
            let ns = p.metadata.namespace.as_deref().unwrap_or("default");
            let records = load_backend_tls_policies(ctx);
            for tr in &p.spec.target_refs {
                for (key, hostname) in backend_tls_hostnames_for_service(ns, &tr.name, &records) {
                    if !host_matches_pattern(hostname, &ua.pattern) {
                        errors.push(format!(
                            "BackendTLSPolicy {} validation.hostname '{}' on Service {}/{} is outside \
                             upstreamAuthority.pattern '{}'",
                            key, hostname, ns, tr.name, ua.pattern
                        ));
                    }
                }
            }
        }

        errors
    }

    async fn parse(
        &self,
        p: EdgionBackendTrafficPolicy,
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

        // Clear old registrations, then re-register with the current targetRefs.
        // Two managers are updated:
        //   * `target_mgr` — conflict/precedence index (siblings, is_winner).
        //   * `service_ref_mgr` — drives Service-change requeue: the ServiceHandler
        //     requeues every ref registered under a Service key, so ResolvedRefs
        //     re-evaluates when a target Service appears/disappears.
        state.target_mgr().clear_resource_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&resource_ref);

        // A policy with validation errors is a no-op in the data plane: the
        // gateway store's `reapply_service` skips invalid specs BEFORE oldest-wins
        // arbitration. To keep controller arbitration in lock-step, an invalid
        // policy must NOT participate in the conflict index — it neither wins nor
        // marks a sibling `Conflicted`. We still track it in `service_ref_mgr` so a
        // Service appear/disappear re-evaluates its ResolvedRefs status.
        //
        // Because an invalid policy registers zero conflict keys, its
        // `old_target_keys`/`new_target_keys` snapshots diverge on any validity
        // flip (valid<->invalid) even when the target set is unchanged. That makes
        // `should_requeue_siblings` fire and re-evaluate siblings whose stale
        // `Conflicted` condition would otherwise survive the edit.
        let spec_valid = p.spec.get_validation_errors().is_empty();
        let policy_ns = p.metadata.namespace.as_deref().unwrap_or("default");
        let mut new_target_keys = Vec::with_capacity(p.spec.target_refs.len());
        for target_ref in &p.spec.target_refs {
            // v1 targets are same-namespace core Services; there is no sectionName.
            let target_key = make_target_conflict_key(policy_ns, &target_ref.name, None);
            // Service-change requeue tracking applies to every policy, valid or not.
            ctx.runtime()
                .service_ref_mgr
                .add_ref(target_key.clone(), resource_ref.clone());
            // Conflict-index participation is gated on validity to mirror the gateway.
            if spec_valid {
                state.target_mgr().add_ref(target_key.clone(), resource_ref.clone());
                new_target_keys.push(target_key);
            }
        }
        new_target_keys.sort();

        let new_timestamp = p.metadata.creation_timestamp.as_ref().map(|t| t.0.to_rfc3339());
        state.set_timestamp(&policy_resource_key, new_timestamp.as_deref());

        if should_requeue_siblings(
            &old_target_keys,
            &new_target_keys,
            old_timestamp.as_deref(),
            new_timestamp.as_deref(),
        ) {
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

        tracing::debug!(
            policy = %resource_ref.key(),
            sibling_count = siblings.len(),
            "Cleared references on EdgionBackendTrafficPolicy delete"
        );

        for sibling in siblings {
            ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
        }
    }

    fn update_status(&self, p: &mut EdgionBackendTrafficPolicy, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = p.metadata.generation;
        let policy_ns = p.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let policy_name = p.metadata.name.clone().unwrap_or_default();

        // Compute ResolvedRefs / Conflicted signals before borrowing status mutably.
        let missing_services: Vec<String> = p
            .spec
            .target_refs
            .iter()
            .filter(|tr| !service_exists(&policy_ns, &tr.name, ctx.runtime()))
            .map(|tr| format!("{}/{}", policy_ns, tr.name))
            .collect();

        // Conflict-index participation (and therefore `conflicted`) is gated on the
        // SPEC-LOCAL error set, matching `parse`'s gating of `target_mgr()` registration
        // (see `spec_valid` there) — NOT on `validation_errors`, which as of the
        // `upstreamAuthority` cross-object rule also carries the cross-object BackendTLSPolicy
        // hostname errors. Those two sets diverge whenever a spec is locally valid but fails
        // the cross-object rule: such a policy DOES win/lose the conflict index (its target
        // keys were registered in `parse`), so `conflicted` must be computed from the same
        // spec-local gate, or a conflicted-and-cross-object-invalid policy would wrongly report
        // `Conflicted=False/NoConflicts`.
        let policy_key = format!("{}/{}", policy_ns, policy_name);
        let conflicted = p.spec.get_validation_errors().is_empty()
            && p.spec.target_refs.iter().any(|tr| {
                let target_key = make_target_conflict_key(&policy_ns, &tr.name, None);
                !ctx.runtime()
                    .edgion_backend_traffic_policy
                    .is_winner(&policy_key, &target_key)
            });

        let status = p.status.get_or_insert_with(EdgionBackendTrafficPolicyStatus::default);
        let conditions = status.conditions.get_or_insert_with(Vec::new);

        // Accepted — reflects spec validation.
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Policy accepted",
                generation,
            )
        } else {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
                generation,
            )
        };
        update_condition(conditions, accepted);

        // ResolvedRefs — target Service existence (same namespace as the policy).
        let resolved = if missing_services.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All target Services resolved",
                generation,
            )
        } else {
            condition_false(
                condition_types::RESOLVED_REFS,
                ebtp_reasons::SERVICE_NOT_FOUND,
                format!("target Service(s) not found: {}", missing_services.join(", ")),
                generation,
            )
        };
        update_condition(conditions, resolved);

        // Conflicted — oldest-wins precedence across policies sharing a Service.
        let conflicted_cond = if conflicted {
            condition_true(
                condition_types::CONFLICTED,
                ebtp_reasons::LOST_OLDEST_WINS,
                "Conflicts with an older policy targeting the same Service",
                generation,
            )
        } else {
            condition_false(
                condition_types::CONFLICTED,
                condition_reasons::NO_CONFLICTS,
                "No conflicts",
                generation,
            )
        };
        update_condition(conditions, conflicted_cond);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use serde_json::json;
    use std::sync::Arc;

    fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (ctx, runtime)
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

    fn condition<'a>(
        p: &'a EdgionBackendTrafficPolicy,
        type_: &str,
    ) -> &'a edgion_resources::resources::common::Condition {
        p.status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .and_then(|c| c.iter().find(|c| c.type_ == type_))
            .unwrap_or_else(|| panic!("condition {type_} present"))
    }

    /// `validate()` returns the spec's validation errors verbatim.
    #[test]
    fn validate_returns_spec_validation_errors_verbatim() {
        let (ctx, _rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();

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
    /// the `precedence_key` minimum (oldest); the two losers are flagged Conflicted.
    #[tokio::test]
    async fn conflict_evaluation_oldest_wins_others_flagged() {
        let (ctx, _rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();

        // `b` is the oldest (min precedence_key), so it wins; `a` and `c` lose.
        let a = make_policy("a", "payments", "2026-03-01T00:00:00Z");
        let b = make_policy("b", "payments", "2026-01-01T00:00:00Z");
        let c = make_policy("c", "payments", "2026-06-01T00:00:00Z");

        for p in [a.clone(), b.clone(), c.clone()] {
            let _ = handler.parse(p, &ctx).await;
        }

        let check = |mut p: EdgionBackendTrafficPolicy, expect_conflicted: bool| {
            handler.update_status(&mut p, &ctx, &[]);
            let conflicted = condition(&p, condition_types::CONFLICTED);
            if expect_conflicted {
                assert_eq!(
                    conflicted.status,
                    "True",
                    "{} should be conflicted",
                    p.metadata.name.as_deref().unwrap()
                );
                assert_eq!(conflicted.reason, ebtp_reasons::LOST_OLDEST_WINS);
            } else {
                assert_eq!(conflicted.status, "False", "winner must not be conflicted");
                assert_eq!(conflicted.reason, condition_reasons::NO_CONFLICTS);
            }
            // Accepted is always True for these valid specs.
            assert_eq!(condition(&p, condition_types::ACCEPTED).status, "True");
        };

        check(b, false); // oldest wins
        check(a, true); // loser
        check(c, true); // loser
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
    /// newer policy targeting the same Service therefore wins (`Conflicted=False`).
    /// When the older policy is later edited to become valid, the newer one flips to
    /// `Conflicted=True/LostOldestWins`.
    #[tokio::test]
    async fn invalid_older_policy_does_not_conflict_valid_newer() {
        let (ctx, _rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();

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
        let conflicted = condition(&b_status, condition_types::CONFLICTED);
        assert_eq!(
            conflicted.status, "False",
            "valid newer policy must win over an invalid older one"
        );
        assert_eq!(conflicted.reason, condition_reasons::NO_CONFLICTS);

        // Edit A to a valid spec (update path) — it now participates and, being older,
        // wins; B must flip to Conflicted=True.
        let a_valid = make_policy("a", "payments", "2026-01-01T00:00:00Z");
        assert!(a_valid.spec.get_validation_errors().is_empty());
        let _ = handler.parse(a_valid, &ctx).await;

        let mut b_status = b.clone();
        handler.update_status(&mut b_status, &ctx, &[]);
        let conflicted = condition(&b_status, condition_types::CONFLICTED);
        assert_eq!(
            conflicted.status, "True",
            "now-valid older policy wins; newer becomes conflicted"
        );
        assert_eq!(conflicted.reason, ebtp_reasons::LOST_OLDEST_WINS);
    }

    /// ResolvedRefs is `False`/`ServiceNotFound` when the target Service is not in
    /// the cache (no Service processor registered in the test runtime → `service_exists`
    /// returns `true`, so we assert the resolved branch here) — the policy still syncs.
    #[tokio::test]
    async fn resolved_refs_true_when_service_lookup_unavailable() {
        let (ctx, _rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();
        let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;
        let mut p = p;
        handler.update_status(&mut p, &ctx, &[]);
        let resolved = condition(&p, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "True");
        assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
    }

    /// With a Service processor registered but its cache EMPTY, `service_exists`
    /// genuinely returns `false`, so ResolvedRefs is `False`/`ServiceNotFound`.
    /// Adding the target Service to the cache and re-running flips it to `True`.
    #[tokio::test]
    async fn resolved_refs_false_then_true_when_service_appears() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use k8s_openapi::api::core::v1::Service;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

        let (ctx, rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();

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

        let p = make_policy("p", "payments", "2026-01-01T00:00:00Z");
        let _ = handler.parse(p.clone(), &ctx).await;

        // Target Service not in cache -> ResolvedRefs=False/ServiceNotFound.
        let mut p_missing = p.clone();
        handler.update_status(&mut p_missing, &ctx, &[]);
        let resolved = condition(&p_missing, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False", "absent Service => ResolvedRefs False");
        assert_eq!(resolved.reason, ebtp_reasons::SERVICE_NOT_FOUND);

        // Add the Service to the cache (key = "ns/payments") and re-run: flips True.
        let svc = Service {
            metadata: ObjectMeta {
                name: Some("payments".to_string()),
                namespace: Some("ns".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        svc_proc.save(svc, None);

        let mut p_present = p.clone();
        handler.update_status(&mut p_present, &ctx, &[]);
        let resolved = condition(&p_present, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "True", "Service now present => ResolvedRefs True");
        assert_eq!(resolved.reason, condition_reasons::RESOLVED_REFS);
    }

    /// An invalid spec is admitted leniently: Accepted=False/Invalid but the
    /// resource still produces a status (it will still sync to the gateway).
    #[test]
    fn invalid_spec_is_accepted_false_invalid() {
        let (ctx, _rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();
        let mut p = make_policy("bad", "svc", "2026-01-01T00:00:00Z");
        p.spec.target_refs.clear();
        let errors = handler.validate(&p, &ctx);
        handler.update_status(&mut p, &ctx, &errors);
        let accepted = condition(&p, condition_types::ACCEPTED);
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
        let handler = EdgionBackendTrafficPolicyHandler::new();

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
        let handler = EdgionBackendTrafficPolicyHandler::new();

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
        let handler = EdgionBackendTrafficPolicyHandler::new();

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
    /// (its target keys are registered in `parse`, gated on `spec.get_validation_errors()`)
    /// and, if it loses oldest-wins, must still report `Conflicted=True/LostOldestWins` — not
    /// be silently suppressed because the widened `validation_errors` list (spec-local PLUS
    /// cross-object) is non-empty.
    #[tokio::test]
    async fn conflicted_status_survives_cross_object_validation_error() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::BackendTLSPolicy;

        let (ctx, rt) = make_ctx();
        let handler = EdgionBackendTrafficPolicyHandler::new();

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

        let mut younger_status = younger.clone();
        handler.update_status(&mut younger_status, &ctx, &younger_errors);
        let conflicted = condition(&younger_status, condition_types::CONFLICTED);
        assert_eq!(
            conflicted.status, "True",
            "a conflicted-and-cross-object-invalid policy must still report Conflicted=True, \
             not be silently suppressed by the widened validation_errors list"
        );
        assert_eq!(conflicted.reason, ebtp_reasons::LOST_OLDEST_WINS);
    }
}
