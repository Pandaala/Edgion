//! In-memory desired state for Controller-managed credential sources.
//!
//! Handlers rebuild this registry synchronously from cached resources. It is
//! intentionally free of credential bytes: the asynchronous credential
//! manager re-reads authorized Secrets from the current stores before use.

use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use edgion_resources::resources::link_sys::{CredentialSourceConfig, CredentialSourceState};
use parking_lot::RwLock;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ResourceRef;

pub const MANAGED_BY_LABEL: &str = "edgion.io/managed-by";
pub const MANAGED_BY_VALUE: &str = "oauth2-credential";
pub const CONSUMER_UID_ANNOTATION: &str = "edgion.io/credential-consumer-uid";
pub const SOURCE_UID_ANNOTATION: &str = "edgion.io/credential-source-uid";
pub const SOURCE_GENERATION_ANNOTATION: &str = "edgion.io/credential-source-generation";
pub const VALID_UNTIL_ANNOTATION: &str = "edgion.io/credential-valid-until";
pub const BINDING_REVISION_ANNOTATION: &str = "edgion.io/credential-binding-revision";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSecretInput {
    pub role: &'static str,
    pub namespace: String,
    pub name: String,
    pub uid: Option<String>,
    pub resource_version: Option<String>,
    pub authorization_revision: String,
}

/// Non-secret desired snapshot produced by a `LinkSys` reconcile.
#[derive(Clone)]
pub struct DesiredCredentialSource {
    pub resource: ResourceRef,
    pub uid: String,
    pub generation: i64,
    pub config: ControllerCredentialSourceConfig,
    pub secret_inputs: Vec<CredentialSecretInput>,
}

/// Controller-only resolved transport snapshot. Deliberately implements
/// neither serialization nor ordinary Debug so resolved TLS material cannot
/// accidentally enter conf_sync or diagnostics.
#[derive(Clone)]
pub struct ControllerCredentialSourceConfig(CredentialSourceConfig);

impl ControllerCredentialSourceConfig {
    pub fn new(config: CredentialSourceConfig) -> Self {
        Self(config)
    }
}

