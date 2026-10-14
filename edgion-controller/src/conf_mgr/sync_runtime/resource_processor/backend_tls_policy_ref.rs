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
use edgion_resources::resources::common::policy_precedence::policy_precedence_key;
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey;
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

        // Project only this target's timestamps under one read guard. These
        // values are coherent with each other, but not an atomic snapshot with
        // the separate reference lookup above. Release the guard before sorting.
        let mut entries: Vec<(String, Option<DateTime<Utc>>)> = {
            let timestamps = self.timestamps.read();
            refs.iter()
                .map(|r| {
                    let key = r.resource_key();
                    let ts = timestamps.get(&key).copied();
                    (key, ts)
                })
                .collect()
        };

        entries.sort_by(|a, b| policy_precedence_key(a.1, &a.0).cmp(&policy_precedence_key(b.1, &b.0)));

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
/// Format: `"group/kind/namespace/name"` or
/// `"group/kind/namespace/name::sectionName"`.
pub fn make_target_conflict_key(target: &PolicyTargetKey, section_name: Option<&str>) -> String {
    match section_name {
        Some(sn) => format!("{target}::{sn}"),
        None => target.to_string(),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/backend_tls_policy_ref_test.rs"]
mod tests;
