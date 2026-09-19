//! Credential-scoped passive ejection and provider cooldowns.
//!
//! Identity is `(typed EdgionBackend key, credential alias)`. Provider DNS
//! answers and secret values are intentionally absent from keys and output.

use crate::backends::outlier_detection::{EjectionAdmission, FailureClass, FailureTransition, PassiveEjectionState};
use dashmap::DashMap;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use std::time::{Duration, SystemTime};

const DEFAULT_COOLDOWN_CAP: Duration = Duration::from_secs(5 * 60);

static GLOBAL_CREDENTIAL_OUTLIER_STORE: LazyLock<Arc<AiCredentialOutlierStore>> =
    LazyLock::new(|| Arc::new(AiCredentialOutlierStore::new()));

pub fn get_ai_credential_outlier_store() -> Arc<AiCredentialOutlierStore> {
    Arc::clone(&GLOBAL_CREDENTIAL_OUTLIER_STORE)
}

/// One immutable EBTP policy generation with a mutable, cold-path alias index.
/// Captured credential handles keep this generation alive after replacement.
pub struct AiCredentialOutlierBackendHandle {
    config: Option<OutlierDetectionResolved>,
    credentials: DashMap<String, Arc<PassiveEjectionState>>,
    aliases: parking_lot::RwLock<HashSet<String>>,
    eject_lock: Mutex<()>,
}

impl AiCredentialOutlierBackendHandle {
    fn new(config: Option<OutlierDetectionResolved>, aliases: HashSet<String>) -> Arc<Self> {
        let credentials = aliases
            .iter()
            .map(|alias| (alias.clone(), PassiveEjectionState::new(config.clone())))
            .collect();
        Arc::new(Self {
            config,
            credentials,
            aliases: parking_lot::RwLock::new(aliases),
            eject_lock: Mutex::new(()),
        })
    }

    /// Build a config-free fallback generation without mutating the previous
    /// generation. Retained aliases share their cooldown state so an
    /// equivalent credential survives a backend snapshot update, while an old
    /// snapshot keeps its original alias view.
    fn fallback_successor(
        aliases: HashSet<String>,
        previous: Option<&Arc<AiCredentialOutlierBackendHandle>>,
    ) -> Arc<Self> {
        let credentials = aliases
            .iter()
            .map(|alias| {
                let state = previous
                    .and_then(|handle| handle.credential(alias))
                    .unwrap_or_else(|| PassiveEjectionState::new(None));
                (alias.clone(), state)
            })
            .collect();
        Arc::new(Self {
            config: None,
            credentials,
            aliases: parking_lot::RwLock::new(aliases),
            eject_lock: Mutex::new(()),
        })
    }

    fn reconcile_aliases(&self, aliases: HashSet<String>) {
        self.credentials.retain(|alias, _| aliases.contains(alias));
        for alias in &aliases {
            self.credentials
                .entry(alias.clone())
                .or_insert_with(|| PassiveEjectionState::new(self.config.clone()));
        }
        *self.aliases.write() = aliases;
    }

    fn credential(&self, alias: &str) -> Option<Arc<PassiveEjectionState>> {
        if !self.aliases.read().contains(alias) {
            return None;
        }
        self.credentials.get(alias).map(|entry| Arc::clone(entry.value()))
    }

    /// Capture one credential from this exact policy generation. The returned
    /// handle remains valid if the alias or whole resource is later removed.
    pub fn capture(self: &Arc<Self>, alias: &str) -> Option<AiCredentialOutlierHandle> {
        self.credential(alias).map(|state| AiCredentialOutlierHandle {
            backend: Arc::clone(self),
            state,
        })
    }

    fn failure_allowed(&self) -> bool {
        let Some(config) = self.config.as_ref() else {
            return false;
        };
        if config.max_ejection_percent == 0 {
            return true;
        }
        let total = self.aliases.read().len().max(1);
        let open = self.credentials.iter().filter(|entry| entry.value().is_open()).count();
        let maximum = (total as u64 * u64::from(config.max_ejection_percent) / 100) as usize;
        open < maximum.max(1)
    }

