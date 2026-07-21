//! BackendTLSPolicy Target Reference Manager
//!
//! Tracks which BackendTLSPolicy resources target which Services.
//! Used for conflict detection: when multiple policies target the same
//! Service (with the same sectionName), only the oldest is accepted;
//! the rest are marked Conflicted per Gateway API spec.
//!
//! Uses the same `BidirectionalRefManager` pattern as SecretRefManager
//! and ServiceRefManager.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;

use super::ref_manager::{BidirectionalRefManager, RefValue, ResourceRef};
use super::secret_utils::SecretRefManager;

type BackendTLSPolicyTargetManager = SecretRefManager;

/// Aggregated state for BackendTLSPolicy conflict detection.
///
/// Combines the target-reference map (tracks which policies target which
/// Services) and creation timestamps (used for precedence sorting) into a
/// single struct so they can be owned as a field of `ControllerRuntime`
/// instead of living in module-level globals.
pub struct BackendTLSPolicyState {
    target_mgr: BackendTLSPolicyTargetManager,
    // Creation timestamps kept as `DateTime<Utc>` (not a formatted RFC3339 String):
    // precedence sorting in `is_winner` compares instants directly, which is correct
    // by construction and avoids the fragility of lexicographically comparing strings
    // whose RFC3339 shape (`Z` vs `+00:00`, sub-second digits) would have to match.
    timestamps: RwLock<HashMap<String, DateTime<Utc>>>,
}

impl BackendTLSPolicyState {
    pub fn new() -> Self {
        Self {
            target_mgr: BidirectionalRefManager::with_component("backend_tls_policy_target"),
            timestamps: RwLock::new(HashMap::new()),
        }
    }

    /// Access the underlying target reference manager.
    pub fn target_mgr(&self) -> &BackendTLSPolicyTargetManager {
        &self.target_mgr
    }

    /// Store the creation timestamp for a policy (used for conflict precedence).
    ///
    /// Passing `None` clears any previously-stored timestamp for the key — this
    /// keeps the stored map in sync with the policy's actual metadata (e.g. on
    /// recreate without a creation_timestamp the old value must not linger,
    /// otherwise `is_winner` would compare a stale Some against fresh peers).
    pub fn set_timestamp(&self, policy_key: &str, timestamp: Option<DateTime<Utc>>) {
        let mut map = self.timestamps.write();
        match timestamp {
            Some(ts) => {
                map.insert(policy_key.to_string(), ts);
            }
            None => {
                map.remove(policy_key);
            }
        }
    }

    /// Remove the stored timestamp when a policy is deleted.
    pub fn remove_timestamp(&self, policy_key: &str) {
        self.timestamps.write().remove(policy_key);
    }

    /// Get the stored creation timestamp for a policy.
    fn get_timestamp(&self, policy_key: &str) -> Option<DateTime<Utc>> {
        self.timestamps.read().get(policy_key).copied()
    }

    /// Public read of the stored creation timestamp for a policy.
    ///
    /// Used by `parse_policy` to detect delete+recreate within a single
    /// PollWatcher tick: same name, same target set, but a new
    /// `creation_timestamp` shifts precedence ordering so siblings must be
    /// re-evaluated.
    pub fn timestamp_of(&self, policy_key: &str) -> Option<DateTime<Utc>> {
        self.get_timestamp(policy_key)
    }

