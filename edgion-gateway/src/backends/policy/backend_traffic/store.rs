//! Runtime store + conflict arbitration for `EdgionBackendTrafficPolicy`.
//!
//! Holds the synced policy cache and resolves oldest-wins policy independently
//! per typed Service or EdgionBackend destination. Service-only LB, health,
//! panic-threshold, and authority consumers retain their existing string-keyed
//! stores; connection, retry, circuit, and outlier configuration is also
//! published in one typed immutable snapshot for AI request capture.

use super::retry_budget::RetryBudget;
use crate::ai::{get_ai_credential_outlier_store, AiCredentialOutlierBackendHandle};
use crate::backends::health::check::{get_hc_config_store, get_health_check_manager};
use crate::backends::outlier_detection::get_outlier_detection_store;
use crate::backends::panic_threshold::get_panic_threshold_store;
use arc_swap::ArcSwap;
use dashmap::DashMap;
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

/// Store owning the synced policy cache, typed winners/effective snapshots, and
/// the existing Service-only request-path maps.
pub struct BackendTrafficPolicyStore {
    /// Synced policies keyed by resource key (`namespace/name`).
    policies: DashMap<String, Arc<EdgionBackendTrafficPolicy>>,
    /// Winning policy key per typed destination identity.
    applied: DashMap<PolicyTargetKey, String>,
    /// Immutable effective policy snapshots for request-scoped capture.
    effective: ArcSwap<EffectivePolicySnapshotData>,
    /// Resolved load-balancer policy per Service key (read on the request path).
    lb_policies: DashMap<String, ParsedLBPolicy>,
    /// Resolved upstream authority per Service key (read on the request path).
    authorities: DashMap<String, Arc<CompiledUpstreamAuthority>>,
    /// Resolved retry budget per Service key, paired with the source config it
    /// was built from (read on the request path). Unlike `lb_policies` /
    /// `authorities`, the budget carries live counters, so it is kept-if-
    /// unchanged across recomputes rather than blindly rebuilt (D6): the
    /// resolve path only replaces the `Arc` when the stored config differs
    /// from the newly resolved one.
    retry_budgets: DashMap<PolicyTargetKey, (RetryConstraintConfig, Arc<RetryBudget>)>,
    /// Service-only compatibility caches preserve the existing allocation-free
    /// request-path lookups. The typed ArcSwap snapshot remains canonical.
    service_retry_budgets: DashMap<String, Arc<RetryBudget>>,
    /// Per-service circuit-breaker concurrency cap (read on the request path).
    circuit_breakers: DashMap<PolicyTargetKey, u32>,
    service_circuit_breakers: DashMap<String, u32>,
    logical_circuit_breakers:
        DashMap<PolicyTargetKey, (u32, Arc<crate::ai::circuit_breaker::LogicalBackendCircuitBreaker>)>,
    /// Per-service resolved HTTP upstream connect deadline (read on the request path).
    connect_timeouts: DashMap<PolicyTargetKey, std::time::Duration>,
    service_connect_timeouts: DashMap<String, std::time::Duration>,
}

/// Effective destination policy captured once by a request. Live retry
/// accounting remains behind its stable `Arc`; the other fields are immutable
/// normalized values suitable for future AI-attempt state handles.
#[derive(Clone)]
pub struct EffectiveBackendTrafficPolicy {
    pub resolved_target_class: ResolvedPolicyTargetClass,
    pub load_balancer: Option<ParsedLBPolicy>,
    pub upstream_authority: Option<Arc<CompiledUpstreamAuthority>>,
    pub(crate) upstream_authority_config: Option<UpstreamAuthorityConfig>,
    pub retry_budget: Option<Arc<RetryBudget>>,
    pub circuit_breaker_limit: Option<u32>,
    pub connect_timeout: Option<std::time::Duration>,
    pub outlier_detection: Option<OutlierDetectionResolved>,
    /// Exact AI credential-outlier policy generation for request capture.
    /// `None` for core Service targets.
    pub ai_credential_outlier: Option<Arc<AiCredentialOutlierBackendHandle>>,
    pub logical_circuit_breaker: Option<Arc<crate::ai::circuit_breaker::LogicalBackendCircuitBreaker>>,
}

