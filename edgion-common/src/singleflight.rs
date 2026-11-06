//! Bounded weak per-key single-flight registry.
//!
//! Callers serialize same-key work by locking the returned [`Flight`] and
//! re-checking their own cache after acquiring it. Used by OIDC token refresh
//! and the shared DNS answer cache.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::Mutex as SyncMutex;
use tokio::sync::Mutex;

/// Absolute allocation-safety ceiling for the flights registered in one registry
/// (one OIDC plugin instance, or one DNS answer cache instance).
pub(crate) const MAX_FLIGHT_REGISTERED_KEYS: usize = 65_536;

fn validate_max_registered_keys(max_registered_keys: usize) -> Result<(), &'static str> {
    if max_registered_keys == 0 {
        return Err("singleflight max registered keys must be greater than zero");
    }
    if max_registered_keys > MAX_FLIGHT_REGISTERED_KEYS {
        return Err("singleflight max registered keys exceeds the safety ceiling");
    }
    Ok(())
}

/// A bounded weak registry of active flights.
///
/// The registry never owns a strong flight reference. Once the final caller
/// drops its [`Flight`], the entry removes itself by allocation
/// identity. Saturated keys receive an unregistered flight, keeping the map
/// bounded while deliberately degrading to uncoalesced refreshes.
pub struct FlightRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    max_registered_keys: usize,
    entries: SyncMutex<HashMap<String, Weak<Flight>>>,
}

pub struct Flight {
    lock: Mutex<()>,
    registered_key: Option<String>,
    registry: Weak<RegistryInner>,
    self_weak: Weak<Flight>,
}

impl FlightRegistry {
    pub fn new(max_registered_keys: usize) -> Result<Self, &'static str> {
        validate_max_registered_keys(max_registered_keys)?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                max_registered_keys,
                entries: SyncMutex::new(HashMap::new()),
            }),
        })
    }

    pub fn get_or_create(&self, key: &str) -> Arc<Flight> {
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
            return Flight::ephemeral();
        }

        let key = key.to_owned();
        let registry = Arc::downgrade(&self.inner);
        let flight = Arc::new_cyclic(|self_weak| Flight {
            lock: Mutex::new(()),
            registered_key: Some(key.clone()),
            registry,
            self_weak: self_weak.clone(),
        });
        entries.insert(key, Arc::downgrade(&flight));
        drop(entries);
        flight
    }

    /// Number of registered (coalescing) flights currently alive.
    pub fn len(&self) -> usize {
        self.inner.entries.lock().len()
    }

    /// Whether no registered flight is alive.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Up to `limit` keys of the registered flights currently alive (an
    /// arbitrary sample when more are registered).
    pub fn keys(&self, limit: usize) -> Vec<String> {
        self.inner.entries.lock().keys().take(limit).cloned().collect()
    }
}

impl Flight {
    fn ephemeral() -> Arc<Self> {
        Arc::new(Self {
            lock: Mutex::new(()),
            registered_key: None,
            registry: Weak::new(),
            self_weak: Weak::new(),
        })
    }

    pub fn lock(&self) -> &Mutex<()> {
        &self.lock
    }
}

impl Drop for Flight {
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
#[path = "../tests/unit/singleflight_test.rs"]
mod tests;