    /// Determine if a policy is the winner among all policies targeting the same target.
    ///
    /// Gateway API precedence:
    /// 1. Oldest `creationTimestamp` wins
    /// 2. Tie-break by `{namespace}/{name}` lexicographic order
    ///
    /// Returns `true` if the policy should be `Accepted`, `false` if `Conflicted`.
    pub fn is_winner(&self, policy_key: &str, target_key: &str) -> bool {
        let refs = self.target_mgr.get_refs(target_key);
        if refs.len() <= 1 {
            return true;
        }

        // Snapshot timestamps once so the refs list and the timestamp lookups
        // come from a single consistent view of the map. Without this, the
        // per-entry `get_timestamp` calls would each re-acquire the read lock
        // and could observe partial writes interleaved with `get_refs`.
        let ts_snapshot = self.timestamps.read().clone();

        let mut entries: Vec<(String, Option<DateTime<Utc>>)> = refs
            .iter()
            .map(|r| {
                let key = r.resource_key();
                let ts = ts_snapshot.get(&key).copied();
                (key, ts)
            })
            .collect();

        entries.sort_by(|a, b| match (&a.1, &b.1) {
            (Some(ts_a), Some(ts_b)) => ts_a.cmp(ts_b).then_with(|| a.0.cmp(&b.0)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        });

        entries.first().map(|(key, _)| key == policy_key).unwrap_or(true)
    }

    /// Get all sibling policies that share any target with the given policy.
    ///
    /// Returns `ResourceRef`s of sibling policies (excluding self).
    pub fn get_siblings(&self, policy_ref: &ResourceRef) -> Vec<ResourceRef> {
        let policy_key = policy_ref.ref_key();
        let target_keys = self.target_mgr.get_dependencies(&policy_key);

        let mut siblings = Vec::new();
        for target_key in target_keys {
            for r in self.target_mgr.get_refs(&target_key) {
                if r != *policy_ref && !siblings.contains(&r) {
                    siblings.push(r);
                }
            }
        }
        siblings
    }

    /// Reset all state. Called by `ControllerRuntime::reset()` on leader-change
    /// or controller restart to prevent stale entries from persisting across
    /// watch-stream reconnects.
    pub fn reset(&self) {
        self.target_mgr.clear();
        self.timestamps.write().clear();
        tracing::info!(
            component = "backend_tls_policy_ref",
            "Cleared BackendTLSPolicy target manager and timestamps"
        );
    }
}

impl Default for BackendTLSPolicyState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a conflict key from target ref fields.
///
/// Policies sharing the same conflict key are in conflict with each other.
/// Different `section_name` values produce different keys (no conflict).
///
/// Format: `"namespace/name"` or `"namespace/name::sectionName"`
pub fn make_target_conflict_key(namespace: &str, name: &str, section_name: Option<&str>) -> String {
    match section_name {
        Some(sn) => format!("{}/{}::{}", namespace, name, sn),
        None => format!("{}/{}", namespace, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::ResourceKind;

    /// Parse an RFC3339 literal into the `DateTime<Utc>` the store now holds.
    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid RFC3339")
    }

    fn make_policy_ref(ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(ResourceKind::BackendTLSPolicy, Some(ns.to_string()), name.to_string())
    }

    fn make_state() -> BackendTLSPolicyState {
        BackendTLSPolicyState::new()
    }

    /// Set up two conflicting policies for testing on a given state instance.
    fn setup_two_policies(state: &BackendTLSPolicyState, ns: &str) -> (ResourceRef, ResourceRef) {
        let policy_a = make_policy_ref(ns, "policy-a");
        let policy_b = make_policy_ref(ns, "policy-b");
        let target_key = make_target_conflict_key(ns, "my-service", None);

        state.target_mgr().add_ref(target_key.clone(), policy_a.clone());
        state.target_mgr().add_ref(target_key, policy_b.clone());

        state.set_timestamp(&format!("{}/policy-a", ns), Some(ts("2024-01-01T00:00:00Z")));
        state.set_timestamp(&format!("{}/policy-b", ns), Some(ts("2024-01-02T00:00:00Z")));

        (policy_a, policy_b)
    }

    #[test]
    fn test_make_target_conflict_key_no_section() {
        let key = make_target_conflict_key("ns-key1", "my-service", None);
        assert_eq!(key, "ns-key1/my-service");
    }

    #[test]
    fn test_make_target_conflict_key_with_section() {
        let key = make_target_conflict_key("ns-key2", "my-service", Some("port-80"));
        assert_eq!(key, "ns-key2/my-service::port-80");
    }

    #[test]
    fn test_no_conflict_single_policy() {
        let ns = "ns-single";
        let state = make_state();

        let policy = make_policy_ref(ns, "only-policy");
        let target_key = make_target_conflict_key(ns, "svc", None);
        state.target_mgr().add_ref(target_key.clone(), policy);

        assert!(state.is_winner(&format!("{}/only-policy", ns), &target_key));
    }

    #[test]
    fn test_older_policy_wins() {
        let ns = "ns-older";
        let state = make_state();
        let (_policy_a, _policy_b) = setup_two_policies(&state, ns);
        let target_key = make_target_conflict_key(ns, "my-service", None);

        assert!(state.is_winner(&format!("{}/policy-a", ns), &target_key));
        assert!(!state.is_winner(&format!("{}/policy-b", ns), &target_key));
    }

    #[test]
    fn test_same_timestamp_name_tiebreak() {
        let ns = "ns-tiebreak";
        let state = make_state();

        let policy_a = make_policy_ref(ns, "alpha");
        let policy_b = make_policy_ref(ns, "beta");
        let target_key = make_target_conflict_key(ns, "svc", None);

        state.target_mgr().add_ref(target_key.clone(), policy_a.clone());
        state.target_mgr().add_ref(target_key.clone(), policy_b.clone());

        // Same timestamp — alphabetically earlier name wins
        state.set_timestamp(&format!("{}/alpha", ns), Some(ts("2024-01-01T00:00:00Z")));
        state.set_timestamp(&format!("{}/beta", ns), Some(ts("2024-01-01T00:00:00Z")));

        assert!(state.is_winner(&format!("{}/alpha", ns), &target_key));
        assert!(!state.is_winner(&format!("{}/beta", ns), &target_key));
    }

    #[test]
    fn test_different_section_names_no_conflict() {
        let ns = "ns-sections";
        let state = make_state();

        let policy_a = make_policy_ref(ns, "policy-a");
        let policy_b = make_policy_ref(ns, "policy-b");

        let key_80 = make_target_conflict_key(ns, "svc", Some("port-80"));
        let key_443 = make_target_conflict_key(ns, "svc", Some("port-443"));

        state.target_mgr().add_ref(key_80.clone(), policy_a);
        state.target_mgr().add_ref(key_443.clone(), policy_b);

        // Each target has only one policy — no conflict
        assert!(state.is_winner(&format!("{}/policy-a", ns), &key_80));
        assert!(state.is_winner(&format!("{}/policy-b", ns), &key_443));
    }

    #[test]
    fn test_get_sibling_policies() {
        let ns = "ns-siblings";
        let state = make_state();
        let (policy_a, policy_b) = setup_two_policies(&state, ns);

        let siblings_a = state.get_siblings(&policy_a);
        assert_eq!(siblings_a.len(), 1);
        assert_eq!(siblings_a[0], policy_b);

        let siblings_b = state.get_siblings(&policy_b);
        assert_eq!(siblings_b.len(), 1);
        assert_eq!(siblings_b[0], policy_a);
    }

    #[test]
    fn test_no_siblings_single_policy() {
        let ns = "ns-lonely";
        let state = make_state();

        let policy = make_policy_ref(ns, "lonely");
        let target_key = make_target_conflict_key(ns, "svc", None);
        state.target_mgr().add_ref(target_key, policy.clone());

        assert!(state.get_siblings(&policy).is_empty());
    }
}
