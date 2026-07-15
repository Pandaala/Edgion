//! Runtime store + conflict arbitration for `EdgionBackendTrafficPolicy`.
//!
//! Holds the synced policy cache plus the resolved per-service load-balancer and
//! upstream-authority maps. On every change it recomputes the winning policy per
//! affected Service key (oldest-wins via `precedence_key`) and pushes the effective
//! config into the four data-plane stores: active health check, circuit breaker
//! (outlier detection), panic threshold, and the load-balancer policy map read on
//! the request path by `resolve_lb_policy` — plus the upstream-authority map read
//! on the request path by both data paths via `get_authority`.

use crate::backends::circuit_breaker::get_circuit_breaker_store;
use crate::backends::health::check::{get_hc_config_store, get_health_check_manager};
use crate::backends::panic_threshold::get_panic_threshold_store;
use dashmap::DashMap;
use edgion_resources::resources::edgion_backend_traffic_policy::precedence_key;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::resources::upstream_authority::CompiledUpstreamAuthority;
use edgion_resources::resources::EdgionBackendTrafficPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

static BACKEND_TRAFFIC_POLICY_STORE: LazyLock<Arc<BackendTrafficPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTrafficPolicyStore::new()));

/// Process-global singleton store.
pub fn get_backend_traffic_policy_store() -> Arc<BackendTrafficPolicyStore> {
    BACKEND_TRAFFIC_POLICY_STORE.clone()
}

