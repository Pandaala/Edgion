//! Runtime store + conflict arbitration for `EdgionBackendTrafficPolicy`.
//!
//! One `ArcSwap` generation publishes the policy map, reverse index, and
//! typed effective snapshots together. HTTP/gRPC request capture (Task 2)
//! reads that snapshot; Task 1 snapshot-backed shims keep the crate compiling.

use super::retry_budget::RetryBudget;
use crate::ai::{get_ai_credential_outlier_store, AiCredentialOutlierBackendHandle};
use crate::backends::degrade_threshold::get_degrade_threshold_store;
use crate::backends::health::check::{get_hc_config_store, get_health_check_manager};
use crate::backends::outlier_detection::get_outlier_detection_store;
use arc_swap::ArcSwap;
use edgion_resources::resources::common::policy_precedence::policy_precedence_key;
use edgion_resources::resources::edgion_backend_traffic_policy::{
    PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass, RetryConstraintConfig,
};
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
use edgion_resources::resources::refs::policy_fields::BackendTrafficPolicyTargets;
use edgion_resources::resources::upstream_authority::CompiledUpstreamAuthority;
use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
use edgion_resources::resources::EdgionBackendTrafficPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static BACKEND_TRAFFIC_POLICY_STORE: LazyLock<BackendTrafficPolicyStore> =
    LazyLock::new(BackendTrafficPolicyStore::new);

/// Process-global singleton store.
pub fn get_backend_traffic_policy_store() -> &'static BackendTrafficPolicyStore {
    &BACKEND_TRAFFIC_POLICY_STORE
}

/// Store owning the synced policy cache and one typed immutable generation.
pub struct BackendTrafficPolicyStore {
    effective: ArcSwap<EffectivePolicySnapshotData>,
}

/// Immutable prepared resource shared by the resource map and target index.
struct PreparedPolicy {
    key: String,
    policy: Arc<EdgionBackendTrafficPolicy>,
    eligible: bool,
    target_keys: Vec<PolicyTargetKey>,
    precedence: Option<chrono::DateTime<chrono::Utc>>,
}

/// Effective destination policy captured once by a request. Live retry
/// accounting remains behind its stable `Arc`; the other fields are immutable
/// normalized values.
#[derive(Clone)]
pub struct EffectiveBackendTrafficPolicy {
    pub ancestor_admission:
        Option<edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission>,
    pub resolved_target_class: ResolvedPolicyTargetClass,
    pub winning_policy_key: String,
    pub load_balancer: Option<ParsedLBPolicy>,
    pub degrade_threshold_override: Option<u8>,
    pub upstream_authority: Option<Arc<CompiledUpstreamAuthority>>,
    pub(crate) upstream_authority_config: Option<UpstreamAuthorityConfig>,
    pub retry_budget: Option<Arc<RetryBudget>>,
    pub(crate) retry_constraint: Option<RetryConstraintConfig>,
    pub circuit_breaker_limit: Option<u32>,
    pub connect_timeout: Option<std::time::Duration>,
    pub outlier_detection: Option<OutlierDetectionResolved>,
    /// Exact AI credential-outlier policy generation for request capture.
    /// `None` for core Service targets.
    pub ai_credential_outlier: Option<Arc<AiCredentialOutlierBackendHandle>>,
    pub logical_circuit_breaker: Option<Arc<crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker>>,
}

impl std::fmt::Debug for EffectiveBackendTrafficPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectiveBackendTrafficPolicy")
            .field("winning_policy_key", &self.winning_policy_key)
            .field("resolved_target_class", &self.resolved_target_class)
            .finish_non_exhaustive()
    }
}

struct EffectivePolicySnapshotData {
    generation: u64,
    policies: HashMap<String, Arc<PreparedPolicy>>,
    reverse_index: HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>>,
    effective: HashMap<PolicyTargetKey, Arc<EffectiveBackendTrafficPolicy>>,
}

/// One immutable, request-capturable view of all effective backend policies.
/// A handle remains valid after later store publications.
#[derive(Clone)]
pub struct BackendTrafficPolicySnapshot {
    inner: Arc<EffectivePolicySnapshotData>,
}