struct EffectivePolicySnapshotData {
    generation: u64,
    policies: HashMap<PolicyTargetKey, Arc<EffectiveBackendTrafficPolicy>>,
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
        self.inner.policies.get(target).cloned()
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
                && left.circuit_breaker_limit == right.circuit_breaker_limit
                && left.load_balancer == right.load_balancer
                && left.upstream_authority_config == right.upstream_authority_config
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

impl Default for BackendTrafficPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendTrafficPolicyStore {
    pub fn new() -> Self {
        Self {
            policies: DashMap::new(),
            applied: DashMap::new(),
            effective: ArcSwap::from_pointee(EffectivePolicySnapshotData {
                generation: 0,
                policies: HashMap::new(),
            }),
            lb_policies: DashMap::new(),
            authorities: DashMap::new(),
            retry_budgets: DashMap::new(),
            service_retry_budgets: DashMap::new(),
            circuit_breakers: DashMap::new(),
            service_circuit_breakers: DashMap::new(),
            logical_circuit_breakers: DashMap::new(),
            connect_timeouts: DashMap::new(),
            service_connect_timeouts: DashMap::new(),
        }
    }

    /// Typed destination keys a valid policy attaches to. Same-namespace
    /// attachment is enforced by the schema.
    fn canonical_target_keys(policy: &EdgionBackendTrafficPolicy) -> Vec<PolicyTargetKey> {
        let namespace = policy.metadata.namespace.as_deref().unwrap_or_default();
        policy.spec.canonical_target_keys(namespace).unwrap_or_default()
    }

