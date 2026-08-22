//! Bounded per-key coordination for OIDC token refreshes.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::Mutex as SyncMutex;
use tokio::sync::Mutex;

/// Absolute allocation-safety ceiling for one plugin's registered flights.
pub(crate) const MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS: usize = 65_536;

pub(crate) fn validate_max_registered_keys(max_registered_keys: usize) -> Result<(), &'static str> {
    if max_registered_keys == 0 {
        return Err("refresh singleflight max registered keys must be greater than zero");
    }
    if max_registered_keys > MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS {
        return Err("refresh singleflight max registered keys exceeds the safety ceiling");
    }
    Ok(())
}

/// A bounded weak registry of active refresh flights.
///
/// The registry never owns a strong flight reference. Once the final caller
/// drops its [`RefreshFlight`], the entry removes itself by allocation
/// identity. Saturated keys receive an unregistered flight, keeping the map
/// bounded while deliberately degrading to uncoalesced refreshes.
pub(super) struct RefreshFlightRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    max_registered_keys: usize,
    entries: SyncMutex<HashMap<String, Weak<RefreshFlight>>>,
}

pub(super) struct RefreshFlight {
    lock: Mutex<()>,
    registered_key: Option<String>,
    registry: Weak<RegistryInner>,
    self_weak: Weak<RefreshFlight>,
}

impl RefreshFlightRegistry {
    pub(super) fn new(max_registered_keys: usize) -> Result<Self, &'static str> {
        validate_max_registered_keys(max_registered_keys)?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                max_registered_keys,
                entries: SyncMutex::new(HashMap::new()),
            }),
        })
    }

    pub(super) fn get_or_create(&self, key: &str) -> Arc<RefreshFlight> {
        let mut entries = self.inner.entries.lock();

        if let Some(existing) = entries.get(key) {
            if let Some(flight) = existing.upgrade() {
                drop(entries);
                return flight;
            }
            entries.remove(key);
        }

        if entries.len() >= self.inner.max_registered_keys {
            drop(entries);
            return RefreshFlight::ephemeral();
        }

        let key = key.to_owned();
        let registry = Arc::downgrade(&self.inner);
        let flight = Arc::new_cyclic(|self_weak| RefreshFlight {
            lock: Mutex::new(()),
            registered_key: Some(key.clone()),
            registry,
            self_weak: self_weak.clone(),
        });
        entries.insert(key, Arc::downgrade(&flight));
        drop(entries);
        flight
    }

    #[cfg(test)]
    fn registered_len(&self) -> usize {
        self.inner.entries.lock().len()
    }
}

impl RefreshFlight {
    fn ephemeral() -> Arc<Self> {
        Arc::new_cyclic(|self_weak| Self {
            lock: Mutex::new(()),
            registered_key: None,
            registry: Weak::new(),
            self_weak: self_weak.clone(),
        })
    }

    pub(super) fn lock(&self) -> &Mutex<()> {
        &self.lock
    }

    #[cfg(test)]
    fn is_registered(&self) -> bool {
        self.registered_key.is_some()
    }
}

impl Drop for RefreshFlight {
    fn drop(&mut self) {
        let Some(key) = self.registered_key.as_ref() else {
            return;
        };
        let Some(registry) = self.registry.upgrade() else {
            return;
        };

        let mut entries = registry.entries.lock();
        let owns_entry = entries
            .get(key)
            .is_some_and(|registered| Weak::ptr_eq(registered, &self.self_weak));
        if owns_entry {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::RefreshFlightRegistry;

    #[test]
    fn same_key_reuses_flight_and_last_owner_cleans_entry() {
        let registry = RefreshFlightRegistry::new(2).unwrap();
        let first = registry.get_or_create("digest-a");
        let second = registry.get_or_create("digest-a");

        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert_eq!(registry.registered_len(), 1);
        drop(first);
        assert_eq!(registry.registered_len(), 1);
        drop(second);
        assert_eq!(registry.registered_len(), 0);
    }

    #[test]
    fn different_keys_receive_independent_flights() {
        let registry = RefreshFlightRegistry::new(2).unwrap();
        let first = registry.get_or_create("digest-a");
        let second = registry.get_or_create("digest-b");

        assert!(!std::sync::Arc::ptr_eq(&first, &second));
        assert_eq!(registry.registered_len(), 2);
    }

    #[test]
    fn saturation_uses_unregistered_flights_without_growing_map() {
        let registry = RefreshFlightRegistry::new(1).unwrap();
        let registered = registry.get_or_create("digest-a");
        let first_ephemeral = registry.get_or_create("digest-b");
        let second_ephemeral = registry.get_or_create("digest-b");

        assert!(registered.is_registered());
        assert!(!first_ephemeral.is_registered());
        assert!(!second_ephemeral.is_registered());
        assert!(!std::sync::Arc::ptr_eq(&first_ephemeral, &second_ephemeral));
        assert_eq!(registry.registered_len(), 1);
    }

    #[test]
    fn final_drop_does_not_deadlock_on_registry_mutex() {
        let registry = RefreshFlightRegistry::new(1).unwrap();
        let flight = registry.get_or_create("digest-a");
        let (sent, received) = mpsc::channel();

        std::thread::spawn(move || {
            drop(flight);
            sent.send(()).unwrap();
        });

        received
            .recv_timeout(Duration::from_secs(1))
            .expect("last-owner cleanup must not deadlock");
        assert_eq!(registry.registered_len(), 0);
    }

    #[test]
    fn zero_limit_is_rejected() {
        assert!(RefreshFlightRegistry::new(0).is_err());
    }

    #[test]
    fn absolute_limit_is_validated_before_registration() {
        assert!(RefreshFlightRegistry::new(super::MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS).is_ok());
        assert!(RefreshFlightRegistry::new(super::MAX_REFRESH_SINGLEFLIGHT_REGISTERED_KEYS + 1).is_err());
    }
}