pub(crate) const OBSERVATION_MAX_CANDIDATES: usize = 32;
pub(crate) const OBSERVATION_MAX_KEY_BYTES: usize = 512;
pub(crate) const OBSERVATION_MAX_TARGET_BYTES: usize = 253;

/// Bounded metadata from one prepared generation, exclusively for Admin diagnostics.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetObservation {
    generation: u64,
    complete: bool,
    target: ObservedTarget,
    candidates: Vec<String>,
    effective: Option<ObservedEffectivePolicy>,
}

#[derive(serde::Serialize)]
struct ObservedTarget {
    group: &'static str,
    kind: &'static str,
    namespace: String,
    name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ObservedEffectivePolicy {
    policy_key: String,
    load_balancer: Option<&'static str>,
}

impl BackendTrafficPolicySnapshot {
    #[inline]
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    #[inline]
    pub fn get_effective(&self, target: &PolicyTargetKey) -> Option<Arc<EffectiveBackendTrafficPolicy>> {
        self.inner.effective.get(target).cloned()
    }

    #[inline]
    pub fn candidates(&self, target: &PolicyTargetKey) -> Vec<Arc<EdgionBackendTrafficPolicy>> {
        self.inner
            .reverse_index
            .get(target)
            .map(|list| list.iter().map(|prepared| Arc::clone(&prepared.policy)).collect())
            .unwrap_or_default()
    }

    /// Project only bounded identities and the actual effective policy. These
    /// diagnostic bounds do not restrict runtime policy fanout or admission.
    pub(crate) fn observe_target(&self, target: &PolicyTargetKey) -> Result<TargetObservation, &'static str> {
        if target.namespace.len() > OBSERVATION_MAX_TARGET_BYTES || target.name.len() > OBSERVATION_MAX_TARGET_BYTES {
            return Err("Policy observation target exceeds metadata limit");
        }
        let candidates = self
            .inner
            .reverse_index
            .get(target)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if candidates.len() > OBSERVATION_MAX_CANDIDATES {
            return Err("Policy observation candidate limit exceeded");
        }
        let effective = self.inner.effective.get(target);
        // Preflight the existing borrowed slice before allocating a result Vec
        // or copying any metadata. Never use the unbounded candidates() helper.
        if candidates
            .iter()
            .any(|candidate| candidate.key.len() > OBSERVATION_MAX_KEY_BYTES)
            || effective.is_some_and(|policy| policy.winning_policy_key.len() > OBSERVATION_MAX_KEY_BYTES)
        {
            return Err("Policy observation key exceeds metadata limit");
        }
        Ok(TargetObservation {
            generation: self.inner.generation,
            complete: true,
            target: ObservedTarget {
                group: target.group(),
                kind: target.kind(),
                namespace: target.namespace.clone(),
                name: target.name.clone(),
            },
            candidates: candidates.iter().map(|candidate| candidate.key.clone()).collect(),
            effective: effective.map(|policy| ObservedEffectivePolicy {
                policy_key: policy.winning_policy_key.clone(),
                load_balancer: policy.load_balancer.as_ref().map(|lb| match lb {
                    ParsedLBPolicy::Ewma => "Ewma",
                    ParsedLBPolicy::LeastConn => "LeastConn",
                    ParsedLBPolicy::ConsistentHash(_) => "ConsistentHash",
                }),
            }),
        })
    }
}

fn effective_snapshot_eq(
    left: Option<&EffectiveBackendTrafficPolicy>,
    right: Option<&EffectiveBackendTrafficPolicy>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.ancestor_admission == right.ancestor_admission
                && left.resolved_target_class == right.resolved_target_class
                && left.winning_policy_key == right.winning_policy_key
                && left.circuit_breaker_limit == right.circuit_breaker_limit
                && left.load_balancer == right.load_balancer
                && left.degrade_threshold_override == right.degrade_threshold_override
                && left.upstream_authority_config == right.upstream_authority_config
                && left.retry_constraint == right.retry_constraint
                && left.connect_timeout == right.connect_timeout
                && left.outlier_detection == right.outlier_detection
                && match (&left.logical_circuit_breaker, &right.logical_circuit_breaker) {
                    (None, None) => true,
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    _ => false,
                }
                && match (&left.ai_credential_outlier, &right.ai_credential_outlier) {
                    (None, None) => true,
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    _ => false,
                }
                && match (&left.retry_budget, &right.retry_budget) {
                    (None, None) => true,
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    _ => false,
                }
        }
        _ => false,
    }
}