/// Store owning the synced policy cache, the resolved winner per Service key, and
/// the request-path load-balancer policy and upstream-authority maps.
pub struct BackendTrafficPolicyStore {
    /// Synced policies keyed by resource key (`namespace/name`).
    policies: DashMap<String, Arc<EdgionBackendTrafficPolicy>>,
    /// Winning policy key per Service key (`namespace/service`).
    applied: DashMap<String, String>,
    /// Resolved load-balancer policy per Service key (read on the request path).
    lb_policies: DashMap<String, ParsedLBPolicy>,
    /// Resolved upstream authority per Service key (read on the request path).
    authorities: DashMap<String, Arc<CompiledUpstreamAuthority>>,
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
            lb_policies: DashMap::new(),
            authorities: DashMap::new(),
        }
    }

    /// Service keys (`namespace/service`) a policy attaches to. Same-namespace
    /// attachment only: the Service namespace is the policy namespace.
    fn policy_service_keys(policy: &EdgionBackendTrafficPolicy) -> Vec<String> {
        let ns = policy.metadata.namespace.clone().unwrap_or_default();
        policy
            .spec
            .target_refs
            .iter()
            .map(|tr| format!("{ns}/{}", tr.name))
            .collect()
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

    /// Admin snapshot of the resolved load-balancer map: Service key -> debug
    /// string of the applied `ParsedLBPolicy`. Not on the request path.
    pub fn lb_policy_snapshot(&self) -> std::collections::BTreeMap<String, String> {
        self.lb_policies
            .iter()
            .map(|e| (e.key().clone(), format!("{:?}", e.value())))
            .collect()
    }

    /// Recompute and apply the winning policy for one Service key.
    /// Returns true when the effective config changed.
    pub fn reapply_service(&self, service_key: &str) -> bool {
        // Winner = min over valid cached policies targeting this key, ordered by
        // precedence_key (LOWER wins). Invalid policies are skipped entirely.
        let mut winner: Option<((i64, String), String, Arc<EdgionBackendTrafficPolicy>)> = None;
        for entry in self.policies.iter() {
            let policy = entry.value();
            if !policy.spec.get_validation_errors().is_empty() {
                continue;
            }
            if !Self::policy_service_keys(policy).iter().any(|k| k == service_key) {
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

        let old_lb = self.get_lb_policy(service_key);
        let old_winner = self.applied.get(service_key).map(|v| v.value().clone());

        let hc_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let cb_store = get_circuit_breaker_store();
        let pt_store = get_panic_threshold_store();

        let (new_lb, new_winner) = match winner {
            Some((_, policy_key_str, policy)) => {
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

                let cb = policy.spec.outlier_detection.clone().map(|od| od.normalize());
                cb_store.set_service_config(service_key, cb);

                let pt = policy.spec.load_balancer.as_ref().and_then(|lb| lb.panic_threshold);
                pt_store.set_service_threshold(service_key, pt);

                let lb = policy.spec.load_balancer.as_ref().and_then(|lb| lb.to_parsed());
                match &lb {
                    Some(v) => {
                        self.lb_policies.insert(service_key.to_string(), v.clone());
                    }
                    None => {
                        self.lb_policies.remove(service_key);
                    }
                }
                self.applied.insert(service_key.to_string(), policy_key_str.clone());
                (lb, Some(policy_key_str))
            }
            None => {
                hc_store.set_service_config(service_key, None);
                hc_manager.reconcile_service(service_key);
                cb_store.set_service_config(service_key, None);
                pt_store.set_service_threshold(service_key, None);
                self.lb_policies.remove(service_key);
                self.authorities.remove(service_key);
                self.applied.remove(service_key);
                (None, None)
            }
        };

        // The request path resolves its LB algorithm from this store on every
        // request (selector cells rebuild lazily via `get_or_build`), so a flip
        // needs no proactive rebuild here.
        old_lb != new_lb || old_winner != new_winner
    }

    /// Replace the whole policy cache and reapply every affected Service key.
    /// Returns the affected-key count (metadata only, for logging).
    pub fn full_set_policies(&self, data: HashMap<String, Arc<EdgionBackendTrafficPolicy>>) -> usize {
        let mut affected: HashSet<String> = HashSet::new();
        for entry in self.policies.iter() {
            affected.extend(Self::policy_service_keys(entry.value()));
        }
        for policy in data.values() {
            affected.extend(Self::policy_service_keys(policy));
        }

        self.policies.clear();
        for (key, policy) in data {
            self.policies.insert(key, policy);
        }

        for service_key in &affected {
            self.reapply_service(service_key);
        }
        affected.len()
    }

    /// Incremental cache diff: recompute only the Service keys touched by the
    /// changed policies (old target keys of update/remove ∪ new target keys of
    /// add/update). Returns the affected-key count (metadata only).
    pub fn partial_update_policies(
        &self,
        add: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        update: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        remove: HashSet<String>,
    ) -> usize {
        let mut affected: HashSet<String> = HashSet::new();

        // Old target keys of the entries being updated or removed.
        for key in update.keys().chain(remove.iter()) {
            if let Some(old) = self.policies.get(key) {
                affected.extend(Self::policy_service_keys(old.value()));
            }
        }
        // New target keys of the entries being added or updated.
        for policy in add.values().chain(update.values()) {
            affected.extend(Self::policy_service_keys(policy));
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

        for service_key in &affected {
            self.reapply_service(service_key);
        }
        affected.len()
    }
}

/// Serializes tests that write the process-global runtime stores. The HC config
/// store swaps a whole `ArcSwap` map (load-clone-modify-store), which is not
/// atomic across concurrent writers, so even tests using distinct Service keys
/// would clobber each other's map without this guard.
#[cfg(test)]
pub(crate) static GLOBAL_STORE_TEST_LOCK: LazyLock<parking_lot::Mutex<()>> =
    LazyLock::new(|| parking_lot::Mutex::new(()));

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
            ejection_seconds: 30,
            max_ejection_seconds: None,
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
                load_balancer,
                health_check,
                outlier_detection,
                upstream_authority: None,
            },
            status: None,
        })
    }

    // (a) A single valid policy populates all four data-plane stores and the
    // resolved LB map.
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
            get_circuit_breaker_store().keys().contains(&key.to_string()),
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
            !get_circuit_breaker_store().keys().contains(&key.to_string()),
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
                load_balancer: Some(LoadBalancerConfig {
                    r#type: LoadBalancerType::ConsistentHash,
                    consistent_hash: None,
                    panic_threshold: None,
                }),
                health_check: None,
                outlier_detection: None,
                upstream_authority: None,
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
            get_circuit_breaker_store().keys().contains(&key_a.to_string()),
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
            !get_circuit_breaker_store().keys().contains(&key_a.to_string()),
            "A CB cleared"
        );

        // svcb now populated.
        assert_eq!(store.get_lb_policy(key_b), Some(ParsedLBPolicy::Ewma), "B LB set");
        assert_eq!(resolve_panic_threshold(key_b), 60);
        assert!(get_hc_config_store().resolve(key_b).is_some(), "B HC present");
        assert!(
            get_circuit_breaker_store().keys().contains(&key_b.to_string()),
            "B CB present"
        );

        // Empty resync => everything cleared.
        store.full_set_policies(HashMap::new());
        assert_eq!(store.get_lb_policy(key_b), None, "B LB cleared on empty resync");
        assert_eq!(resolve_panic_threshold(key_b), 0, "B panic back to global default");
        assert!(get_hc_config_store().resolve(key_b).is_none(), "B HC cleared");
        assert!(
            !get_circuit_breaker_store().keys().contains(&key_b.to_string()),
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
}
