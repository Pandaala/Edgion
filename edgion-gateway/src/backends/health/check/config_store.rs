use arc_swap::ArcSwap;
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::HealthPolicyIdentity;

static HC_CONFIG_STORE: LazyLock<Arc<HealthCheckConfigStore>> =
    LazyLock::new(|| Arc::new(HealthCheckConfigStore::new()));
static NEXT_HEALTH_POLICY_GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn get_hc_config_store() -> Arc<HealthCheckConfigStore> {
    HC_CONFIG_STORE.clone()
}

/// Per-service active health check configuration store.
///
/// A single map keyed by Service key (`namespace/service`). The
/// `EdgionBackendTrafficPolicy` conf-handler is the sole writer: it computes the
/// winning policy per Service key and pushes the effective config via
/// `set_service_config`. There are no source tiers anymore.
pub struct HealthCheckConfigStore {
    configs: ArcSwap<HashMap<String, Arc<ResolvedActiveHealthPolicy>>>,
    publication: parking_lot::RwLock<()>,
}

pub(crate) struct ResolvedActiveHealthPolicy {
    pub config: ActiveHealthCheckConfig,
    pub identity: Arc<HealthPolicyIdentity>,
}

impl Default for HealthCheckConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheckConfigStore {
    pub fn new() -> Self {
        Self {
            configs: ArcSwap::from_pointee(HashMap::new()),
            publication: parking_lot::RwLock::new(()),
        }
    }

    /// Resolve the active health check config for a Service key, if any.
    pub fn resolve(&self, service_key: &str) -> Option<ActiveHealthCheckConfig> {
        self.configs.load().get(service_key).map(|policy| policy.config.clone())
    }

    pub(crate) fn resolve_policy(&self, service_key: &str) -> Option<Arc<ResolvedActiveHealthPolicy>> {
        self.configs.load().get(service_key).cloned()
    }