fn reverse_index_equivalent(
    left: &HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>>,
    right: &HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>>,
) -> bool {
    left.len() == right.len()
        && right.iter().all(|(target, policies)| {
            left.get(target).is_some_and(|existing| {
                existing.len() == policies.len()
                    && existing
                        .iter()
                        .zip(policies)
                        .all(|(left, right)| Arc::ptr_eq(&left.policy, &right.policy))
            })
        })
}

fn policies_pointer_equivalent(
    left: &HashMap<String, Arc<PreparedPolicy>>,
    right: &HashMap<String, Arc<PreparedPolicy>>,
) -> bool {
    left.len() == right.len()
        && right.iter().all(|(key, prepared)| {
            left.get(key)
                .is_some_and(|existing| Arc::ptr_eq(&existing.policy, &prepared.policy))
        })
}

fn effective_pointer_equivalent(
    left: &HashMap<PolicyTargetKey, Arc<EffectiveBackendTrafficPolicy>>,
    right: &HashMap<PolicyTargetKey, Arc<EffectiveBackendTrafficPolicy>>,
) -> bool {
    left.len() == right.len()
        && right
            .iter()
            .all(|(target, policy)| left.get(target).is_some_and(|existing| Arc::ptr_eq(existing, policy)))
}

impl Default for BackendTrafficPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendTrafficPolicyStore {
    pub fn new() -> Self {
        Self {
            effective: ArcSwap::from_pointee(EffectivePolicySnapshotData {
                generation: 0,
                policies: HashMap::new(),
                reverse_index: HashMap::new(),
                effective: HashMap::new(),
            }),
        }
    }

    fn canonical_target_keys(policy: &EdgionBackendTrafficPolicy) -> Vec<PolicyTargetKey> {
        BackendTrafficPolicyTargets::from_policy(policy)
            .fields()
            .map(|field| field.canonical_key())
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default()
    }

    fn dedup_target_keys(keys: Vec<PolicyTargetKey>) -> Vec<PolicyTargetKey> {
        let mut seen = HashSet::with_capacity(keys.len());
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
        out
    }

    fn prepare_policy(key: String, policy: Arc<EdgionBackendTrafficPolicy>) -> PreparedPolicy {
        #[cfg(test)]
        publish_counters::PREPARE_POLICY_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let target_keys = Self::dedup_target_keys(Self::canonical_target_keys(&policy));
        let eligible =
            edgion_resources::resources::common::policy_candidates::prepared_backend_traffic_targets(&policy).is_some();
        PreparedPolicy {
            key,
            precedence: policy
                .spec
                .prepared_precedence
                .and_then(|prepared| prepared.creation_timestamp),
            policy,
            eligible,
            target_keys,
        }
    }

    fn policy_resource_equivalent(left: &EdgionBackendTrafficPolicy, right: &EdgionBackendTrafficPolicy) -> bool {
        left.metadata.name == right.metadata.name
            && left.metadata.namespace == right.metadata.namespace
            && left.metadata.uid == right.metadata.uid
            && left.metadata.creation_timestamp == right.metadata.creation_timestamp
            && left.spec.prepared_precedence == right.spec.prepared_precedence
            && left.spec.prepared_ancestor_admission == right.spec.prepared_ancestor_admission
            && left.spec.target_refs == right.spec.target_refs
            && left.spec.resolved_target_class == right.spec.resolved_target_class
            && left.spec.load_balancer == right.spec.load_balancer
            && left.spec.outlier_detection == right.spec.outlier_detection
            && left.spec.upstream_authority == right.spec.upstream_authority
            && left.spec.retry_constraint == right.spec.retry_constraint
            && left.spec.circuit_breaker == right.spec.circuit_breaker
            && left.spec.connection == right.spec.connection
            && match (&left.spec.health_check, &right.spec.health_check) {
                (None, None) => true,
                (Some(left), Some(right)) => left.active == right.active,
                _ => false,
            }
    }

