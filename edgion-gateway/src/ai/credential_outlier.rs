//! Credential-scoped passive ejection and provider cooldowns.
//!
//! Identity is `(typed EdgionAIService key, credential alias)`. Provider DNS
//! answers and secret values are intentionally absent from keys and output.

use crate::backends::outlier_detection::{EjectionAdmission, FailureClass, FailureTransition, PassiveEjectionState};
use dashmap::DashMap;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

const DEFAULT_COOLDOWN_CAP: Duration = Duration::from_secs(5 * 60);

static GLOBAL_CREDENTIAL_OUTLIER_STORE: LazyLock<Arc<AiCredentialOutlierStore>> =
    LazyLock::new(|| Arc::new(AiCredentialOutlierStore::new()));

pub fn get_ai_credential_outlier_store() -> Arc<AiCredentialOutlierStore> {
    Arc::clone(&GLOBAL_CREDENTIAL_OUTLIER_STORE)
}

/// One immutable EBTP policy generation with a mutable, cold-path alias index.
/// Captured credential handles keep this generation alive after replacement.
pub struct AiCredentialOutlierServiceHandle {
    config: Option<OutlierDetectionResolved>,
    credentials: DashMap<String, Arc<PassiveEjectionState>>,
    aliases: parking_lot::RwLock<HashSet<String>>,
    eject_lock: Mutex<()>,
}

impl AiCredentialOutlierServiceHandle {
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
            service: Arc::clone(self),
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
    service: Arc<AiCredentialOutlierServiceHandle>,
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
            .service
            .config
            .as_ref()
            .is_some_and(|config| config.max_ejection_percent == 0)
        {
            return self.state.finish_failure(candidate, true);
        }
        let _guard = self
            .service
            .eject_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.state.finish_failure(candidate, self.service.failure_allowed())
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
        let cooldown = hint.clamp(Duration::from_secs(1), self.service.cooldown_cap());
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
    services: DashMap<PolicyTargetKey, Arc<AiCredentialOutlierServiceHandle>>,
}