impl Deref for ControllerCredentialSourceConfig {
    type Target = CredentialSourceConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DesiredCredentialSource {
    pub fn logical_key(&self) -> String {
        self.resource.resource_key()
    }

    pub fn identity_key(&self) -> String {
        format!("{}/{}", self.logical_key(), self.uid)
    }
}

/// Desired publication binding produced by an `EdgionPlugins` reconcile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesiredCredentialConsumer {
    pub resource: ResourceRef,
    pub uid: String,
    pub generation: i64,
    pub source_key: String,
    pub source_uid: String,
    pub source_generation: i64,
    pub target_secret_namespace: String,
    pub target_secret_name: String,
    pub authorization_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialManagerResult {
    pub observed_generation: i64,
    pub state: CredentialSourceState,
    pub published_generation: Option<String>,
    pub issued_at: Option<String>,
    pub valid_until: Option<String>,
    pub last_successful_refresh_time: Option<String>,
    pub next_refresh_time: Option<String>,
    pub desired_consumer_count: u32,
    pub published_consumer_count: u32,
    pub conflicted_consumer_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CredentialSourceRegistryStats {
    pub source_count: usize,
    pub consumer_count: usize,
    pub result_count: usize,
    pub reconcile_pending_count: usize,
    pub reconcile_epoch_count: usize,
    pub reconcile_observation_count: usize,
}

impl DesiredCredentialConsumer {
    pub fn logical_key(&self) -> String {
        self.resource.resource_key()
    }

    pub fn identity_key(&self) -> String {
        format!("{}/{}", self.logical_key(), self.uid)
    }

    pub fn binding_revision(&self) -> String {
        let mut digest = Sha256::new();
        for part in [
            self.uid.as_str(),
            &self.generation.to_string(),
            self.source_uid.as_str(),
            &self.source_generation.to_string(),
            self.target_secret_namespace.as_str(),
            self.target_secret_name.as_str(),
            self.authorization_revision.as_str(),
        ] {
            digest.update(part.as_bytes());
            digest.update([0]);
        }
        hex::encode(digest.finalize())
    }
}

#[derive(Default)]
struct RegistryState {
    sources: HashMap<String, DesiredCredentialSource>,
    consumers: HashMap<String, DesiredCredentialConsumer>,
    reconcile_pending: HashSet<String>,
    reconcile_observations: HashMap<String, String>,
    reconcile_epochs: HashMap<String, u64>,
    next_reconcile_epoch: u64,
    results: HashMap<String, CredentialManagerResult>,
}

/// Typed, process-local desired state shared by handlers and the future
/// leader-only credential manager.
#[derive(Default)]
pub struct CredentialSourceRegistry {
    inner: RwLock<RegistryState>,
}

impl CredentialSourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_source(&self, source: DesiredCredentialSource) {
        self.inner.write().sources.insert(source.logical_key(), source);
    }

    pub fn remove_source(&self, logical_key: &str) -> Option<DesiredCredentialSource> {
        self.inner.write().sources.remove(logical_key)
    }

    pub fn forget_source(&self, logical_key: &str) -> Option<DesiredCredentialSource> {
        let mut state = self.inner.write();
        state.results.remove(logical_key);
        state.sources.remove(logical_key)
    }

    pub fn source(&self, logical_key: &str) -> Option<DesiredCredentialSource> {
        self.inner.read().sources.get(logical_key).cloned()
    }

    pub fn sources(&self) -> Vec<DesiredCredentialSource> {
        self.inner.read().sources.values().cloned().collect()
    }

    pub fn replace_consumer(&self, consumer: DesiredCredentialConsumer) {
        self.inner.write().consumers.insert(consumer.logical_key(), consumer);
    }

    pub fn remove_consumer(&self, logical_key: &str) -> Option<DesiredCredentialConsumer> {
        self.inner.write().consumers.remove(logical_key)
    }

    pub fn forget_consumer(&self, logical_key: &str) -> Option<DesiredCredentialConsumer> {
        let mut state = self.inner.write();
        state.reconcile_pending.remove(logical_key);
        state.reconcile_epochs.remove(logical_key);
        state.reconcile_observations.remove(logical_key);
        state.consumers.remove(logical_key)
    }

    pub fn consumer(&self, logical_key: &str) -> Option<DesiredCredentialConsumer> {
        self.inner.read().consumers.get(logical_key).cloned()
    }

    pub fn consumers_for_source(&self, source_key: &str) -> Vec<DesiredCredentialConsumer> {
        self.inner
            .read()
            .consumers
            .values()
            .filter(|consumer| consumer.source_key == source_key)
            .cloned()
            .collect()
    }

    /// Signal that the desired publication is absent or invalid. Repeated
    /// handler retries are coalesced until a healthy Secret is observed.
    pub fn request_consumer_reconcile(&self, logical_key: &str, observed_revision: String) {
        let mut state = self.inner.write();
        let changed = state.reconcile_observations.get(logical_key) != Some(&observed_revision);
        state.reconcile_pending.insert(logical_key.to_string());
        if changed {
            state.next_reconcile_epoch = state.next_reconcile_epoch.wrapping_add(1).max(1);
            let epoch = state.next_reconcile_epoch;
            state.reconcile_epochs.insert(logical_key.to_string(), epoch);
            state
                .reconcile_observations
                .insert(logical_key.to_string(), observed_revision);
        }
    }

    pub fn clear_consumer_reconcile(&self, logical_key: &str) {
        let mut state = self.inner.write();
        state.reconcile_pending.remove(logical_key);
        state.reconcile_observations.remove(logical_key);
    }

    pub fn reconcile_epoch_for_source(&self, source_key: &str) -> u64 {
        let state = self.inner.read();
        state
            .consumers
            .iter()
            .filter(|(_, consumer)| consumer.source_key == source_key)
            .filter_map(|(key, _)| state.reconcile_epochs.get(key))
            .copied()
            .max()
            .unwrap_or(0)
    }

    pub fn replace_result(&self, logical_key: String, result: CredentialManagerResult) -> bool {
        let mut state = self.inner.write();
        if state.results.get(&logical_key) == Some(&result) {
            return false;
        }
        state.results.insert(logical_key, result);
        true
    }

    pub fn result(&self, logical_key: &str) -> Option<CredentialManagerResult> {
        self.inner.read().results.get(logical_key).cloned()
    }

    pub fn remove_result(&self, logical_key: &str) -> Option<CredentialManagerResult> {
        self.inner.write().results.remove(logical_key)
    }

    pub fn stats(&self) -> CredentialSourceRegistryStats {
        let state = self.inner.read();
        CredentialSourceRegistryStats {
            source_count: state.sources.len(),
            consumer_count: state.consumers.len(),
            result_count: state.results.len(),
            reconcile_pending_count: state.reconcile_pending.len(),
            reconcile_epoch_count: state.reconcile_epochs.len(),
            reconcile_observation_count: state.reconcile_observations.len(),
        }
    }

    pub fn keys(&self) -> Vec<String> {
        let state = self.inner.read();
        let mut keys = state
            .sources
            .keys()
            .chain(state.consumers.keys())
            .chain(state.results.keys())
            .chain(state.reconcile_pending.iter())
            .chain(state.reconcile_observations.keys())
            .chain(state.reconcile_epochs.keys())
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        keys
    }

    pub fn clear(&self) {
        let mut state = self.inner.write();
        state.sources.clear();
        state.consumers.clear();
        state.reconcile_pending.clear();
        state.reconcile_observations.clear();
        state.reconcile_epochs.clear();
        state.results.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::ResourceKind;

    fn consumer(source_key: &str) -> DesiredCredentialConsumer {
        DesiredCredentialConsumer {
            resource: ResourceRef::new(
                ResourceKind::EdgionPlugins,
                Some("apps".to_string()),
                "provider".to_string(),
            ),
            uid: "consumer-uid".to_string(),
            generation: 3,
            source_key: source_key.to_string(),
            source_uid: "source-uid".to_string(),
            source_generation: 7,
            target_secret_namespace: "apps".to_string(),
            target_secret_name: "provider".to_string(),
            authorization_revision: "same-namespace".to_string(),
        }
    }

    #[test]
    fn consumer_replacement_is_atomic_by_logical_identity() {
        let registry = CredentialSourceRegistry::new();
        registry.replace_consumer(consumer("ops/source-a"));
        registry.replace_consumer(consumer("ops/source-b"));

        assert!(registry.consumers_for_source("ops/source-a").is_empty());
        assert_eq!(registry.consumers_for_source("ops/source-b").len(), 1);
    }

    #[test]
    fn clear_removes_all_desired_state() {
        let registry = CredentialSourceRegistry::new();
        registry.replace_consumer(consumer("ops/source"));
        registry.clear();
        assert!(registry.consumer("apps/provider").is_none());
        assert_eq!(registry.stats(), CredentialSourceRegistryStats::default());
        assert!(registry.keys().is_empty());
    }

    #[test]
    fn publication_reconcile_requests_coalesce_until_health_is_observed() {
        let registry = CredentialSourceRegistry::new();
        registry.replace_consumer(consumer("ops/source"));

        registry.request_consumer_reconcile("apps/provider", "missing".to_string());
        let first = registry.reconcile_epoch_for_source("ops/source");
        assert_ne!(first, 0);
        registry.request_consumer_reconcile("apps/provider", "missing".to_string());
        assert_eq!(registry.reconcile_epoch_for_source("ops/source"), first);

        registry.clear_consumer_reconcile("apps/provider");
        registry.request_consumer_reconcile("apps/provider", "missing".to_string());
        assert!(registry.reconcile_epoch_for_source("ops/source") > first);
        let second = registry.reconcile_epoch_for_source("ops/source");
        registry.request_consumer_reconcile("apps/provider", "uid:43".to_string());
        assert!(registry.reconcile_epoch_for_source("ops/source") > second);
    }
}