    fn cooldown_cap(&self) -> Duration {
        self.config
            .as_ref()
            .map(|config| Duration::from_secs(config.max_ejection_seconds))
            .unwrap_or(DEFAULT_COOLDOWN_CAP)
            .min(DEFAULT_COOLDOWN_CAP)
            .max(Duration::from_secs(1))
    }
}

/// Request-stable credential state. Attempt code uses this handle directly and
/// never performs a global target-key lookup.
#[derive(Clone)]
pub struct AiCredentialOutlierHandle {
    backend: Arc<AiCredentialOutlierBackendHandle>,
    state: Arc<PassiveEjectionState>,
}

impl AiCredentialOutlierHandle {
    pub fn admit(&self) -> EjectionAdmission {
        self.state.admit()
    }

    pub fn record_failure(&self, class: FailureClass) -> FailureTransition {
        let Some(candidate) = self.state.record_failure_candidate(class) else {
            return FailureTransition::None;
        };
        if self
            .backend
            .config
            .as_ref()
            .is_some_and(|config| config.max_ejection_percent == 0)
        {
            return self.state.finish_failure(candidate, true);
        }
        let _guard = self.backend.eject_lock.lock();
        self.state.finish_failure(candidate, self.backend.failure_allowed())
    }

    pub fn record_success(&self) {
        self.state.record_success();
    }

    pub fn apply_retry_after(&self, value: &str, now: SystemTime) -> Option<Duration> {
        let hint = edgion_common::time::parse_retry_after(value, now)?;
        self.apply_cooldown(hint)
    }

    /// Install an already parsed provider cooldown using the same bounded
    /// policy as Retry-After. Zero is ignored and raw provider header values
    /// never enter the state or observability surfaces.
    pub fn apply_cooldown(&self, hint: Duration) -> Option<Duration> {
        if hint.is_zero() {
            return None;
        }
        let cooldown = hint.clamp(Duration::from_secs(1), self.backend.cooldown_cap());
        self.state.open_for(cooldown);
        Some(cooldown)
    }

    #[cfg(test)]
    pub(crate) fn force_expired_cooldown_for_test(&self) {
        self.state.open_for(Duration::ZERO);
    }
}

/// Runtime credential ejection store. It emits no metrics and performs no
/// tracing on request-path operations.
pub struct AiCredentialOutlierStore {
    backends: DashMap<PolicyTargetKey, AiCredentialOutlierEntry>,
}

/// The single mutable entry for one typed AI backend target. The config-free
/// backend fallback and the effective EBTP generation are separate roles so
/// their independently ordered snapshot publications cannot alias.
struct AiCredentialOutlierEntry {
    aliases: HashSet<String>,
    fallback: Option<Arc<AiCredentialOutlierBackendHandle>>,
    effective: Option<Arc<AiCredentialOutlierBackendHandle>>,
}

impl Default for AiCredentialOutlierStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AiCredentialOutlierStore {
    pub fn new() -> Self {
        Self {
            backends: DashMap::new(),
        }
    }

    /// Number of AI backends with credential outlier state.
    pub fn count(&self) -> usize {
        self.backends.len()
    }

