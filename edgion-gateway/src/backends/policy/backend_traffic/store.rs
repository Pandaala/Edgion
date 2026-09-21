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
use edgion_resources::resources::edgion_backend_traffic_policy::{
    precedence_key, PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass, RetryConstraintConfig,
};
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
use edgion_resources::resources::upstream_authority::CompiledUpstreamAuthority;
use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
use edgion_resources::resources::EdgionBackendTrafficPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static BACKEND_TRAFFIC_POLICY_STORE: LazyLock<Arc<BackendTrafficPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTrafficPolicyStore::new()));

/// Process-global singleton store.
pub fn get_backend_traffic_policy_store() -> Arc<BackendTrafficPolicyStore> {
    BACKEND_TRAFFIC_POLICY_STORE.clone()
}

/// Store owning the synced policy cache and one typed immutable generation.
pub struct BackendTrafficPolicyStore {
    effective: ArcSwap<EffectivePolicySnapshotData>,
}

/// Prepared resource entry published with the generation.
#[derive(Clone)]
struct PreparedPolicy {
    key: String,
    policy: Arc<EdgionBackendTrafficPolicy>,
    eligible: bool,
    target_keys: Vec<PolicyTargetKey>,
    precedence: (i64, String),
}

/// Effective destination policy captured once by a request. Live retry
/// accounting remains behind its stable `Arc`; the other fields are immutable
/// normalized values.
#[derive(Clone)]
pub struct EffectiveBackendTrafficPolicy {
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
    pub logical_circuit_breaker: Option<Arc<crate::ai::circuit_breaker::LogicalBackendCircuitBreaker>>,
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
    policies: HashMap<String, PreparedPolicy>,
    reverse_index: HashMap<PolicyTargetKey, Vec<PreparedPolicy>>,
    effective: HashMap<PolicyTargetKey, Arc<EffectiveBackendTrafficPolicy>>,
}

/// One immutable, request-capturable view of all effective backend policies.
/// A handle remains valid after later store publications.
#[derive(Clone)]
pub struct BackendTrafficPolicySnapshot {
    inner: Arc<EffectivePolicySnapshotData>,
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
}