    /// Set or clear the config for a Service key. `None` removes any entry.
    pub fn set_service_config(&self, service_key: &str, config: Option<ActiveHealthCheckConfig>) {
        let _publication = self.publication.write();
        let mut new_map = (**self.configs.load()).clone();
        match config {
            Some(cfg) => {
                if new_map.get(service_key).is_some_and(|current| current.config == cfg) {
                    return;
                }
                let Some(generation) = NEXT_HEALTH_POLICY_GENERATION
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1))
                    .ok()
                else {
                    new_map.remove(service_key);
                    self.configs.store(Arc::new(new_map));
                    return;
                };
                new_map.insert(
                    service_key.to_string(),
                    Arc::new(ResolvedActiveHealthPolicy {
                        config: cfg,
                        identity: Arc::new(HealthPolicyIdentity { generation }),
                    }),
                );
            }
            None => {
                new_map.remove(service_key);
            }
        }
        self.configs.store(Arc::new(new_map));
    }

    /// Each poll is admitted atomically with respect to configuration publication.
    /// The synchronous read guard is dropped before this wrapper yields Pending.
    pub(crate) async fn with_current_generation<F, T>(
        &self,
        service_key: &str,
        identity: &Arc<HealthPolicyIdentity>,
        future: F,
    ) -> Result<T, super::probes::ProbeError>
    where
        F: std::future::Future<Output = Result<T, super::probes::ProbeError>>,
    {
        let mut future = std::pin::pin!(future);
        futures::future::poll_fn(|cx| {
            let _publication = self.publication.read();
            let current = self.configs.load();
            if !current
                .get(service_key)
                .is_some_and(|policy| Arc::ptr_eq(&policy.identity, identity))
            {
                return std::task::Poll::Ready(Err(super::probes::ProbeError::Superseded));
            }
            future.as_mut().poll(cx)
        })
        .await
    }

    /// Number of Service keys currently configured.
    pub fn count(&self) -> usize {
        self.configs.load().len()
    }

    /// Service keys currently configured (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.configs.load().keys().cloned().collect()
    }

    /// Remove the config for a Service key (equivalent to `set_service_config(key, None)`).
    pub fn remove_service(&self, service_key: &str) {
        self.set_service_config(service_key, None);
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = HealthCheckConfigStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};

    fn test_config(tp: HealthCheckType) -> ActiveHealthCheckConfig {
        ActiveHealthCheckConfig {
            r#type: tp,
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_none() {
        let store = HealthCheckConfigStore::new();
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_set_then_resolve_returns_config() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.r#type, HealthCheckType::Http);
    }

    #[test]
    fn test_set_none_removes() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
        assert!(store.resolve("default/svc").is_some());

        store.set_service_config("default/svc", None);
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_remove_service_is_set_none() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.remove_service("default/svc");
        assert!(store.resolve("default/svc").is_none());
    }

    #[test]
    fn test_overwrite_replaces_config() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Http)));
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));

        let resolved = store.resolve("default/svc").expect("resolved config");
        assert_eq!(resolved.r#type, HealthCheckType::Tcp);
    }

    #[test]
    fn identical_config_reuses_policy_identity_but_change_replaces_it() {
        let store = HealthCheckConfigStore::new();
        let config = test_config(HealthCheckType::Http);
        store.set_service_config("default/svc", Some(config.clone()));
        let first = store.resolve_policy("default/svc").unwrap();
        store.set_service_config("default/svc", Some(config));
        let same = store.resolve_policy("default/svc").unwrap();
        assert!(Arc::ptr_eq(&first.identity, &same.identity));

        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Tcp)));
        let changed = store.resolve_policy("default/svc").unwrap();
        assert!(!Arc::ptr_eq(&first.identity, &changed.identity));
        assert!(changed.identity.generation > first.identity.generation);
    }

    #[test]
    fn test_keys_and_count_track_entries() {
        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/a", Some(test_config(HealthCheckType::Http)));
        store.set_service_config("default/b", Some(test_config(HealthCheckType::Tcp)));
        assert_eq!(store.count(), 2);
        let mut keys = store.keys();
        keys.sort();
        assert_eq!(keys, vec!["default/a".to_string(), "default/b".to_string()]);
    }

    #[test]
    fn publication_revokes_an_unpolled_encrypted_probe() {
        use futures::task::noop_waker;
        use std::future::Future;
        use std::task::{Context, Poll};

        let store = HealthCheckConfigStore::new();
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Https)));
        let identity = store.resolve_policy("default/svc").unwrap().identity.clone();
        let probe = async { panic!("an old queued probe must never start") };
        let guarded = store.with_current_generation::<_, ()>("default/svc", &identity, probe);
        let mut guarded = std::pin::pin!(guarded);
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Grpcs)));
        let waker = noop_waker();
        assert!(matches!(
            guarded.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(Err(super::super::probes::ProbeError::Superseded))
        ));
    }

    #[test]
    fn publication_waits_for_poll_and_revokes_before_repoll() {
        use futures::task::noop_waker;
        use std::future::Future;
        use std::sync::atomic::AtomicUsize;
        use std::task::{Context, Poll};

        let store = Arc::new(HealthCheckConfigStore::new());
        store.set_service_config("default/svc", Some(test_config(HealthCheckType::Https)));
        let identity = store.resolve_policy("default/svc").unwrap().identity.clone();
        let polls = AtomicUsize::new(0);
        let (poll_entered, publisher_start) = std::sync::mpsc::sync_channel(0);
        let (gate_checked, checked) = std::sync::mpsc::sync_channel(0);
        let writer_store = Arc::clone(&store);
        let writer = std::thread::spawn(move || {
            publisher_start.recv().unwrap();
            assert!(writer_store.publication.try_write().is_none());
            gate_checked.send(()).unwrap();
            writer_store.set_service_config("default/svc", Some(test_config(HealthCheckType::Grpcs)));
        });
        let probe = futures::future::poll_fn(|_| {
            polls.fetch_add(1, Ordering::Relaxed);
            poll_entered.send(()).unwrap();
            checked.recv().unwrap();
            Poll::<Result<(), super::super::probes::ProbeError>>::Pending
        });
        let guarded = store.with_current_generation("default/svc", &identity, probe);
        let mut guarded = std::pin::pin!(guarded);
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(guarded.as_mut().poll(&mut context).is_pending());
        writer.join().unwrap();
        assert!(matches!(
            guarded.as_mut().poll(&mut context),
            Poll::Ready(Err(super::super::probes::ProbeError::Superseded))
        ));
        assert_eq!(polls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn resolved_secret_rotation_replaces_health_generation() {
        let store = HealthCheckConfigStore::new();
        let mut config = test_config(HealthCheckType::Https);
        let secret = |bytes: &[u8]| k8s_openapi::api::core::v1::Secret {
            data: Some(std::collections::BTreeMap::from([(
                "ca.crt".to_owned(),
                k8s_openapi::ByteString(bytes.to_vec()),
            )])),
            ..Default::default()
        };
        config.resolved_ca_certificates = Some(vec![secret(b"first")]);
        store.set_service_config("default/svc", Some(config.clone()));
        let first = store.resolve_policy("default/svc").unwrap();
        config.resolved_ca_certificates = Some(vec![secret(b"second")]);
        store.set_service_config("default/svc", Some(config.clone()));
        let second = store.resolve_policy("default/svc").unwrap();
        assert!(!Arc::ptr_eq(&first.identity, &second.identity));
        config.resolved_client_certificate = Some(secret(b"client"));
        store.set_service_config("default/svc", Some(config));
        let third = store.resolve_policy("default/svc").unwrap();
        assert!(!Arc::ptr_eq(&second.identity, &third.identity));
    }
}