    fn retain_or_prepare_policy(
        previous: &EffectivePolicySnapshotData,
        key: &str,
        incoming: Arc<EdgionBackendTrafficPolicy>,
    ) -> Arc<PreparedPolicy> {
        let prepared = previous.policies.get(key);
        let policy = prepared
            .filter(|old| Self::policy_resource_equivalent(&old.policy, &incoming))
            .map(|old| Arc::clone(&old.policy))
            .unwrap_or(incoming);
        if let Some(prepared) = prepared {
            if Arc::ptr_eq(&prepared.policy, &policy) {
                return Arc::clone(prepared);
            }
        }
        Arc::new(Self::prepare_policy(key.to_owned(), policy))
    }

    fn build_reverse_index(
        policies: &HashMap<String, Arc<PreparedPolicy>>,
    ) -> HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>> {
        #[cfg(test)]
        publish_counters::FULL_INDEX_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let mut index: HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>> = HashMap::new();
        for prepared in policies.values() {
            if !prepared.eligible {
                continue;
            }
            for target in &prepared.target_keys {
                index.entry(target.clone()).or_default().push(Arc::clone(prepared));
            }
        }
        for candidates in index.values_mut() {
            candidates.sort_by(|a, b| {
                policy_precedence_key(a.precedence, a.policy.metadata.name.as_deref().unwrap_or_default()).cmp(
                    &policy_precedence_key(b.precedence, b.policy.metadata.name.as_deref().unwrap_or_default()),
                )
            });
        }
        index
    }

    fn patch_reverse_index(
        index: &mut HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>>,
        old_prepared: Option<&Arc<PreparedPolicy>>,
        new_prepared: Option<&Arc<PreparedPolicy>>,
    ) {
        if let Some(old) = old_prepared {
            for key in &old.target_keys {
                if let Some(list) = index.get_mut(key) {
                    list.retain(|prepared| !Arc::ptr_eq(&prepared.policy, &old.policy));
                    if list.is_empty() {
                        index.remove(key);
                    }
                }
            }
        }
        if let Some(new) = new_prepared.filter(|prepared| prepared.eligible) {
            for key in &new.target_keys {
                let list = index.entry(key.clone()).or_default();
                if !list.iter().any(|prepared| Arc::ptr_eq(&prepared.policy, &new.policy)) {
                    list.push(Arc::clone(new));
                }
                list.sort_by(|a, b| {
                    policy_precedence_key(a.precedence, a.policy.metadata.name.as_deref().unwrap_or_default()).cmp(
                        &policy_precedence_key(b.precedence, b.policy.metadata.name.as_deref().unwrap_or_default()),
                    )
                });
            }
        }
    }

    pub(crate) fn service_target(service_key: &str) -> PolicyTargetKey {
        let (namespace, name) = service_key.split_once('/').unwrap_or(("", service_key));
        PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    }

    /// Snapshot-backed shim. Live-generation read; not an AC close.
    pub fn get_lb_policy(&self, service_key: &str) -> Option<ParsedLBPolicy> {
        self.snapshot()
            .get_effective(&Self::service_target(service_key))
            .and_then(|policy| policy.load_balancer.clone())
    }

    /// Snapshot-backed shim. Live-generation read; not an AC close.
    pub fn get_authority(&self, service_key: &str) -> Option<Arc<CompiledUpstreamAuthority>> {
        self.snapshot()
            .get_effective(&Self::service_target(service_key))
            .and_then(|policy| policy.upstream_authority.clone())
    }

    /// Snapshot-backed shim. Live-generation read; not an AC close.
    pub fn get_retry_budget(&self, service_key: &str) -> Option<Arc<RetryBudget>> {
        self.snapshot()
            .get_effective(&Self::service_target(service_key))
            .and_then(|policy| policy.retry_budget.clone())
    }

