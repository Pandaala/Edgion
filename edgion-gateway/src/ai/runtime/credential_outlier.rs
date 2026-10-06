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
#[path = "../../../tests/unit/ai/runtime/credential_outlier_test.rs"]
mod tests;