    fn policy_target_keys(&self, policy: &EdgionBackendTrafficPolicy) -> Vec<PolicyTargetKey> {
        let Some(resolved_class) = policy.spec.resolved_target_class else {
            return Vec::new();
        };
        let keys = Self::canonical_target_keys(policy);
        let matches_resolved_class = keys.iter().all(|key| {
            matches!(
                (resolved_class, key.target_kind),
                (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
                    | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
            )
        });
        if !matches_resolved_class {
            return Vec::new();
        }
        keys
    }

    fn service_target(service_key: &str) -> PolicyTargetKey {
        let (namespace, name) = service_key.split_once('/').unwrap_or(("", service_key));
        PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    }

    /// Resolved load-balancer policy for a Service key. Request-path read:
    /// a single `DashMap` get + clone, no tracing.
    pub fn get_lb_policy(&self, service_key: &str) -> Option<ParsedLBPolicy> {
        self.lb_policies.get(service_key).map(|v| v.value().clone())
    }

    /// Resolved upstream authority for a Service key. Request-path read:
    /// a single `DashMap` get + `Arc` clone, no tracing.
    pub fn get_authority(&self, service_key: &str) -> Option<Arc<CompiledUpstreamAuthority>> {
        self.authorities.get(service_key).map(|v| Arc::clone(v.value()))
    }

    /// Resolved retry budget for a Service key. Request-path read: a single
    /// `DashMap` get + `Arc` clone, no tracing.
    pub fn get_retry_budget(&self, service_key: &str) -> Option<Arc<RetryBudget>> {
        self.service_retry_budgets
            .get(service_key)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// Per-endpoint concurrency cap for this service, if configured.
    pub fn get_circuit_breaker_limit(&self, service_key: &str) -> Option<u32> {
        self.service_circuit_breakers
            .get(service_key)
            .map(|entry| *entry.value())
    }

    /// Resolved HTTP upstream connect deadline for this service, if overridden.
    pub fn get_connect_timeout(&self, service_key: &str) -> Option<std::time::Duration> {
        self.service_connect_timeouts
            .get(service_key)
            .map(|entry| *entry.value())
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
        self.lb_policies
            .iter()
            .map(|e| (e.key().clone(), format!("{:?}", e.value())))
            .collect()
    }

    fn sync_service_effective_cache(&self, service_key: &str, snapshot: Option<&EffectiveBackendTrafficPolicy>) {
        match snapshot.and_then(|value| value.retry_budget.as_ref()) {
            Some(budget) => {
                self.service_retry_budgets
                    .insert(service_key.to_string(), Arc::clone(budget));
            }
            None => {
                self.service_retry_budgets.remove(service_key);
            }
        }
        match snapshot.and_then(|value| value.circuit_breaker_limit) {
            Some(limit) => {
                self.service_circuit_breakers.insert(service_key.to_string(), limit);
            }
            None => {
                self.service_circuit_breakers.remove(service_key);
            }
        }
        match snapshot.and_then(|value| value.connect_timeout) {
            Some(timeout) => {
                self.service_connect_timeouts.insert(service_key.to_string(), timeout);
            }
            None => {
                self.service_connect_timeouts.remove(service_key);
            }
        }
    }

    /// Recompute and apply the winning policy for one typed destination.
    /// Returns true when the effective config changed.
    fn resolve_target(
        &self,
        target: &PolicyTargetKey,
        old_effective: Option<&Arc<EffectiveBackendTrafficPolicy>>,
    ) -> (bool, Option<Arc<EffectiveBackendTrafficPolicy>>) {
        // Winner = min over valid cached policies targeting this key, ordered by
        // precedence_key (LOWER wins). Invalid policies are skipped entirely.
        let mut winner: Option<((i64, String), String, Arc<EdgionBackendTrafficPolicy>)> = None;
        for entry in self.policies.iter() {
            let policy = entry.value();
            if !policy.spec.get_validation_errors().is_empty() {
                continue;
            }
            if !self.policy_target_keys(policy).iter().any(|key| key == target) {
                continue;
            }
            let pk = precedence_key(&policy.metadata);
            let take = match &winner {
                Some((best, _, _)) => pk < *best,
                None => true,
            };
            if take {
                winner = Some((pk, entry.key().clone(), Arc::clone(policy)));
            }
        }

        let service_key =
            (target.target_kind == PolicyTargetKind::Service).then(|| format!("{}/{}", target.namespace, target.name));
        let old_lb = service_key.as_deref().and_then(|key| self.get_lb_policy(key));
        let old_winner = self.applied.get(target).map(|v| v.value().clone());
        let old_effective = old_effective.cloned();

        let hc_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let cb_store = get_outlier_detection_store();
        let pt_store = get_panic_threshold_store();

        let (new_lb, new_winner, new_effective) = match winner {
            Some((_, policy_key_str, policy)) => {
                let normalized_outlier = policy.spec.outlier_detection.clone().map(|od| od.normalize());
                let mut lb = None;

                if let Some(service_key) = service_key.as_deref() {
                    // Must run BEFORE `hc_store.set_service_config` + `hc_manager.reconcile_service`
                    // below: `reconcile_service` spawns a fresh `health_check_loop` task, which
                    // resolves the probe `Host` via `get_authority(service_key)` exactly ONCE at
                    // task startup and never refreshes it. On a multi-worker runtime that spawned
                    // task can be polled before this write lands, so it would read the stale (or
                    // absent) authority and probe with the wrong Host until the next policy change
                    // respawns it. Writing the authority first guarantees the spawned probe always
                    // observes the fresh value.
                    //
                    // `compile` cannot fail here: `get_validation_errors()` (checked above)
                    // already rejected every spec whose authority does not compile.
                    match policy
                        .spec
                        .upstream_authority
                        .as_ref()
                        .and_then(|ua| CompiledUpstreamAuthority::compile(ua).ok())
                    {
                        Some(c) => {
                            self.authorities.insert(service_key.to_string(), Arc::new(c));
                        }
                        None => {
                            self.authorities.remove(service_key);
                        }
                    }

                    let hc = policy.spec.health_check.as_ref().and_then(|h| h.active.clone());
                    hc_store.set_service_config(service_key, hc);
                    hc_manager.reconcile_service(service_key);

                    cb_store.set_service_config(service_key, normalized_outlier.clone());

                    let pt = policy.spec.load_balancer.as_ref().and_then(|lb| lb.panic_threshold);
                    pt_store.set_service_threshold(service_key, pt);

                    lb = policy.spec.load_balancer.as_ref().and_then(|lb| lb.to_parsed());
                    match &lb {
                        Some(value) => {
                            self.lb_policies.insert(service_key.to_string(), value.clone());
                        }
                        None => {
                            self.lb_policies.remove(service_key);
                        }
                    }
                }

                // Retry budget: unlike `lb_policies`/`authorities` above, this map
                // carries live counters, so it is kept-if-unchanged (D6) — only a
                // real config diff (or the config newly appearing) rebuilds the
                // `Arc`; a byte-identical reapply (full resync, reconnect, an
                // unrelated field edit on the winning policy) must NOT reset
                // in-flight accounting.
                match policy.spec.retry_constraint.as_ref() {
                    Some(cfg) => {
                        let unchanged = self
                            .retry_budgets
                            .get(target)
                            .is_some_and(|entry| &entry.value().0 == cfg);
                        if !unchanged {
                            self.retry_budgets
                                .insert(target.clone(), (cfg.clone(), Arc::new(RetryBudget::from_config(cfg))));
                        }
                    }
                    None => {
                        self.retry_budgets.remove(target);
                    }
                }

                match &policy.spec.circuit_breaker {
                    Some(cb) => {
                        self.circuit_breakers.insert(target.clone(), cb.max_parallel_requests);
                        if target.target_kind == PolicyTargetKind::EdgionBackend {
                            let unchanged = self
                                .logical_circuit_breakers
                                .get(target)
                                .is_some_and(|entry| entry.value().0 == cb.max_parallel_requests);
                            if !unchanged {
                                self.logical_circuit_breakers.insert(
                                    target.clone(),
                                    (
                                        cb.max_parallel_requests,
                                        crate::ai::circuit_breaker::LogicalBackendCircuitBreaker::new(
                                            cb.max_parallel_requests,
                                        ),
                                    ),
                                );
                            }
                        }
                    }
                    None => {
                        self.circuit_breakers.remove(target);
                        self.logical_circuit_breakers.remove(target);
                    }
                }

                match policy
                    .spec
                    .connection
                    .as_ref()
                    .and_then(|c| c.connect_timeout.as_deref())
                {
                    Some(s) => match edgion_resources::duration::parse_duration(s) {
                        Ok(d) => {
                            self.connect_timeouts.insert(target.clone(), d);
                        }
                        Err(_) => {
                            self.connect_timeouts.remove(target);
                        }
                    },
                    None => {
                        self.connect_timeouts.remove(target);
                    }
                }

                self.applied.insert(target.clone(), policy_key_str.clone());
                let target_class = policy.spec.resolved_target_class;
                let is_ai = target_class == Some(ResolvedPolicyTargetClass::Ai);
                let ai_credential_outlier = is_ai
                    .then(|| {
                        get_ai_credential_outlier_store().publish_policy(target.clone(), normalized_outlier.clone())
                    })
                    .flatten();
                let candidate = Arc::new(EffectiveBackendTrafficPolicy {
                    resolved_target_class: target_class.expect("eligible policy has a resolved target class"),
                    load_balancer: None,
                    upstream_authority: None,
                    upstream_authority_config: None,
                    retry_budget: self.retry_budgets.get(target).map(|entry| Arc::clone(&entry.value().1)),
                    circuit_breaker_limit: self.circuit_breakers.get(target).map(|entry| *entry.value()),
                    connect_timeout: self.connect_timeouts.get(target).map(|entry| *entry.value()),
                    outlier_detection: is_ai.then_some(normalized_outlier).flatten(),
                    ai_credential_outlier,
                    logical_circuit_breaker: self
                        .logical_circuit_breakers
                        .get(target)
                        .map(|entry| Arc::clone(&entry.value().1)),
                });
                let effective = match old_effective.as_ref() {
                    Some(old) if effective_snapshot_eq(Some(old), Some(&candidate)) => Arc::clone(old),
                    _ => candidate,
                };
                if let Some(service_key) = service_key.as_deref() {
                    self.sync_service_effective_cache(service_key, Some(&effective));
                }
                (lb, Some(policy_key_str), Some(effective))
            }
            None => {
                if let Some(service_key) = service_key.as_deref() {
                    hc_store.set_service_config(service_key, None);
                    hc_manager.reconcile_service(service_key);
                    cb_store.set_service_config(service_key, None);
                    pt_store.set_service_threshold(service_key, None);
                    self.lb_policies.remove(service_key);
                    self.authorities.remove(service_key);
                }
                self.retry_budgets.remove(target);
                self.circuit_breakers.remove(target);
                self.logical_circuit_breakers.remove(target);
                self.connect_timeouts.remove(target);
                self.applied.remove(target);
                if target.target_kind == PolicyTargetKind::EdgionBackend {
                    // No EBTP still needs a discoverable default generation for
                    // Retry-After cooldowns. Existing captured generations remain
                    // alive through their request-owned Arcs.
                    get_ai_credential_outlier_store().publish_policy(target.clone(), None);
                }
                if let Some(service_key) = service_key.as_deref() {
                    self.sync_service_effective_cache(service_key, None);
                }
                (None, None, None)
            }
        };

        // The request path resolves its LB algorithm from this store on every
        // request (selector cells rebuild lazily via `get_or_build`), so a flip
        // needs no proactive rebuild here.
        (
            old_lb != new_lb
                || old_winner != new_winner
                || !effective_snapshot_eq(old_effective.as_deref(), new_effective.as_deref()),
            new_effective,
        )
    }

    fn publish_effective_updates(
        &self,
        current: &Arc<EffectivePolicySnapshotData>,
        updates: HashMap<PolicyTargetKey, Option<Arc<EffectiveBackendTrafficPolicy>>>,
    ) {
        // Config sync serializes ConfHandler writes for a resource kind. This
        // load-build-store publication therefore has one writer; callers must
        // not introduce an independent publisher without adding writer
        // serialization here.
        let mut next = current.policies.clone();
        let mut changed = false;
        for (target, policy) in updates {
            match policy {
                Some(policy) => {
                    let same = next.get(&target).is_some_and(|existing| Arc::ptr_eq(existing, &policy));
                    if !same {
                        next.insert(target, policy);
                        changed = true;
                    }
                }
                None => {
                    changed |= next.remove(&target).is_some();
                }
            }
        }
        if changed {
            self.effective.store(Arc::new(EffectivePolicySnapshotData {
                generation: current
                    .generation
                    .checked_add(1)
                    .expect("BackendTrafficPolicy snapshot generation exhausted"),
                policies: next,
            }));
        }
    }

    /// Recompute and publish one typed destination.
    pub fn reapply_target(&self, target: &PolicyTargetKey) -> bool {
        let current = self.effective.load_full();
        let (changed, effective) = self.resolve_target(target, current.policies.get(target));
        self.publish_effective_updates(&current, HashMap::from([(target.clone(), effective)]));
        changed
    }

    /// Service-only compatibility seam used by existing LB/health/authority
    /// consumers. It cannot observe an equally named EdgionBackend policy.
    pub fn reapply_service(&self, service_key: &str) -> bool {
        self.reapply_target(&Self::service_target(service_key))
    }

    /// Replace the whole policy cache and reapply every affected typed target.
    /// Returns the affected-key count (metadata only, for logging).
    pub fn full_set_policies(&self, data: HashMap<String, Arc<EdgionBackendTrafficPolicy>>) -> usize {
        let mut affected: HashSet<PolicyTargetKey> = HashSet::new();
        for entry in self.policies.iter() {
            affected.extend(Self::canonical_target_keys(entry.value()));
        }
        for policy in data.values() {
            affected.extend(Self::canonical_target_keys(policy));
        }

        self.policies.clear();
        for (key, policy) in data {
            self.policies.insert(key, policy);
        }

        let current = self.effective.load_full();
        let mut updates = HashMap::with_capacity(affected.len());
        for target in &affected {
            let (_, effective) = self.resolve_target(target, current.policies.get(target));
            updates.insert(target.clone(), effective);
        }
        self.publish_effective_updates(&current, updates);
        affected.len()
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
        let mut affected: HashSet<PolicyTargetKey> = HashSet::new();

        // Old target keys of the entries being updated or removed.
        for key in update.keys().chain(remove.iter()) {
            if let Some(old) = self.policies.get(key) {
                affected.extend(Self::canonical_target_keys(old.value()));
            }
        }
        // New target keys of the entries being added or updated.
        for policy in add.values().chain(update.values()) {
            affected.extend(Self::canonical_target_keys(policy));
        }

        // Apply cache mutations.
        for (key, policy) in add {
            self.policies.insert(key, policy);
        }
        for (key, policy) in update {
            self.policies.insert(key, policy);
        }
        for key in &remove {
            self.policies.remove(key);
        }

        let current = self.effective.load_full();
        let mut updates = HashMap::with_capacity(affected.len());
        for target in &affected {
            let (_, effective) = self.resolve_target(target, current.policies.get(target));
            updates.insert(target.clone(), effective);
        }
        self.publish_effective_updates(&current, updates);
        affected.len()
    }
}

/// Serializes tests that write the process-global runtime stores. This aliases
/// the cross-module guard because these tests also publish the global panic
/// threshold; a module-local lock would still race with gateway-config and
/// panic-threshold tests.
#[cfg(test)]
pub(crate) use crate::backends::panic_threshold::GLOBAL_STORE_TEST_LOCK;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::panic_threshold::{configure_lb_panic_default, resolve_panic_threshold};
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
    /// given LB type, panic threshold, health-check and outlier-detection flags.
    #[allow(clippy::too_many_arguments)]
    fn policy(
        ns: &str,
        name: &str,
        svc: &str,
        creation: Option<Time>,
        lb_type: Option<LoadBalancerType>,
        panic_threshold: Option<u8>,
        with_hc: bool,
        with_outlier: bool,
    ) -> Arc<EdgionBackendTrafficPolicy> {
        let load_balancer = lb_type.map(|t| LoadBalancerConfig {
            r#type: t,
            consistent_hash: None,
            panic_threshold,
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

    #[test]
    fn single_policy_populates_all_stores() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_panic_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6a/svc";
        store.policies.insert(
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
        );

        assert!(store.reapply_service(key));

        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));
        assert_eq!(resolve_panic_threshold(key), 40);
        assert!(get_hc_config_store().resolve(key).is_some(), "HC config present");
        assert!(
            get_outlier_detection_store().keys().contains(&key.to_string()),
            "CB config present"
        );

        // Cleanup shared global stores keyed by this service key.
        store.policies.clear();
        store.reapply_service(key);
    }

    // (b) Two policies: the older wins; deleting the older promotes the newer.
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
        store.policies.insert("bt6b/older".to_string(), older);
        store.policies.insert("bt6b/newer".to_string(), newer);

        store.reapply_service(key);
        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::Ewma), "older wins");