    /// Snapshot-backed shim. Live-generation read; not an AC close.
    pub fn get_circuit_breaker_limit(&self, service_key: &str) -> Option<u32> {
        self.snapshot()
            .get_effective(&Self::service_target(service_key))
            .and_then(|policy| policy.circuit_breaker_limit)
    }

    /// Snapshot-backed shim. Live-generation read; not an AC close.
    pub fn get_connect_timeout(&self, service_key: &str) -> Option<std::time::Duration> {
        self.snapshot()
            .get_effective(&Self::service_target(service_key))
            .and_then(|policy| policy.connect_timeout)
    }

    /// Effective policy for a typed Service or EdgionBackend destination.
    /// Callers capture the returned Arc once and use it for the whole request.
    pub fn get_effective(&self, target: &PolicyTargetKey) -> Option<Arc<EffectiveBackendTrafficPolicy>> {
        self.snapshot().get_effective(target)
    }

    /// Capture a coherent generation for all typed policy targets.
    #[inline]
    pub fn snapshot(&self) -> BackendTrafficPolicySnapshot {
        BackendTrafficPolicySnapshot {
            inner: self.effective.load_full(),
        }
    }

    /// Admin snapshot of the resolved load-balancer map: Service key -> debug
    /// string of the applied `ParsedLBPolicy`. Not on the request path.
    pub fn lb_policy_snapshot(&self) -> std::collections::BTreeMap<String, String> {
        self.snapshot()
            .inner
            .effective
            .iter()
            .filter(|(target, _)| target.target_kind == PolicyTargetKind::Service)
            .filter_map(|(target, policy)| {
                policy
                    .load_balancer
                    .as_ref()
                    .map(|lb| (format!("{}/{}", target.namespace, target.name), format!("{lb:?}")))
            })
            .collect()
    }

    fn build_effective(
        target: &PolicyTargetKey,
        winner_key: &str,
        policy: &EdgionBackendTrafficPolicy,
        old: Option<&EffectiveBackendTrafficPolicy>,
    ) -> Arc<EffectiveBackendTrafficPolicy> {
        let is_service = target.target_kind == PolicyTargetKind::Service;
        let is_ai = target.target_kind == PolicyTargetKind::EdgionBackend;
        let normalized_outlier = policy.spec.outlier_detection.clone().map(|od| od.normalize());

        let (load_balancer, degrade_threshold_override) = if is_service {
            (
                policy.spec.load_balancer.as_ref().and_then(|lb| lb.to_parsed()),
                policy.spec.load_balancer.as_ref().and_then(|lb| lb.degrade_threshold),
            )
        } else {
            (None, None)
        };

        let (upstream_authority, upstream_authority_config) = if is_service {
            match policy.spec.upstream_authority.as_ref() {
                Some(cfg) => (
                    CompiledUpstreamAuthority::compile(cfg).ok().map(Arc::new),
                    Some(cfg.clone()),
                ),
                None => (None, None),
            }
        } else {
            (None, None)
        };

        let retry_constraint = policy.spec.retry_constraint.clone();
        let retry_budget = match retry_constraint.as_ref() {
            Some(cfg) => match old {
                Some(old) if old.retry_constraint.as_ref() == Some(cfg) => old.retry_budget.clone(),
                _ => Some(Arc::new(RetryBudget::from_config(cfg))),
            },
            None => None,
        };

        let circuit_breaker_limit = policy.spec.circuit_breaker.as_ref().map(|cb| cb.max_parallel_requests);
        let logical_circuit_breaker = if is_ai {
            match circuit_breaker_limit {
                Some(limit) => match old {
                    Some(old) if old.circuit_breaker_limit == Some(limit) => old.logical_circuit_breaker.clone(),
                    _ => Some(crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker::new(
                        limit,
                    )),
                },
                None => None,
            }
        } else {
            None
        };

        let connect_timeout = policy
            .spec
            .connection
            .as_ref()
            .and_then(|c| c.connect_timeout.as_deref())
            .and_then(|s| edgion_resources::duration::parse_duration(s).ok());

        let ai_credential_outlier = if is_ai {
            match old {
                Some(old) if old.outlier_detection == normalized_outlier && old.ai_credential_outlier.is_some() => {
                    old.ai_credential_outlier.clone()
                }
                _ => get_ai_credential_outlier_store()
                    .publish_effective_policy(target.clone(), normalized_outlier.clone()),
            }
        } else {
            None
        };

        let target_class = policy
            .spec
            .resolved_target_class
            .expect("eligible policy has a resolved target class");

        Arc::new(EffectiveBackendTrafficPolicy {
            ancestor_admission: policy
                .spec
                .prepared_ancestor_admission
                .clone()
                .map(|value| value.validated()),
            resolved_target_class: target_class,
            winning_policy_key: winner_key.to_string(),
            load_balancer,
            degrade_threshold_override,
            upstream_authority,
            upstream_authority_config,
            retry_budget,
            retry_constraint,
            circuit_breaker_limit,
            connect_timeout,
            outlier_detection: normalized_outlier,
            ai_credential_outlier,
            logical_circuit_breaker,
        })
    }