impl Default for AiCredentialOutlierStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AiCredentialOutlierStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Number of AI services with credential outlier state.
    pub fn count(&self) -> usize {
        self.services.len()
    }

    /// AI service target keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.iter().map(|entry| entry.key().to_string()).collect()
    }

    /// Publish the effective outlier config and current usable credential aliases.
    /// Equivalent config updates preserve cooldown/ejection state; a real config
    /// change resets it, matching the existing Service outlier store.
    pub fn configure_service(
        &self,
        target: PolicyTargetKey,
        config: Option<OutlierDetectionResolved>,
        aliases: impl IntoIterator<Item = String>,
    ) -> bool {
        if target.target_kind != PolicyTargetKind::EdgionAIService {
            return false;
        }
        let aliases: HashSet<_> = aliases.into_iter().collect();
        match self.services.entry(target) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                if occupied.get().config == config {
                    occupied.get().reconcile_aliases(aliases);
                } else {
                    occupied.insert(AiCredentialOutlierServiceHandle::new(config, aliases));
                }
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(AiCredentialOutlierServiceHandle::new(config, aliases));
            }
        }
        true
    }

    /// Update only the policy half of the state. Credential aliases already
    /// published by EdgionAIService sync are retained.
    pub fn publish_policy(
        &self,
        target: PolicyTargetKey,
        config: Option<OutlierDetectionResolved>,
    ) -> Option<Arc<AiCredentialOutlierServiceHandle>> {
        if target.target_kind != PolicyTargetKind::EdgionAIService {
            return None;
        }
        let handle = match self.services.entry(target) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                if occupied.get().config != config {
                    let aliases = occupied.get().aliases.read().clone();
                    occupied.insert(AiCredentialOutlierServiceHandle::new(config, aliases));
                }
                Arc::clone(occupied.get())
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                // An absent policy for a target that has never published
                // credentials creates nothing usable: cooldowns and ejections
                // are recorded per credential alias, so a generation with no
                // aliases can never be captured. Inserting one here left an
                // entry per target for the process lifetime.
                config.as_ref()?;
                Arc::clone(&vacant.insert(AiCredentialOutlierServiceHandle::new(config, HashSet::new())))
            }
        };
        Some(handle)
    }

    pub fn set_policy(&self, target: PolicyTargetKey, config: Option<OutlierDetectionResolved>) -> bool {
        self.publish_policy(target, config).is_some()
    }

    /// Update only the credential half of the state. Effective EBTP config
    /// already published by policy sync is retained.
    pub fn set_credentials(&self, target: PolicyTargetKey, aliases: impl IntoIterator<Item = String>) -> bool {
        if target.target_kind != PolicyTargetKind::EdgionAIService {
            return false;
        }
        let aliases: HashSet<_> = aliases.into_iter().collect();
        match self.services.entry(target) {
            dashmap::mapref::entry::Entry::Occupied(occupied) => occupied.get().reconcile_aliases(aliases),
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(AiCredentialOutlierServiceHandle::new(None, aliases));
            }
        }
        true
    }

    pub fn remove_service(&self, target: &PolicyTargetKey) {
        self.services.remove(target);
    }

    /// Current globally discoverable policy generation. Request planning may
    /// use this only when no effective EBTP snapshot exists for the target.
    pub fn current_handle(&self, target: &PolicyTargetKey) -> Option<Arc<AiCredentialOutlierServiceHandle>> {
        self.services.get(target).map(|entry| Arc::clone(entry.value()))
    }

    /// Check normal admission or acquire the sole half-open probe lease.
    pub fn admit(&self, target: &PolicyTargetKey, alias: &str) -> EjectionAdmission {
        let Some(service) = self.services.get(target).map(|entry| Arc::clone(entry.value())) else {
            return EjectionAdmission::Rejected;
        };
        let Some(credential) = service.capture(alias) else {
            return EjectionAdmission::Rejected;
        };
        credential.admit()
    }

    pub fn record_failure(&self, target: &PolicyTargetKey, alias: &str, class: FailureClass) -> FailureTransition {
        let Some(service) = self.services.get(target).map(|entry| Arc::clone(entry.value())) else {
            return FailureTransition::None;
        };
        let Some(credential) = service.capture(alias) else {
            return FailureTransition::None;
        };
        credential.record_failure(class)
    }

    pub fn record_success(&self, target: &PolicyTargetKey, alias: &str) {
        let Some(service) = self.services.get(target).map(|entry| Arc::clone(entry.value())) else {
            return;
        };
        if let Some(credential) = service.capture(alias) {
            credential.record_success();
        }
    }

    /// Parse and install a Retry-After cooldown. Valid positive hints are
    /// clamped to `1s..min(policy.maxEjectionTime or 5m, 5m)`.
    pub fn apply_retry_after(
        &self,
        target: &PolicyTargetKey,
        alias: &str,
        value: &str,
        now: SystemTime,
    ) -> Option<Duration> {
        let service = self.services.get(target).map(|entry| Arc::clone(entry.value()))?;
        service.capture(alias)?.apply_retry_after(value, now)
    }

    #[cfg(test)]
    fn state(&self, target: &PolicyTargetKey, alias: &str) -> Arc<PassiveEjectionState> {
        self.services
            .get(target)
            .and_then(|service| service.credential(alias))
            .expect("configured credential")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str) -> PolicyTargetKey {
        PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
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
        store.configure_service(target.clone(), None, ["primary".to_string()]);
        assert_eq!(store.count(), 1);
        assert_eq!(store.keys(), vec![target.to_string()]);

        store.remove_service(&target);
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    #[test]
    fn retry_after_is_clamped_and_isolated_by_service_and_alias() {
        let store = AiCredentialOutlierStore::new();
        let first = target("first");
        let second = target("second");
        for service in [&first, &second] {
            assert!(store.configure_service(
                service.clone(),
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
        store.configure_service(target.clone(), Some(config(10, 100)), ["primary".to_string()]);
        let credential = store
            .current_handle(&target)
            .and_then(|service| service.capture("primary"))
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
        store.configure_service(target.clone(), Some(config(60, 100)), ["primary".to_string()]);
        let service = store.current_handle(&target).expect("service handle");
        let rate_limited = service.capture("primary").expect("rate-limited request handle");
        let in_flight_success = service.capture("primary").expect("concurrent request handle");

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
        store.configure_service(target.clone(), None, ["key".to_string()]);
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
        store.configure_service(
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
        store.configure_service(
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
        store.configure_service(target.clone(), Some(config(10, 100)), ["key".to_string()]);
        store.apply_retry_after(&target, "key", "5", SystemTime::UNIX_EPOCH);
        store.configure_service(target.clone(), Some(config(10, 100)), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
        store.configure_service(target.clone(), Some(config(20, 100)), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Closed));
    }

    #[test]
    fn expired_cooldown_admits_one_probe_and_drop_releases_it() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.configure_service(target.clone(), Some(config(10, 100)), ["key".to_string()]);
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
        let mut service = target("same-name");
        service.target_kind = PolicyTargetKind::Service;
        assert!(!store.configure_service(service, None, ["key".to_string()]));
    }

    #[test]
    fn independent_policy_and_credential_sync_preserve_the_other_half() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);
        store.set_policy(target.clone(), Some(config(10, 100)));
        assert_eq!(
            store.record_failure(&target, "key", FailureClass::Gateway),
            FailureTransition::Opened
        );

        // Equivalent updates from either independently-ordered conf-sync stream
        // retain live state.
        store.set_policy(target.clone(), Some(config(10, 100)));
        store.set_credentials(target.clone(), ["key".to_string()]);
        assert!(matches!(store.admit(&target, "key"), EjectionAdmission::Rejected));
    }

    #[test]
    fn policy_generations_are_request_stable_across_change_and_remove() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);

        let first = store
            .publish_policy(target.clone(), Some(config(10, 100)))
            .expect("AI target");
        let first_credential = first.capture("key").expect("first credential");
        first_credential.apply_retry_after("5", SystemTime::UNIX_EPOCH);

        let equivalent = store
            .publish_policy(target.clone(), Some(config(10, 100)))
            .expect("AI target");
        assert!(Arc::ptr_eq(&first, &equivalent));

        let second = store
            .publish_policy(target.clone(), Some(config(20, 100)))
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

        store.remove_service(&target);
        assert!(store.current_handle(&target).is_none());
        assert!(matches!(first_credential.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn no_policy_generation_still_supports_retry_after() {
        let store = AiCredentialOutlierStore::new();
        let target = target("svc");
        store.set_credentials(target.clone(), ["key".to_string()]);
        let default_generation = store.current_handle(&target).expect("default generation");
        let credential = default_generation.capture("key").expect("credential");
        assert_eq!(
            credential.apply_retry_after("999", SystemTime::UNIX_EPOCH),
            Some(DEFAULT_COOLDOWN_CAP)
        );
        assert!(matches!(credential.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn absent_policy_for_an_unknown_target_creates_nothing() {
        // Publishing "no policy" for a target that never had credentials used
        // to insert an entry that no request could ever capture from, so every
        // such target leaked one for the process lifetime.
        let store = AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "default".to_string(),
            name: "ghost".to_string(),
        };
        assert!(store.publish_policy(target.clone(), None).is_none());
        assert_eq!(store.count(), 0, "leaked: {:?}", store.keys());
        assert!(store.current_handle(&target).is_none());
    }

    #[test]
    fn absent_policy_still_keeps_a_live_credential_generation() {
        // The documented reason the no-policy generation exists: a service with
        // credentials but no EBTP still needs somewhere to record Retry-After
        // cooldowns. That must survive.
        let store = AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "default".to_string(),
            name: "live".to_string(),
        };
        store.set_credentials(target.clone(), ["primary".to_string()]);
        assert!(store.publish_policy(target.clone(), None).is_some());
        assert_eq!(store.count(), 1);
        let handle = store.current_handle(&target).expect("default generation");
        assert!(handle.capture("primary").is_some());
    }
}