fn effective_snapshot_eq(
    left: Option<&EffectiveBackendTrafficPolicy>,
    right: Option<&EffectiveBackendTrafficPolicy>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.resolved_target_class == right.resolved_target_class
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
    left: &HashMap<PolicyTargetKey, Vec<PreparedPolicy>>,
    right: &HashMap<PolicyTargetKey, Vec<PreparedPolicy>>,
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
    left: &HashMap<String, PreparedPolicy>,
    right: &HashMap<String, PreparedPolicy>,
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
        let namespace = policy.metadata.namespace.as_deref().unwrap_or_default();
        policy.spec.canonical_target_keys(namespace).unwrap_or_default()
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
        let resolved_class = policy.spec.resolved_target_class;
        let class_matches = resolved_class.is_some_and(|resolved_class| {
            target_keys.iter().all(|key| {
                matches!(
                    (resolved_class, key.target_kind),
                    (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
                        | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
                )
            })
        });
        let eligible = policy.spec.get_validation_errors().is_empty() && class_matches && !target_keys.is_empty();
        PreparedPolicy {
            key,
            precedence: precedence_key(&policy.metadata),
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

    fn retain_policy_arc(
        previous: &EffectivePolicySnapshotData,
        key: &str,
        incoming: Arc<EdgionBackendTrafficPolicy>,
    ) -> Arc<EdgionBackendTrafficPolicy> {
        previous
            .policies
            .get(key)
            .filter(|old| Self::policy_resource_equivalent(&old.policy, &incoming))
            .map(|old| Arc::clone(&old.policy))
            .unwrap_or(incoming)
    }

    fn build_reverse_index(
        policies: &HashMap<String, PreparedPolicy>,
    ) -> HashMap<PolicyTargetKey, Vec<PreparedPolicy>> {
        #[cfg(test)]
        publish_counters::FULL_INDEX_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let mut index: HashMap<PolicyTargetKey, Vec<PreparedPolicy>> = HashMap::new();
        for prepared in policies.values() {
            if !prepared.eligible {
                continue;
            }
            for target in &prepared.target_keys {
                index.entry(target.clone()).or_default().push(prepared.clone());
            }
        }
        for candidates in index.values_mut() {
            candidates.sort_by_key(|prepared| prepared.precedence.clone());
        }
        index
    }

    fn patch_reverse_index(
        index: &mut HashMap<PolicyTargetKey, Vec<PreparedPolicy>>,
        old_prepared: Option<&PreparedPolicy>,
        new_prepared: Option<&PreparedPolicy>,
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
                    list.push(new.clone());
                }
                list.sort_by_key(|prepared| prepared.precedence.clone());
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
                    _ => Some(crate::ai::circuit_breaker::LogicalBackendCircuitBreaker::new(limit)),
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
                                .and_then(|prepared| prepared.policy.spec.health_check.as_ref())
                                .and_then(|h| h.active.clone());
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
        next_policies: HashMap<String, PreparedPolicy>,
        reverse_index: HashMap<PolicyTargetKey, Vec<PreparedPolicy>>,
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
                let old = previous.effective.get(target).map(|policy| policy.as_ref());
                let candidate = Self::build_effective(target, &winner.key, &winner.policy, old);
                let kept = match old {
                    Some(old) if effective_snapshot_eq(Some(old), Some(&candidate)) => {
                        previous.effective.get(target).cloned().unwrap_or(candidate)
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
            let policy = Self::retain_policy_arc(&previous, &key, policy);
            let prepared = Self::prepare_policy(key.clone(), policy);
            affected.extend(prepared.target_keys.iter().cloned());
            next_policies.insert(key, prepared);
        }
        let reverse_index = Self::build_reverse_index(&next_policies);
        self.publish_generation(&previous, next_policies, reverse_index, affected)
    }

    /// Incremental cache diff: recompute only the typed targets touched by the
    /// changed policies (old target keys of update/remove ∪ new target keys of
    /// add/update). Returns the affected-key count (metadata only).
    pub fn partial_update_policies(
        &self,
        add: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        update: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        remove: HashSet<String>,
    ) -> usize {
        let previous = self.effective.load_full();
        let mut affected = HashSet::new();
        for key in update.keys().chain(remove.iter()) {
            if let Some(old) = previous.policies.get(key) {
                affected.extend(old.target_keys.iter().cloned());
            }
        }
        for policy in add.values().chain(update.values()) {
            affected.extend(Self::dedup_target_keys(Self::canonical_target_keys(policy)));
        }

        let mut next_policies = previous.policies.clone();
        let mut reverse_index = previous.reverse_index.clone();

        for (key, policy) in add {
            let policy = Self::retain_policy_arc(&previous, &key, policy);
            let prepared = Self::prepare_policy(key.clone(), policy);
            Self::patch_reverse_index(&mut reverse_index, None, Some(&prepared));
            next_policies.insert(key, prepared);
        }
        for (key, policy) in update {
            let policy = Self::retain_policy_arc(&previous, &key, policy);
            let prepared = Self::prepare_policy(key.clone(), policy);
            let old = previous.policies.get(&key);
            Self::patch_reverse_index(&mut reverse_index, old, Some(&prepared));
            next_policies.insert(key, prepared);
        }
        for key in &remove {
            if let Some(old) = next_policies.remove(key) {
                Self::patch_reverse_index(&mut reverse_index, Some(&old), None);
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
mod publish_counters {
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static PREPARE_POLICY_CALLS: AtomicUsize = AtomicUsize::new(0);
    pub static FULL_INDEX_BUILDS: AtomicUsize = AtomicUsize::new(0);
    pub static SIDE_EFFECT_CALLS: AtomicUsize = AtomicUsize::new(0);

    pub fn reset() {
        PREPARE_POLICY_CALLS.store(0, Ordering::SeqCst);
        FULL_INDEX_BUILDS.store(0, Ordering::SeqCst);
        SIDE_EFFECT_CALLS.store(0, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::degrade_threshold::{configure_lb_degrade_default, resolve_degrade_threshold};
    use edgion_resources::resources::edgion_backend_traffic_policy::{
        EdgionBackendTrafficPolicySpec, LoadBalancerConfig, LoadBalancerType, PolicyTargetRef,
    };
    use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
    use edgion_resources::resources::outlier_detection::OutlierDetectionConfig;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

    fn ts(rfc3339: &str) -> Time {
        Time(rfc3339.parse().expect("rfc3339"))
    }

    /// Build a policy in `ns` named `name`, targeting Service `svc`, with the
    /// given LB type, degrade threshold, health-check and outlier-detection flags.
    #[allow(clippy::too_many_arguments)]
    fn policy(
        ns: &str,
        name: &str,
        svc: &str,
        creation: Option<Time>,
        lb_type: Option<LoadBalancerType>,
        degrade_threshold: Option<u8>,
        with_hc: bool,
        with_outlier: bool,
    ) -> Arc<EdgionBackendTrafficPolicy> {
        let load_balancer = lb_type.map(|t| LoadBalancerConfig {
            r#type: t,
            consistent_hash: None,
            degrade_threshold,
        });
        let health_check = with_hc.then(|| ServiceHealthCheck {
            active: Some(ActiveHealthCheckConfig::default()),
        });
        let outlier_detection = with_outlier.then_some(OutlierDetectionConfig {
            consecutive_errors: 3,
            consecutive_gateway_errors: None,
            consecutive_local_origin_failures: None,
            ejection_time: "30s".to_string(),
            max_ejection_time: None,
            max_ejection_percent: 50,
        });
        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                creation_timestamp: creation,
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: svc.to_string(),
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
                retry_constraint: None,
                load_balancer,
                health_check,
                outlier_detection,
                upstream_authority: None,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        })
    }

    fn ai_policy(ns: &str, name: &str, target: &str, percent: u8, timeout: &str) -> Arc<EdgionBackendTrafficPolicy> {
        use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;
        use edgion_resources::resources::edgion_backend_traffic_policy::{
            BudgetDetails, ConnectionOverride, RetryConstraintConfig,
        };

        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                creation_timestamp: Some(ts("2026-01-01T00:00:00Z")),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: "edgion.io".to_string(),
                    kind: "EdgionBackend".to_string(),
                    name: target.to_string(),
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::Ai),
                retry_constraint: Some(RetryConstraintConfig {
                    budget: BudgetDetails {
                        percent,
                        interval: "10s".to_string(),
                    },
                    min_retry_rate: None,
                }),
                load_balancer: None,
                health_check: None,
                outlier_detection: Some(OutlierDetectionConfig {
                    consecutive_errors: 4,
                    consecutive_gateway_errors: Some(5),
                    consecutive_local_origin_failures: Some(6),
                    ejection_time: "30s".to_string(),
                    max_ejection_time: Some("5m".to_string()),
                    max_ejection_percent: 50,
                }),
                upstream_authority: None,
                circuit_breaker: Some(CircuitBreakerConfig {
                    max_parallel_requests: 7,
                }),
                connection: Some(ConnectionOverride {
                    connect_timeout: Some(timeout.to_string()),
                }),
            },
            status: None,
        })
    }

    fn service_target(ns: &str, name: &str) -> PolicyTargetKey {
        PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: ns.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn single_policy_populates_all_stores() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6a/svc";
        store.full_set_policies(HashMap::from([(
            "bt6a/p1".to_string(),
            policy(
                "bt6a",
                "p1",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(40),
                true,
                true,
            ),
        )]));

        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));
        assert_eq!(resolve_degrade_threshold(key), 40);
        assert!(get_hc_config_store().resolve(key).is_some(), "HC config present");
        assert!(
            get_outlier_detection_store().keys().contains(&key.to_string()),
            "CB config present"
        );
        assert_eq!(
            store
                .get_effective(&service_target("bt6a", "svc"))
                .expect("effective")
                .winning_policy_key,
            "bt6a/p1"
        );

        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn oldest_wins_then_promotes_on_delete() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6b/svc";
        let older = policy(
            "bt6b",
            "older",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        );
        let newer = policy(
            "bt6b",
            "newer",
            "svc",
            Some(ts("2026-06-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        store.full_set_policies(HashMap::from([
            ("bt6b/older".to_string(), older),
            ("bt6b/newer".to_string(), newer),
        ]));
        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::Ewma), "older wins");

        store.partial_update_policies(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["bt6b/older".to_string()]),
        );
        assert_eq!(
            store.get_lb_policy(key),
            Some(ParsedLBPolicy::LeastConn),
            "newer promoted after older deleted"
        );

        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn last_removal_clears_all_stores() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6c/svc";
        store.full_set_policies(HashMap::from([(
            "bt6c/p1".to_string(),
            policy(
                "bt6c",
                "p1",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(55),
                true,
                true,
            ),
        )]));
        assert!(store.get_lb_policy(key).is_some());

        store.partial_update_policies(HashMap::new(), HashMap::new(), HashSet::from(["bt6c/p1".to_string()]));

        assert_eq!(store.get_lb_policy(key), None, "LB entry removed");
        assert_eq!(
            resolve_degrade_threshold(key),
            0,
            "degrade override cleared to global default"
        );
        assert!(get_hc_config_store().resolve(key).is_none(), "HC config cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key.to_string()),
            "CB config cleared"
        );
    }

    #[test]
    fn reapply_reports_change_on_lb_flip() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6e/svc";
        let first = policy(
            "bt6e",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        assert_eq!(store.snapshot().generation(), 0);
        store.full_set_policies(HashMap::from([("bt6e/p1".to_string(), first.clone())]));
        assert_eq!(store.snapshot().generation(), 1, "first apply is a change");
        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));

        store.full_set_policies(HashMap::from([("bt6e/p1".to_string(), first)]));
        assert_eq!(store.snapshot().generation(), 1, "equivalent republish does not bump");

        store.full_set_policies(HashMap::from([(
            "bt6e/p1".to_string(),
            policy(
                "bt6e",
                "p1",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                None,
                false,
                false,
            ),
        )]));
        assert_eq!(store.snapshot().generation(), 2, "LB flip is a change");
        assert_eq!(
            store.get_lb_policy(key),
            Some(ParsedLBPolicy::Ewma),
            "store entry flipped"
        );

        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn invalid_policy_is_skipped() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6d/svc";
        let invalid = Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("bad".to_string()),
                namespace: Some("bt6d".to_string()),
                creation_timestamp: Some(ts("2026-01-01T00:00:00Z")),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                current_status: None,
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
                retry_constraint: None,
                load_balancer: Some(LoadBalancerConfig {
                    r#type: LoadBalancerType::ConsistentHash,
                    consistent_hash: None,
                    degrade_threshold: None,
                }),
                health_check: None,
                outlier_detection: None,
                upstream_authority: None,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        });
        assert!(
            !invalid.spec.get_validation_errors().is_empty(),
            "fixture must be invalid"
        );
        store.full_set_policies(HashMap::from([("bt6d/bad".to_string(), invalid)]));
        assert_eq!(store.get_lb_policy(key), None, "invalid policy contributes nothing");
        assert!(get_hc_config_store().resolve(key).is_none());
        assert!(
            store.snapshot().candidates(&service_target("bt6d", "svc")).is_empty(),
            "invalid policy is absent from reverse_index"
        );
    }

    #[test]
    fn resolved_marker_withdrawal_promotes_sibling_and_restore_reinstates_oldest() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let target = "marker/svc";
        let older = policy(
            "marker",
            "older",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        let newer = policy(
            "marker",
            "newer",
            "svc",
            Some(ts("2026-02-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        );
        store.full_set_policies(HashMap::from([
            ("marker/older".to_string(), older.clone()),
            ("marker/newer".to_string(), newer),
        ]));
        assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::LeastConn));

        let mut withdrawn = (*older).clone();
        withdrawn.spec.resolved_target_class = None;
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([("marker/older".to_string(), Arc::new(withdrawn))]),
            HashSet::new(),
        );
        assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::Ewma));

        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([("marker/older".to_string(), older)]),
            HashSet::new(),
        );
        assert_eq!(store.get_lb_policy(target), Some(ParsedLBPolicy::LeastConn));
    }

    #[test]
    fn forged_mismatched_resolved_marker_contributes_nothing() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let mut forged = (*policy(
            "marker-mismatch",
            "forged",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        ))
        .clone();
        forged.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Ai);
        assert_eq!(
            store.full_set_policies(HashMap::from([(
                "marker-mismatch/forged".to_string(),
                Arc::new(forged),
            )])),
            1,
            "the typed target is affected even though the forged marker publishes nothing"
        );
        assert_eq!(store.get_lb_policy("marker-mismatch/svc"), None);
    }

    #[test]
    fn full_set_policies_clears_stale_and_populates_new() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key_a = "bt6f/svca";
        let key_b = "bt6g/svcb";

        let mut initial = HashMap::new();
        initial.insert(
            "bt6f/p1".to_string(),
            policy(
                "bt6f",
                "p1",
                "svca",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(40),
                true,
                true,
            ),
        );
        store.full_set_policies(initial);

        assert_eq!(store.get_lb_policy(key_a), Some(ParsedLBPolicy::LeastConn));
        assert_eq!(resolve_degrade_threshold(key_a), 40);
        assert!(get_hc_config_store().resolve(key_a).is_some(), "A HC present");
        assert!(
            get_outlier_detection_store().keys().contains(&key_a.to_string()),
            "A CB present"
        );

        let mut next = HashMap::new();
        next.insert(
            "bt6g/p2".to_string(),
            policy(
                "bt6g",
                "p2",
                "svcb",
                Some(ts("2026-02-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                Some(60),
                true,
                true,
            ),
        );
        store.full_set_policies(next);

        assert_eq!(store.get_lb_policy(key_a), None, "stale A LB cleared");
        assert_eq!(resolve_degrade_threshold(key_a), 0, "falls back to global default");
        assert!(get_hc_config_store().resolve(key_a).is_none(), "A HC cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key_a.to_string()),
            "A CB cleared"
        );

        assert_eq!(store.get_lb_policy(key_b), Some(ParsedLBPolicy::Ewma), "B LB set");
        assert_eq!(resolve_degrade_threshold(key_b), 60);
        assert!(get_hc_config_store().resolve(key_b).is_some(), "B HC present");
        assert!(
            get_outlier_detection_store().keys().contains(&key_b.to_string()),
            "B CB present"
        );

        store.full_set_policies(HashMap::new());
        assert_eq!(store.get_lb_policy(key_b), None, "B LB cleared on empty resync");
        assert_eq!(resolve_degrade_threshold(key_b), 0, "B falls back to global default");
        assert!(get_hc_config_store().resolve(key_b).is_none(), "B HC cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key_b.to_string()),
            "B CB cleared"
        );
    }

    #[test]
    fn upstream_authority_resolves_and_clears() {
        use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6h/svc";

        let mut p = policy(
            "bt6h",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.sandbox.internal".to_string(),
            health_check_host: None,
        });
        store.full_set_policies(HashMap::from([("bt6h/p1".to_string(), p)]));
        let resolved = store.get_authority(key).expect("authority resolved");
        assert_eq!(resolved.suffix, ".sandbox.internal");
        assert_eq!(resolved.compose("tenant-a"), "tenant-a.sandbox.internal");

        store.full_set_policies(HashMap::new());
        assert!(store.get_authority(key).is_none(), "cleared on removal");
    }

    #[test]
    fn retry_budget_resolves_with_keep_if_unchanged() {
        use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6i/svc";

        let mut p = policy(
            "bt6i",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p).unwrap().spec.retry_constraint = Some(RetryConstraintConfig {
            budget: BudgetDetails {
                percent: 20,
                interval: "10s".to_string(),
            },
            min_retry_rate: None,
        });
        store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p.clone())]));
        let first = store.get_retry_budget(key).expect("retry budget resolved");

        store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p)]));
        let second = store.get_retry_budget(key).expect("retry budget still present");
        assert!(
            Arc::ptr_eq(&first, &second),
            "unchanged config must keep the same Arc (live counters preserved)"
        );

        let mut p2 = policy(
            "bt6i",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p2).unwrap().spec.retry_constraint = Some(RetryConstraintConfig {
            budget: BudgetDetails {
                percent: 50,
                interval: "10s".to_string(),
            },
            min_retry_rate: None,
        });
        store.full_set_policies(HashMap::from([("bt6i/p1".to_string(), p2)]));
        let third = store.get_retry_budget(key).expect("retry budget resolved after change");
        assert!(!Arc::ptr_eq(&second, &third), "changed config must build a new Arc");

        store.full_set_policies(HashMap::new());
        assert!(store.get_retry_budget(key).is_none(), "cleared on removal");
    }

    #[test]
    fn circuit_breaker_limit_resolves_and_clears() {
        use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6j/svc";

        let mut p = policy(
            "bt6j",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p).unwrap().spec.circuit_breaker = Some(CircuitBreakerConfig {
            max_parallel_requests: 5,
        });
        store.full_set_policies(HashMap::from([("bt6j/p1".to_string(), p)]));
        assert_eq!(store.get_circuit_breaker_limit(key), Some(5));

        store.full_set_policies(HashMap::new());
        assert_eq!(store.get_circuit_breaker_limit(key), None, "cleared on removal");
    }

    #[test]
    fn connect_timeout_resolves_and_clears() {
        use edgion_resources::resources::edgion_backend_traffic_policy::ConnectionOverride;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6k/svc";

        let mut p = policy(
            "bt6k",
            "p1",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p).unwrap().spec.connection = Some(ConnectionOverride {
            connect_timeout: Some("250ms".to_string()),
        });
        store.full_set_policies(HashMap::from([("bt6k/p1".to_string(), p)]));
        assert_eq!(
            store.get_connect_timeout(key),
            Some(std::time::Duration::from_millis(250))
        );

        store.full_set_policies(HashMap::new());
        assert_eq!(store.get_connect_timeout(key), None, "cleared on removal");
    }

    #[test]
    fn typed_service_and_ai_targets_with_same_name_do_not_collide() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let service_target = PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "typed".to_string(),
            name: "shared".to_string(),
        };
        let ai_target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "typed".to_string(),
            name: "shared".to_string(),
        };

        let mut policies = HashMap::new();
        policies.insert(
            "typed/service-policy".to_string(),
            policy(
                "typed",
                "service-policy",
                "shared",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                None,
                false,
                false,
            ),
        );
        policies.insert(
            "typed/ai-policy".to_string(),
            ai_policy("typed", "ai-policy", "shared", 25, "750ms"),
        );
        assert_eq!(store.full_set_policies(policies), 2);

        assert_eq!(store.get_lb_policy("typed/shared"), Some(ParsedLBPolicy::LeastConn));
        let service = store.get_effective(&service_target).expect("Service snapshot");
        assert!(service.retry_budget.is_none());
        assert!(service.ai_credential_outlier.is_none());
        assert_eq!(service.winning_policy_key, "typed/service-policy");

        let ai = store.get_effective(&ai_target).expect("AI snapshot");
        assert!(ai.ai_credential_outlier.is_some());
        assert!(ai.retry_budget.is_some());
        assert_eq!(ai.circuit_breaker_limit, Some(7));
        assert_eq!(ai.connect_timeout, Some(std::time::Duration::from_millis(750)));
        let outlier = ai.outlier_detection.as_ref().expect("AI outlier config");
        assert_eq!(outlier.consecutive_errors, 4);
        assert_eq!(outlier.consecutive_gateway_errors, 5);
        assert_eq!(outlier.consecutive_local_origin_failures, 6);
        assert_eq!(ai.winning_policy_key, "typed/ai-policy");
    }

    #[test]
    fn equivalent_ai_update_keeps_retry_handle_and_field_update_is_atomic() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "typed-update".to_string(),
            name: "models".to_string(),
        };
        let policy_key = "typed-update/policy".to_string();

        let mut initial = HashMap::new();
        initial.insert(
            policy_key.clone(),
            ai_policy("typed-update", "policy", "models", 20, "250ms"),
        );
        store.full_set_policies(initial);
        let first_snapshot = store.snapshot();
        let first = first_snapshot.get_effective(&target).expect("initial snapshot");
        let first_budget = first.retry_budget.as_ref().expect("retry budget");
        let first_outlier = first.ai_credential_outlier.as_ref().expect("AI outlier generation");

        let mut equivalent = HashMap::new();
        equivalent.insert(
            policy_key.clone(),
            ai_policy("typed-update", "policy", "models", 20, "250ms"),
        );
        store.full_set_policies(equivalent);
        let equivalent_snapshot = store.snapshot();
        let second = equivalent_snapshot.get_effective(&target).expect("equivalent snapshot");
        assert_eq!(first_snapshot.generation(), equivalent_snapshot.generation());
        assert!(Arc::ptr_eq(&first, &second), "equivalent outer Arc retained");
        assert!(Arc::ptr_eq(
            first_budget,
            second.retry_budget.as_ref().expect("retry budget retained")
        ));
        assert!(Arc::ptr_eq(
            first_outlier,
            second
                .ai_credential_outlier
                .as_ref()
                .expect("outlier generation retained")
        ));

        let mut changed = HashMap::new();
        changed.insert(policy_key, ai_policy("typed-update", "policy", "models", 20, "2s"));
        store.full_set_policies(changed);
        let third = store.get_effective(&target).expect("updated snapshot");
        assert_eq!(third.connect_timeout, Some(std::time::Duration::from_secs(2)));
        assert_eq!(third.circuit_breaker_limit, Some(7));
        assert!(third.outlier_detection.is_some());
        assert!(Arc::ptr_eq(
            second.retry_budget.as_ref().unwrap(),
            third.retry_budget.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            second.ai_credential_outlier.as_ref().unwrap(),
            third.ai_credential_outlier.as_ref().unwrap()
        ));

        let mut outlier_changed = ai_policy("typed-update", "policy", "models", 20, "2s");
        Arc::get_mut(&mut outlier_changed)
            .unwrap()
            .spec
            .outlier_detection
            .as_mut()
            .unwrap()
            .consecutive_errors = 8;
        store.full_set_policies(HashMap::from([("typed-update/policy".to_string(), outlier_changed)]));
        let fourth = store.get_effective(&target).expect("outlier-updated snapshot");
        assert!(!Arc::ptr_eq(
            third.ai_credential_outlier.as_ref().unwrap(),
            fourth.ai_credential_outlier.as_ref().unwrap()
        ));

        store.full_set_policies(HashMap::new());
        assert!(store.get_effective(&target).is_none());
        assert!(get_ai_credential_outlier_store()
            .effective_handle_for_test(&target)
            .is_none());
        assert_eq!(first.connect_timeout, Some(std::time::Duration::from_millis(250)));
    }

    #[test]
    fn multi_target_reconcile_publishes_one_coherent_generation() {
        let store = BackendTrafficPolicyStore::new();
        let target_a = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "batch".to_string(),
            name: "a".to_string(),
        };
        let target_b = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "batch".to_string(),
            name: "b".to_string(),
        };
        let before = store.snapshot();
        assert_eq!(before.generation(), 0);

        store.full_set_policies(HashMap::from([
            (
                "batch/policy-a".to_string(),
                ai_policy("batch", "policy-a", "a", 20, "100ms"),
            ),
            (
                "batch/policy-b".to_string(),
                ai_policy("batch", "policy-b", "b", 20, "200ms"),
            ),
        ]));
        let after = store.snapshot();
        assert_eq!(after.generation(), 1, "one batch yields one publication");
        assert_eq!(
            after.get_effective(&target_a).unwrap().connect_timeout,
            Some(std::time::Duration::from_millis(100))
        );
        assert_eq!(
            after.get_effective(&target_b).unwrap().connect_timeout,
            Some(std::time::Duration::from_millis(200))
        );
        assert!(before.get_effective(&target_a).is_none(), "old handle is immutable");
    }

    #[test]
    fn multi_target_partial_update_publishes_both_ai_targets_atomically() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let target_a = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "partial-batch".to_string(),
            name: "a".to_string(),
        };
        let target_b = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "partial-batch".to_string(),
            name: "b".to_string(),
        };
        let policy_key = "partial-batch/policy".to_string();

        let mut initial = ai_policy("partial-batch", "policy", "a", 20, "100ms");
        Arc::get_mut(&mut initial)
            .unwrap()
            .spec
            .target_refs
            .push(PolicyTargetRef {
                group: "edgion.io".to_string(),
                kind: "EdgionBackend".to_string(),
                name: "b".to_string(),
            });
        assert_eq!(
            store.partial_update_policies(
                HashMap::from([(policy_key.clone(), initial)]),
                HashMap::new(),
                HashSet::new(),
            ),
            2
        );
        let first = store.snapshot();
        assert_eq!(first.generation(), 1, "one multi-target add yields one publication");
        assert_eq!(
            first.get_effective(&target_a).unwrap().connect_timeout,
            Some(std::time::Duration::from_millis(100))
        );
        assert_eq!(
            first.get_effective(&target_b).unwrap().connect_timeout,
            Some(std::time::Duration::from_millis(100))
        );

        let mut updated = ai_policy("partial-batch", "policy", "a", 20, "2s");
        Arc::get_mut(&mut updated)
            .unwrap()
            .spec
            .target_refs
            .push(PolicyTargetRef {
                group: "edgion.io".to_string(),
                kind: "EdgionBackend".to_string(),
                name: "b".to_string(),
            });
        assert_eq!(
            store.partial_update_policies(HashMap::new(), HashMap::from([(policy_key, updated)]), HashSet::new(),),
            2
        );
        let second = store.snapshot();
        assert_eq!(second.generation(), 2, "one multi-target update yields one publication");
        for target in [&target_a, &target_b] {
            assert_eq!(
                second.get_effective(target).unwrap().connect_timeout,
                Some(std::time::Duration::from_secs(2))
            );
            assert_eq!(
                first.get_effective(target).unwrap().connect_timeout,
                Some(std::time::Duration::from_millis(100)),
                "the captured pre-update generation stays immutable"
            );
        }
    }

    #[test]
    fn lb_only_update_bumps_generation_exactly_once() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        store.full_set_policies(HashMap::from([(
            "gen/p".to_string(),
            policy(
                "gen",
                "p",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                None,
                false,
                false,
            ),
        )]));
        let first = store.snapshot();
        store.full_set_policies(HashMap::from([(
            "gen/p".to_string(),
            policy(
                "gen",
                "p",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                None,
                false,
                false,
            ),
        )]));
        let second = store.snapshot();
        assert_eq!(second.generation(), first.generation() + 1);
        store.full_set_policies(HashMap::from([(
            "gen/p".to_string(),
            policy(
                "gen",
                "p",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::Ewma),
                None,
                false,
                false,
            ),
        )]));
        assert_eq!(store.snapshot().generation(), second.generation());
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn authority_only_update_bumps_generation_exactly_once() {
        use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let mut first = policy(
            "auth",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut first).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
            pattern: "*.a.internal".to_string(),
            template: "${ctx:tenant}.a.internal".to_string(),
            health_check_host: None,
        });
        store.full_set_policies(HashMap::from([("auth/p".to_string(), first)]));
        let g1 = store.snapshot().generation();

        let mut second = policy(
            "auth",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            None,
            None,
            false,
            false,
        );
        Arc::get_mut(&mut second).unwrap().spec.upstream_authority = Some(UpstreamAuthorityConfig {
            pattern: "*.b.internal".to_string(),
            template: "${ctx:tenant}.b.internal".to_string(),
            health_check_host: None,
        });
        store.full_set_policies(HashMap::from([("auth/p".to_string(), second.clone())]));
        assert_eq!(store.snapshot().generation(), g1 + 1);
        store.full_set_policies(HashMap::from([("auth/p".to_string(), second)]));
        assert_eq!(store.snapshot().generation(), g1 + 1);
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn degrade_override_only_update_bumps_generation() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        store.full_set_policies(HashMap::from([(
            "degrade/p".to_string(),
            policy(
                "degrade",
                "p",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(20),
                false,
                false,
            ),
        )]));
        let g1 = store.snapshot().generation();
        store.full_set_policies(HashMap::from([(
            "degrade/p".to_string(),
            policy(
                "degrade",
                "p",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(80),
                false,
                false,
            ),
        )]));
        assert_eq!(store.snapshot().generation(), g1 + 1);
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn equivalent_full_and_partial_do_not_bump() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let p = policy(
            "eq",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        store.full_set_policies(HashMap::from([("eq/p".to_string(), p.clone())]));
        let g1 = store.snapshot().generation();
        publish_counters::reset();
        store.full_set_policies(HashMap::from([("eq/p".to_string(), p.clone())]));
        store.partial_update_policies(HashMap::new(), HashMap::from([("eq/p".to_string(), p)]), HashSet::new());
        assert_eq!(store.snapshot().generation(), g1);
        assert_eq!(
            publish_counters::SIDE_EFFECT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn real_change_full_and_partial_match() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let initial = policy(
            "match",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        let updated = policy(
            "match",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::Ewma),
            None,
            false,
            false,
        );
        let full = BackendTrafficPolicyStore::new();
        full.full_set_policies(HashMap::from([("match/p".to_string(), updated.clone())]));
        let partial = BackendTrafficPolicyStore::new();
        partial.full_set_policies(HashMap::from([("match/p".to_string(), initial)]));
        partial.partial_update_policies(
            HashMap::new(),
            HashMap::from([("match/p".to_string(), updated)]),
            HashSet::new(),
        );
        let target = service_target("match", "svc");
        let left = full.get_effective(&target).expect("full");
        let right = partial.get_effective(&target).expect("partial");
        assert_eq!(left.winning_policy_key, right.winning_policy_key);
        assert_eq!(left.load_balancer, right.load_balancer);
        assert_eq!(
            full.snapshot().candidates(&target).len(),
            partial.snapshot().candidates(&target).len()
        );
        full.full_set_policies(HashMap::new());
        partial.full_set_policies(HashMap::new());
    }

    #[test]
    fn retarget_clears_reverse_index_and_side_effects() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let store = BackendTrafficPolicyStore::new();
        store.full_set_policies(HashMap::from([(
            "rt/p".to_string(),
            policy(
                "rt",
                "p",
                "a",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                Some(40),
                true,
                true,
            ),
        )]));
        assert!(store.get_lb_policy("rt/a").is_some());
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([(
                "rt/p".to_string(),
                policy(
                    "rt",
                    "p",
                    "b",
                    Some(ts("2026-01-01T00:00:00Z")),
                    Some(LoadBalancerType::Ewma),
                    Some(60),
                    true,
                    true,
                ),
            )]),
            HashSet::new(),
        );
        assert!(store.snapshot().candidates(&service_target("rt", "a")).is_empty());
        assert_eq!(store.get_lb_policy("rt/a"), None);
        assert!(get_hc_config_store().resolve("rt/a").is_none());
        assert_eq!(store.get_lb_policy("rt/b"), Some(ParsedLBPolicy::Ewma));
        assert!(!store.snapshot().candidates(&service_target("rt", "b")).is_empty());
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn prepare_policy_dedups_repeated_service_target_refs() {
        let mut p = policy(
            "dup",
            "p",
            "svc",
            Some(ts("2026-01-01T00:00:00Z")),
            Some(LoadBalancerType::LeastConn),
            None,
            false,
            false,
        );
        Arc::get_mut(&mut p).unwrap().spec.target_refs.push(PolicyTargetRef {
            group: String::new(),
            kind: "Service".to_string(),
            name: "svc".to_string(),
        });
        let prepared = BackendTrafficPolicyStore::prepare_policy("dup/p".to_string(), p);
        assert_eq!(prepared.target_keys.len(), 1);
        assert!(prepared.eligible);
    }

    #[test]
    fn partial_update_does_not_scan_all_policies() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let mut initial = HashMap::new();
        for i in 0..8 {
            let name = format!("p{i}");
            initial.insert(
                format!("scan/{name}"),
                policy(
                    "scan",
                    &name,
                    &format!("svc{i}"),
                    Some(ts("2026-01-01T00:00:00Z")),
                    Some(LoadBalancerType::LeastConn),
                    None,
                    false,
                    false,
                ),
            );
        }
        store.full_set_policies(initial);
        publish_counters::reset();
        store.partial_update_policies(
            HashMap::new(),
            HashMap::from([(
                "scan/p0".to_string(),
                policy(
                    "scan",
                    "p0",
                    "svc0",
                    Some(ts("2026-01-01T00:00:00Z")),
                    Some(LoadBalancerType::Ewma),
                    None,
                    false,
                    false,
                ),
            )]),
            HashSet::new(),
        );
        assert_eq!(
            publish_counters::PREPARE_POLICY_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            publish_counters::FULL_INDEX_BUILDS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn ai_outlier_handle_materialized_before_store() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "mat".to_string(),
            name: "models".to_string(),
        };
        store.full_set_policies(HashMap::from([(
            "mat/policy".to_string(),
            ai_policy("mat", "policy", "models", 20, "250ms"),
        )]));
        let snap = store.get_effective(&target).expect("effective");
        let handle = snap.ai_credential_outlier.as_ref().expect("handle on snapshot");
        let published = get_ai_credential_outlier_store()
            .effective_handle_for_test(&target)
            .expect("handle in outlier store");
        assert!(Arc::ptr_eq(handle, &published));
        store.full_set_policies(HashMap::new());
    }

    #[test]
    fn ai_policy_publication_gaps_never_alias_the_backend_fallback() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "after".to_string(),
            name: "models".to_string(),
        };
        let outliers = get_ai_credential_outlier_store();
        outliers.clear_effective_policy(&target);
        outliers.remove_credentials(&target);
        let fallback = outliers
            .set_credentials(target.clone(), ["primary".to_string()])
            .expect("config-free fallback");

        // `build_effective` deliberately materializes the effective role before
        // its containing ArcSwap generation is stored. The live backend
        // fallback must remain a different generation during that add gap.
        let policy = ai_policy("after", "policy", "models", 20, "250ms");
        let candidate = BackendTrafficPolicyStore::build_effective(&target, "after/policy", policy.as_ref(), None);
        let effective = candidate.ai_credential_outlier.as_ref().expect("effective generation");
        assert!(!Arc::ptr_eq(&fallback, effective));
        assert!(outliers
            .fallback_handle_for_test(&target)
            .is_some_and(|published| Arc::ptr_eq(&fallback, &published)));

        // During removal, the ArcSwap without an effective policy is visible
        // before cold cleanup clears this role. Fallback selection is still
        // stable both before and after that cleanup.
        assert!(outliers.effective_handle_for_test(&target).is_some());
        outliers.clear_effective_policy(&target);
        assert!(outliers.effective_handle_for_test(&target).is_none());
        assert!(outliers
            .fallback_handle_for_test(&target)
            .is_some_and(|published| Arc::ptr_eq(&fallback, &published)));
        assert!(effective.capture("primary").is_some());

        outliers.remove_credentials(&target);
        assert!(!outliers.keys().contains(&target.to_string()));
    }
}