    /// AI backend target keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.backends.iter().map(|entry| entry.key().to_string()).collect()
    }

    /// Test convenience for constructing both independently published roles.
    #[cfg(test)]
    pub fn configure_backend(
        &self,
        target: PolicyTargetKey,
        config: Option<OutlierDetectionResolved>,
        aliases: impl IntoIterator<Item = String>,
    ) -> bool {
        if target.target_kind != PolicyTargetKind::EdgionBackend {
            return false;
        }
        if self.set_credentials(target.clone(), aliases).is_none() {
            return false;
        }
        match config {
            Some(config) => {
                self.publish_effective_policy(target, Some(config));
            }
            None => self.clear_effective_policy(&target),
        }
        true
    }

    /// Publish the effective EBTP generation. Policy presence is distinct from
    /// its optional passive-ejection config, so `None` still materializes an
    /// effective generation for provider cooldowns.
    pub(crate) fn publish_effective_policy(
        &self,
        target: PolicyTargetKey,
        config: Option<OutlierDetectionResolved>,
    ) -> Option<Arc<AiCredentialOutlierBackendHandle>> {
        if target.target_kind != PolicyTargetKind::EdgionBackend {
            return None;
        }
        let handle = match self.backends.entry(target) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                if entry.effective.as_ref().is_none_or(|handle| handle.config != config) {
                    entry.effective = Some(AiCredentialOutlierBackendHandle::new(config, entry.aliases.clone()));
                }
                Arc::clone(entry.effective.as_ref().expect("effective generation"))
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                let effective = AiCredentialOutlierBackendHandle::new(config, HashSet::new());
                vacant.insert(AiCredentialOutlierEntry {
                    aliases: HashSet::new(),
                    fallback: None,
                    effective: Some(Arc::clone(&effective)),
                });
                effective
            }
        };
        Some(handle)
    }

    /// Remove only the effective-policy role. A live backend fallback remains
    /// available, while a policy-only entry is removed entirely.
    pub(crate) fn clear_effective_policy(&self, target: &PolicyTargetKey) {
        let dashmap::mapref::entry::Entry::Occupied(mut occupied) = self.backends.entry(target.clone()) else {
            return;
        };
        occupied.get_mut().effective = None;
        if occupied.get().fallback.is_none() {
            occupied.remove();
        }
    }

    /// Publish the credential role and return the exact config-free fallback
    /// generation to embed in the immutable backend snapshot.
    pub(crate) fn set_credentials(
        &self,
        target: PolicyTargetKey,
        aliases: impl IntoIterator<Item = String>,
    ) -> Option<Arc<AiCredentialOutlierBackendHandle>> {
        if target.target_kind != PolicyTargetKind::EdgionBackend {
            return None;
        }
        let aliases: HashSet<_> = aliases.into_iter().collect();
        let fallback = match self.backends.entry(target) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                if entry.aliases != aliases || entry.fallback.is_none() {
                    entry.fallback = Some(AiCredentialOutlierBackendHandle::fallback_successor(
                        aliases.clone(),
                        entry.fallback.as_ref(),
                    ));
                }
                entry.aliases = aliases.clone();
                if let Some(effective) = entry.effective.as_ref() {
                    effective.reconcile_aliases(aliases);
                }
                Arc::clone(entry.fallback.as_ref().expect("fallback generation"))
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                let fallback = AiCredentialOutlierBackendHandle::new(None, aliases.clone());
                vacant.insert(AiCredentialOutlierEntry {
                    aliases,
                    fallback: Some(Arc::clone(&fallback)),
                    effective: None,
                });
                fallback
            }
        };
        Some(fallback)
    }

    /// Remove only the backend credential role. Effective policy ownership is
    /// retained with an empty live alias set until EBTP publication clears it.
    pub(crate) fn remove_credentials(&self, target: &PolicyTargetKey) {
        let dashmap::mapref::entry::Entry::Occupied(mut occupied) = self.backends.entry(target.clone()) else {
            return;
        };
        let entry = occupied.get_mut();
        entry.aliases.clear();
        entry.fallback = None;
        if let Some(effective) = entry.effective.as_ref() {
            effective.reconcile_aliases(HashSet::new());
        }
        if entry.effective.is_none() {
            occupied.remove();
        }
    }

    #[cfg(test)]
    pub fn current_handle(&self, target: &PolicyTargetKey) -> Option<Arc<AiCredentialOutlierBackendHandle>> {
        self.backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())
    }

    #[cfg(test)]
    pub(crate) fn fallback_handle_for_test(
        &self,
        target: &PolicyTargetKey,
    ) -> Option<Arc<AiCredentialOutlierBackendHandle>> {
        self.backends.get(target).and_then(|entry| entry.fallback.clone())
    }

    #[cfg(test)]
    pub(crate) fn effective_handle_for_test(
        &self,
        target: &PolicyTargetKey,
    ) -> Option<Arc<AiCredentialOutlierBackendHandle>> {
        self.backends.get(target).and_then(|entry| entry.effective.clone())
    }

    /// Check normal admission or acquire the sole half-open probe lease.
    #[cfg(test)]
    pub fn admit(&self, target: &PolicyTargetKey, alias: &str) -> EjectionAdmission {
        let Some(backend) = self
            .backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())
        else {
            return EjectionAdmission::Rejected;
        };
        let Some(credential) = backend.capture(alias) else {
            return EjectionAdmission::Rejected;
        };
        credential.admit()
    }

    #[cfg(test)]
    pub fn record_failure(&self, target: &PolicyTargetKey, alias: &str, class: FailureClass) -> FailureTransition {
        let Some(backend) = self
            .backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())
        else {
            return FailureTransition::None;
        };
        let Some(credential) = backend.capture(alias) else {
            return FailureTransition::None;
        };
        credential.record_failure(class)
    }

    #[cfg(test)]
    pub fn record_success(&self, target: &PolicyTargetKey, alias: &str) {
        let Some(backend) = self
            .backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())
        else {
            return;
        };
        if let Some(credential) = backend.capture(alias) {
            credential.record_success();
        }
    }

    /// Parse and install a Retry-After cooldown. Valid positive hints are
    /// clamped to `1s..min(policy.maxEjectionTime or 5m, 5m)`.
    #[cfg(test)]
    pub fn apply_retry_after(
        &self,
        target: &PolicyTargetKey,
        alias: &str,
        value: &str,
        now: SystemTime,
    ) -> Option<Duration> {
        let backend = self
            .backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())?;
        backend.capture(alias)?.apply_retry_after(value, now)
    }

    #[cfg(test)]
    fn state(&self, target: &PolicyTargetKey, alias: &str) -> Arc<PassiveEjectionState> {
        self.backends
            .get(target)
            .and_then(|entry| entry.effective.as_ref().or(entry.fallback.as_ref()).cloned())
            .and_then(|backend| backend.credential(alias))
            .expect("configured credential")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str) -> PolicyTargetKey {
        PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: name.to_string(),
        }
    }

    fn config(max_seconds: u64, max_percent: u32) -> OutlierDetectionResolved {
        OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: max_seconds,
            max_ejection_percent: max_percent,
        }
    }

    #[test]
    fn count_and_keys_track_service_lifecycle() {
        let store = AiCredentialOutlierStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());

        let target = target("tracked");
        store.configure_backend(target.clone(), None, ["primary".to_string()]);
        assert_eq!(store.count(), 1);
        assert_eq!(store.keys(), vec![target.to_string()]);

        store.remove_credentials(&target);
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    #[test]
    fn retry_after_is_clamped_and_isolated_by_service_and_alias() {
        let store = AiCredentialOutlierStore::new();
        let first = target("first");
        let second = target("second");
        for backend in [&first, &second] {
            assert!(store.configure_backend(
                backend.clone(),
                Some(config(10, 100)),
                ["primary".to_string(), "secondary".to_string()],
            ));
        }
        let applied = store
            .apply_retry_after(&first, "primary", "999", SystemTime::UNIX_EPOCH)
            .expect("valid cooldown");
        assert_eq!(applied, Duration::from_secs(10));
        assert!(matches!(store.admit(&first, "primary"), EjectionAdmission::Rejected));
        assert!(matches!(store.admit(&first, "secondary"), EjectionAdmission::Closed));
        assert!(matches!(store.admit(&second, "primary"), EjectionAdmission::Closed));
    }

    #[test]
    fn parsed_reset_cooldown_uses_the_retry_after_floor_and_cap() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(target.clone(), Some(config(10, 100)), ["primary".to_string()]);
        let credential = store
            .current_handle(&target)
            .and_then(|backend| backend.capture("primary"))
            .expect("credential");
        assert_eq!(
            credential.apply_cooldown(Duration::from_millis(1)),
            Some(Duration::from_secs(1))
        );
        assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
        assert_eq!(
            credential.apply_cooldown(Duration::from_secs(999)),
            Some(Duration::from_secs(10))
        );
        assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn concurrent_success_does_not_cancel_retry_after_cooldown() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(target.clone(), Some(config(60, 100)), ["primary".to_string()]);
        let backend = store.current_handle(&target).expect("backend handle");
        let rate_limited = backend.capture("primary").expect("rate-limited request handle");
        let in_flight_success = backend.capture("primary").expect("concurrent request handle");

        assert_eq!(
            rate_limited.apply_retry_after("60", SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(60))
        );
        in_flight_success.record_success();

        assert!(matches!(rate_limited.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn zero_past_invalid_and_unknown_alias_do_not_install_cooldown() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(target.clone(), None, ["key".to_string()]);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        assert_eq!(store.apply_retry_after(&target, "key", "0", now), None);
        assert_eq!(store.apply_retry_after(&target, "key", "invalid", now), None);
        assert_eq!(store.apply_retry_after(&target, "missing", "10", now), None);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Closed));
    }

    #[test]
    fn max_ejection_percent_is_scoped_to_credentials_of_one_service() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(
            target.clone(),
            Some(config(10, 50)),
            ["one".to_string(), "two".to_string()],
        );
        assert_eq!(
            store.record_failure(&target, "one", FailureClass::Gateway),
            FailureTransition::Opened
        );
        assert_eq!(
            store.record_failure(&target, "two", FailureClass::Gateway),
            FailureTransition::None
        );
        assert!(matches!(store.admit(&target, "one"), EjectionAdmission::Rejected));
        assert!(matches!(store.admit(&target, "two"), EjectionAdmission::Closed));
    }

    #[test]
    fn credential_success_resets_failures_and_releases_ejection_capacity() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        let mut threshold_three = config(10, 50);
        threshold_three.consecutive_gateway_errors = 3;
        store.configure_backend(
            target.clone(),
            Some(threshold_three),
            ["one".to_string(), "two".to_string()],
        );

        assert_eq!(
            store.record_failure(&target, "one", FailureClass::Gateway),
            FailureTransition::None
        );
        store.record_success(&target, "one");
        for _ in 0..2 {
            assert_eq!(
                store.record_failure(&target, "one", FailureClass::Gateway),
                FailureTransition::None
            );
        }
        assert_eq!(
            store.record_failure(&target, "one", FailureClass::Gateway),
            FailureTransition::Opened
        );
        for _ in 0..2 {
            assert_eq!(
                store.record_failure(&target, "two", FailureClass::Gateway),
                FailureTransition::None
            );
        }
        assert_eq!(
            store.record_failure(&target, "two", FailureClass::Gateway),
            FailureTransition::None,
            "the first open credential consumes the 50% ejection capacity"
        );

        store.record_success(&target, "one");
        assert_eq!(
            store.record_failure(&target, "two", FailureClass::Gateway),
            FailureTransition::Opened,
            "recovery must release ejection capacity for another alias"
        );
    }

    #[test]
    fn equivalent_config_preserves_state_and_changed_config_resets() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
        store.apply_retry_after(&target, "key", "5", SystemTime::UNIX_EPOCH);
        store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
        store.configure_backend(target.clone(), Some(config(20, 100)), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Closed));
    }

    #[test]
    fn expired_cooldown_admits_one_probe_and_drop_releases_it() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_backend(target.clone(), Some(config(10, 100)), ["key".to_string()]);
        store.state(&target, "key").open_for(Duration::ZERO);
        let EjectionAdmission::Probe(probe) = store.admit(&target, "key") else {
            panic!("probe must be admitted");
        };
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
        drop(probe);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Probe(_)));
    }

    #[test]
    fn service_kind_is_part_of_identity() {
        let store = AiCredentialOutlierStore::new();
        let mut backend = target("same-name");
        backend.target_kind = PolicyTargetKind::Service;
        assert!(!store.configure_backend(backend, None, ["key".to_string()]));
    }

    #[test]
    fn independent_policy_and_credential_sync_preserve_the_other_half() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);
        store.publish_effective_policy(target.clone(), Some(config(10, 100)));
        assert_eq!(
            store.record_failure(&target, "key", FailureClass::Gateway),
            FailureTransition::Opened
        );

        // Equivalent updates from either independently-ordered conf-sync stream
        // retain live state.
        store.publish_effective_policy(target.clone(), Some(config(10, 100)));
        store.set_credentials(target.clone(), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
    }

    #[test]
    fn policy_generations_are_request_stable_across_change_and_remove() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);

        let first = store
            .publish_effective_policy(target.clone(), Some(config(10, 100)))
            .expect("AI target");
        let first_credential = first.capture("key").expect("first credential");
        first_credential.apply_retry_after("5", SystemTime::UNIX_EPOCH);

        let equivalent = store
            .publish_effective_policy(target.clone(), Some(config(10, 100)))
            .expect("AI target");
        assert!(Arc::ptr_eq(&first, &equivalent));

        let second = store
            .publish_effective_policy(target.clone(), Some(config(20, 100)))
            .expect("AI target");
        assert!(!Arc::ptr_eq(&first, &second));
        let second_credential = second.capture("key").expect("second credential");
        assert!(matches!(second_credential.admit(), EjectionAdmission::Closed));
        assert!(matches!(first_credential.admit(), EjectionAdmission::Rejected));

        store.set_credentials(target.clone(), std::iter::empty());
        assert!(store
            .current_handle(&target)
            .is_some_and(|handle| handle.capture("key").is_none()));
        // The request-owned handle does not consult the mutable alias index.
        second_credential.apply_retry_after("5", SystemTime::UNIX_EPOCH);
        assert!(matches!(second_credential.admit(), EjectionAdmission::Rejected));

        store.remove_credentials(&target);
        assert!(store.current_handle(&target).is_some());
        store.clear_effective_policy(&target);
        assert!(store.current_handle(&target).is_none());
        assert!(matches!(first_credential.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn no_policy_generation_still_supports_retry_after() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);
        let default_generation = store.fallback_handle_for_test(&target).expect("default generation");
        let credential = default_generation.capture("key").expect("credential");
        assert_eq!(
            credential.apply_retry_after("999", SystemTime::UNIX_EPOCH),
            Some(DEFAULT_COOLDOWN_CAP)
        );
        assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn clearing_policy_for_an_unknown_target_creates_nothing() {
        let store = AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "ghost".to_string(),
        };
        store.clear_effective_policy(&target);
        assert_eq!(store.count(), 0, "leaked: {:?}", store.keys());
        assert!(store.current_handle(&target).is_none());
    }

    #[test]
    fn effective_policy_without_outlier_config_is_distinct_from_fallback() {
        let store = AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "live".to_string(),
        };
        let effective = store
            .publish_effective_policy(target.clone(), None)
            .expect("effective generation");
        assert!(effective.capture("primary").is_none());
        let fallback = store
            .set_credentials(target.clone(), ["primary".to_string()])
            .expect("fallback generation");
        assert_eq!(store.count(), 1);
        assert!(!Arc::ptr_eq(&fallback, &effective));
        assert!(fallback.capture("primary").is_some());
        assert!(effective.capture("primary").is_some());
    }

    #[test]
    fn fallback_alias_changes_are_copy_on_write_and_share_retained_state() {
        let store = AiCredentialOutlierStore::new();
        let target = target("copy-on-write");
        let first = store
            .set_credentials(target.clone(), ["primary".to_string()])
            .expect("first fallback");
        let equivalent = store
            .set_credentials(target.clone(), ["primary".to_string()])
            .expect("equivalent fallback");
        assert!(Arc::ptr_eq(&first, &equivalent));
        let primary = first.capture("primary").expect("primary credential");
        primary.apply_retry_after("30", SystemTime::UNIX_EPOCH);
        let effective = store
            .publish_effective_policy(target.clone(), Some(config(10, 100)))
            .expect("effective generation");

        let second = store
            .set_credentials(target.clone(), ["primary".to_string(), "secondary".to_string()])
            .expect("second fallback");
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(first.capture("secondary").is_none());
        assert!(second.capture("secondary").is_some());
        assert!(effective.capture("secondary").is_some());
        assert!(matches!(
            second.capture("primary").expect("retained primary").admit(),
            EjectionAdmission::Rejected
        ));

        store.remove_credentials(&target);
        assert!(store.fallback_handle_for_test(&target).is_none());
        assert!(store
            .effective_handle_for_test(&target)
            .is_some_and(|handle| handle.capture("primary").is_none()));
        assert!(first.capture("primary").is_some());
        assert!(second.capture("secondary").is_some());
        store.clear_effective_policy(&target);
        assert_eq!(store.count(), 0);
    }
}