        store.policies.remove("bt6b/older");
        store.reapply_service(key);
        assert_eq!(
            store.get_lb_policy(key),
            Some(ParsedLBPolicy::LeastConn),
            "newer promoted after older deleted"
        );

        store.policies.clear();
        store.reapply_service(key);
    }

    // (c) Removing the last policy clears all four stores for the key.
    #[test]
    fn last_removal_clears_all_stores() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_panic_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6c/svc";
        store.policies.insert(
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
        );
        store.reapply_service(key);
        assert!(store.get_lb_policy(key).is_some());

        store.policies.remove("bt6c/p1");
        store.reapply_service(key);

        assert_eq!(store.get_lb_policy(key), None, "LB entry removed");
        assert_eq!(
            resolve_panic_threshold(key),
            0,
            "panic override cleared to global default"
        );
        assert!(get_hc_config_store().resolve(key).is_none(), "HC config cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key.to_string()),
            "CB config cleared"
        );
    }

    // (e) reapply_service reports change=true exactly when the effective config
    // moves, and change=false on a redundant reapply. This pins the store's
    // change-detection: a first apply and an LB-algorithm flip each report a
    // change, while an identical reapply is a no-op.
    #[test]
    fn reapply_reports_change_on_lb_flip() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6e/svc";
        store.policies.insert(
            "bt6e/p1".to_string(),
            policy(
                "bt6e",
                "p1",
                "svc",
                Some(ts("2026-01-01T00:00:00Z")),
                Some(LoadBalancerType::LeastConn),
                None,
                false,
                false,
            ),
        );

        // First apply installs the LB policy => change.
        assert!(store.reapply_service(key), "first apply is a change");
        assert_eq!(store.get_lb_policy(key), Some(ParsedLBPolicy::LeastConn));

        // Redundant reapply with identical cache => no change.
        assert!(!store.reapply_service(key), "redundant reapply is a no-op");

        // Flip the LB algorithm on the same (oldest) policy => change, store flips.
        store.policies.insert(
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
        );
        assert!(store.reapply_service(key), "LB flip is a change");
        assert_eq!(
            store.get_lb_policy(key),
            Some(ParsedLBPolicy::Ewma),
            "store entry flipped"
        );

        store.policies.clear();
        store.reapply_service(key);
    }

    // (d) A policy failing get_validation_errors() is skipped entirely.
    #[test]
    fn invalid_policy_is_skipped() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = BackendTrafficPolicyStore::new();
        let key = "bt6d/svc";
        // ConsistentHash without a consistentHash section => invalid.
        let invalid = Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("bad".to_string()),
                namespace: Some("bt6d".to_string()),
                creation_timestamp: Some(ts("2026-01-01T00:00:00Z")),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
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
                    panic_threshold: None,
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
        store.policies.insert("bt6d/bad".to_string(), invalid);

        store.reapply_service(key);
        assert_eq!(store.get_lb_policy(key), None, "invalid policy contributes nothing");
        assert!(get_hc_config_store().resolve(key).is_none());
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

    // (f) full_set_policies (full-resync path): a resync that drops a policy must
    // clear the stale Service key across ALL FOUR data-plane stores, while the
    // surviving policy's key is populated; a subsequent empty resync clears
    // everything. This is the full-resync stale-clear path that partial updates
    // do not exercise (affected = old cached targets ∪ new targets).
    #[test]
    fn full_set_policies_clears_stale_and_populates_new() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_panic_default(0);
        let store = BackendTrafficPolicyStore::new();
        let key_a = "bt6f/svca";
        let key_b = "bt6g/svcb";

        // Seed P1 -> svca with every section populated (LB + panic + HC + CB).
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

        // svca fully populated across all four stores.
        assert_eq!(store.get_lb_policy(key_a), Some(ParsedLBPolicy::LeastConn));
        assert_eq!(resolve_panic_threshold(key_a), 40);
        assert!(get_hc_config_store().resolve(key_a).is_some(), "A HC present");
        assert!(
            get_outlier_detection_store().keys().contains(&key_a.to_string()),
            "A CB present"
        );

        // Full resync delivering ONLY P2 -> svcb. P1/svca is dropped from the set,
        // so its four stores must be cleared; svcb becomes populated.
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

        // svca cleared across ALL FOUR stores.
        assert_eq!(store.get_lb_policy(key_a), None, "stale A LB cleared");
        assert_eq!(resolve_panic_threshold(key_a), 0, "A panic back to global default");
        assert!(get_hc_config_store().resolve(key_a).is_none(), "A HC cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key_a.to_string()),
            "A CB cleared"
        );

        // svcb now populated.
        assert_eq!(store.get_lb_policy(key_b), Some(ParsedLBPolicy::Ewma), "B LB set");
        assert_eq!(resolve_panic_threshold(key_b), 60);
        assert!(get_hc_config_store().resolve(key_b).is_some(), "B HC present");
        assert!(
            get_outlier_detection_store().keys().contains(&key_b.to_string()),
            "B CB present"
        );

        // Empty resync => everything cleared.
        store.full_set_policies(HashMap::new());
        assert_eq!(store.get_lb_policy(key_b), None, "B LB cleared on empty resync");
        assert_eq!(resolve_panic_threshold(key_b), 0, "B panic back to global default");
        assert!(get_hc_config_store().resolve(key_b).is_none(), "B HC cleared");
        assert!(
            !get_outlier_detection_store().keys().contains(&key_b.to_string()),
            "B CB cleared"
        );
    }

    // (g) upstreamAuthority resolves per Service key through the same oldest-wins
    // arbitration as the other sections, and clears when the policy goes away.
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
        store.policies.insert("bt6h/p1".to_string(), p);

        store.reapply_service(key);
        let resolved = store.get_authority(key).expect("authority resolved");
        assert_eq!(resolved.suffix, ".sandbox.internal");
        assert_eq!(resolved.compose("tenant-a"), "tenant-a.sandbox.internal");

        store.policies.clear();
        store.reapply_service(key);
        assert!(store.get_authority(key).is_none(), "cleared on removal");
    }

    // (h) retryConstraint resolves per Service key with keep-if-unchanged
    // semantics (D6): a redundant reapply with a byte-identical config must
    // NOT rebuild the budget (live counters preserved), while a real config
    // diff builds a fresh one, and removing the policy clears the entry.
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
        store.policies.insert("bt6i/p1".to_string(), p);

        store.reapply_service(key);
        let first = store.get_retry_budget(key).expect("retry budget resolved");

        // Re-run the SAME resolve (simulating a full_set/relist): the config
        // is byte-identical, so the Arc must be kept, not rebuilt.
        store.reapply_service(key);
        let second = store.get_retry_budget(key).expect("retry budget still present");
        assert!(
            Arc::ptr_eq(&first, &second),
            "unchanged config must keep the same Arc (live counters preserved)"
        );

        // Change the config (different percent) on the same policy => a new
        // Arc is built.
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
        store.policies.insert("bt6i/p1".to_string(), p2);
        store.reapply_service(key);
        let third = store.get_retry_budget(key).expect("retry budget resolved after change");
        assert!(!Arc::ptr_eq(&second, &third), "changed config must build a new Arc");

        // Removing the policy clears the budget.
        store.policies.clear();
        store.reapply_service(key);
        assert!(store.get_retry_budget(key).is_none(), "cleared on removal");
    }

    // (i) circuitBreaker resolves per Service key and clears when the policy
    // that set it is removed.
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
        store.policies.insert("bt6j/p1".to_string(), p);

        store.reapply_service(key);
        assert_eq!(store.get_circuit_breaker_limit(key), Some(5));

        // Removing the policy clears the cap.
        store.policies.clear();
        store.reapply_service(key);
        assert_eq!(store.get_circuit_breaker_limit(key), None, "cleared on removal");
    }

    // (j) connection.connectTimeout resolves per Service key and clears on removal.
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
        store.policies.insert("bt6k/p1".to_string(), p);

        store.reapply_service(key);
        assert_eq!(
            store.get_connect_timeout(key),
            Some(std::time::Duration::from_millis(250))
        );

        // Removing the policy clears the override.
        store.policies.clear();
        store.reapply_service(key);
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

        let ai = store.get_effective(&ai_target).expect("AI snapshot");
        assert!(ai.ai_credential_outlier.is_some());
        assert!(ai.retry_budget.is_some());
        assert_eq!(ai.circuit_breaker_limit, Some(7));
        assert_eq!(ai.connect_timeout, Some(std::time::Duration::from_millis(750)));
        let outlier = ai.outlier_detection.as_ref().expect("AI outlier config");
        assert_eq!(outlier.consecutive_errors, 4);
        assert_eq!(outlier.consecutive_gateway_errors, 5);
        assert_eq!(outlier.consecutive_local_origin_failures, 6);

        // AI application must not populate Service-only LB/health/authority
        // stores under its typed key or overwrite the equally named Service.
        assert_eq!(
            store.applied.get(&service_target).unwrap().value(),
            "typed/service-policy"
        );
        assert_eq!(store.applied.get(&ai_target).unwrap().value(), "typed/ai-policy");
    }

    #[test]
    fn equivalent_ai_update_keeps_retry_handle_and_field_update_is_atomic() {
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
        let default_generation = get_ai_credential_outlier_store()
            .current_handle(&target)
            .expect("default no-policy generation");
        assert!(!Arc::ptr_eq(
            fourth.ai_credential_outlier.as_ref().unwrap(),
            &default_generation
        ));
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
}