    fn apply_background_side_effects(
        &self,
        _previous: &EffectivePolicySnapshotData,
        next: &EffectivePolicySnapshotData,
        affected: &HashSet<PolicyTargetKey>,
    ) {
        #[cfg(test)]
        publish_counters::SIDE_EFFECT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let hc_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let cb_store = get_outlier_detection_store();
        let pt_store = get_degrade_threshold_store();

        for target in affected {
            match target.target_kind {
                PolicyTargetKind::Service => {
                    let service_key = format!("{}/{}", target.namespace, target.name);
                    match next.effective.get(target) {
                        Some(effective) => {
                            let hc = next
                                .policies
                                .get(&effective.winning_policy_key)
                                .map(|prepared| Arc::clone(&prepared.policy));
                            hc_store.set_service_config(&service_key, hc);
                            hc_manager.reconcile_service(&service_key);
                            cb_store.set_service_config(&service_key, effective.outlier_detection.clone());
                            pt_store.set_service_threshold(&service_key, effective.degrade_threshold_override);
                        }
                        None => {
                            hc_store.set_service_config(&service_key, None);
                            hc_manager.reconcile_service(&service_key);
                            cb_store.set_service_config(&service_key, None);
                            pt_store.set_service_threshold(&service_key, None);
                        }
                    }
                }
                PolicyTargetKind::EdgionBackend => {
                    if !next.effective.contains_key(target) {
                        get_ai_credential_outlier_store().clear_effective_policy(target);
                    }
                }
            }
        }
    }

    fn publish_generation(
        &self,
        previous: &EffectivePolicySnapshotData,
        next_policies: HashMap<String, Arc<PreparedPolicy>>,
        reverse_index: HashMap<PolicyTargetKey, Vec<Arc<PreparedPolicy>>>,
        affected: HashSet<PolicyTargetKey>,
    ) -> usize {
        let affected_len = affected.len();
        let mut effective = HashMap::with_capacity(previous.effective.len());
        for (target, policy) in &previous.effective {
            if !affected.contains(target) {
                effective.insert(target.clone(), Arc::clone(policy));
            }
        }
        for target in &affected {
            if let Some(winner) = reverse_index.get(target).and_then(|list| list.first()) {
                let old = previous.effective.get(target);
                let candidate = Self::build_effective(target, &winner.key, &winner.policy, old.map(Arc::as_ref));
                let kept = match old {
                    Some(old) if effective_snapshot_eq(Some(old.as_ref()), Some(&candidate)) => {
                        let kept = Arc::clone(old);
                        drop(candidate);
                        kept
                    }
                    _ => candidate,
                };
                effective.insert(target.clone(), kept);
            }
        }

        if policies_pointer_equivalent(&previous.policies, &next_policies)
            && reverse_index_equivalent(&previous.reverse_index, &reverse_index)
            && effective_pointer_equivalent(&previous.effective, &effective)
        {
            return affected_len;
        }

        let next = Arc::new(EffectivePolicySnapshotData {
            generation: previous
                .generation
                .checked_add(1)
                .expect("BackendTrafficPolicy snapshot generation exhausted"),
            policies: next_policies,
            reverse_index,
            effective,
        });
        self.effective.store(Arc::clone(&next));
        self.apply_background_side_effects(previous, &next, &affected);
        affected_len
    }

    /// Recompute and publish the current policy map (single-writer test/debug helper).
    #[allow(dead_code)]
    pub(crate) fn reapply_target(&self, _target: &PolicyTargetKey) -> bool {
        let previous = self.effective.load_full();
        let before = previous.generation;
        let incoming = previous
            .policies
            .iter()
            .map(|(key, prepared)| (key.clone(), Arc::clone(&prepared.policy)))
            .collect();
        self.full_set_policies(incoming);
        self.effective.load().generation != before
    }

    /// Service-only compatibility seam used by existing tests.
    #[allow(dead_code)]
    pub(crate) fn reapply_service(&self, service_key: &str) -> bool {
        self.reapply_target(&Self::service_target(service_key))
    }

    /// Replace the whole policy cache and reapply every affected typed target.
    /// Returns the affected-key count (metadata only, for logging).
    pub fn full_set_policies(&self, data: HashMap<String, Arc<EdgionBackendTrafficPolicy>>) -> usize {
        let previous = self.effective.load_full();
        let mut affected = HashSet::new();
        for prepared in previous.policies.values() {
            affected.extend(prepared.target_keys.iter().cloned());
        }
        let mut next_policies = HashMap::with_capacity(data.len());
        for (key, policy) in data {
            let prepared = Self::retain_or_prepare_policy(&previous, &key, policy);
            affected.extend(prepared.target_keys.iter().cloned());
            next_policies.insert(key, prepared);
        }
        let reverse_index = Self::build_reverse_index(&next_policies);
        self.publish_generation(&previous, next_policies, reverse_index, affected)
    }

    /// Incremental cache diff: recompute only the typed targets touched by the
    /// changed policies (previous ∪ final target keys), regardless of event
    /// classification. Overlapping inputs use remove > update > add priority.
    /// Returns the affected-key count (metadata only).
    pub fn partial_update_policies(
        &self,
        add: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        update: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        remove: HashSet<String>,
    ) -> usize {
        let mut changes: HashMap<String, Option<Arc<EdgionBackendTrafficPolicy>>> =
            add.into_iter().map(|(key, policy)| (key, Some(policy))).collect();
        changes.extend(update.into_iter().map(|(key, policy)| (key, Some(policy))));
        changes.extend(remove.into_iter().map(|key| (key, None)));

        let previous = self.effective.load_full();
        let mut affected = HashSet::new();
        let mut next_policies = previous.policies.clone();
        let mut reverse_index = previous.reverse_index.clone();

        for (key, incoming) in changes {
            let old = previous.policies.get(&key);
            if let Some(old) = old {
                affected.extend(old.target_keys.iter().cloned());
            }
            let prepared = incoming.map(|policy| Self::retain_or_prepare_policy(&previous, &key, policy));
            if let Some(prepared) = &prepared {
                affected.extend(prepared.target_keys.iter().cloned());
            }
            Self::patch_reverse_index(&mut reverse_index, old, prepared.as_ref());
            match prepared {
                Some(prepared) => {
                    next_policies.insert(key, prepared);
                }
                None => {
                    next_policies.remove(&key);
                }
            }
        }

        self.publish_generation(&previous, next_policies, reverse_index, affected)
    }
}

/// Serializes tests that write the process-global runtime stores. This aliases
/// the cross-module guard because these tests also publish the global degrade
/// threshold; a module-local lock would still race with gateway-config and
/// degrade-threshold tests.
#[cfg(test)]
pub(crate) use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK;

#[cfg(test)]
#[path = "../../../../tests/unit/backends/policy/backend_traffic/store_test/publish_counters.rs"]
mod publish_counters;

#[cfg(test)]
#[path = "../../../../tests/unit/backends/policy/backend_traffic/store_test.rs"]
mod tests;
