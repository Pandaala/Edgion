//! In-memory synthetic EndpointSlices published by the Controller resolver.
//!
//! The EndpointSlice processor reads this store through
//! `ProcessorHandler::synthetic_object` whenever its real source (Kubernetes
//! watch, file system or etcd) has no object for a key, so these slices flow
//! through the ordinary EndpointSlice pipeline and conf_sync without ever
//! being written to the apiserver.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use k8s_openapi::api::discovery::v1::EndpointSlice;
use parking_lot::RwLock;

#[derive(Default)]
struct Inner {
    /// Bumped by `clear` (runtime reset); writes from an older resolver task
    /// carry a stale epoch and are rejected.
    epoch: u64,
    /// `namespace/name` -> slice.
    slices: HashMap<String, Arc<EndpointSlice>>,
    /// Service key -> its slice keys.
    by_service: HashMap<String, BTreeSet<String>>,
}

/// Controller-published synthetic slices, keyed like the EndpointSlice cache.
#[derive(Default)]
pub(crate) struct DnsSyntheticSlices {
    inner: RwLock<Inner>,
}

fn slice_key(slice: &EndpointSlice) -> Option<String> {
    Some(format!(
        "{}/{}",
        slice.metadata.namespace.as_deref()?,
        slice.metadata.name.as_deref()?
    ))
}

impl DnsSyntheticSlices {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn get(&self, key: &str) -> Option<EndpointSlice> {
        self.inner.read().slices.get(key).map(|slice| (**slice).clone())
    }

    /// The current write epoch; a resolver task captures it when it starts.
    pub(crate) fn epoch(&self) -> u64 {
        self.inner.read().epoch
    }

    /// Replace every slice of one Service (empty = withdraw) on behalf of a
    /// writer started at `epoch`. Returns the keys whose content changed, which
    /// the caller must enqueue; an identical replacement, or a write from a
    /// task that predates the last `clear`, returns nothing and changes nothing.
    pub(crate) fn replace_service(&self, epoch: u64, service_key: &str, slices: Vec<EndpointSlice>) -> Vec<String> {
        let incoming: HashMap<String, EndpointSlice> = slices
            .into_iter()
            .filter_map(|slice| slice_key(&slice).map(|key| (key, slice)))
            .collect();
        let mut inner = self.inner.write();
        if inner.epoch != epoch {
            return Vec::new();
        }
        let previous = inner.by_service.remove(service_key).unwrap_or_default();
        let mut changed = Vec::new();
        for key in &previous {
            if !incoming.contains_key(key) {
                inner.slices.remove(key);
                changed.push(key.clone());
            }
        }
        let mut keys = BTreeSet::new();
        for (key, slice) in incoming {
            if inner.slices.get(&key).is_none_or(|current| **current != slice) {
                inner.slices.insert(key.clone(), Arc::new(slice));
                changed.push(key.clone());
            }
            keys.insert(key);
        }
        if !keys.is_empty() {
            inner.by_service.insert(service_key.to_string(), keys);
        }
        changed.sort();
        changed
    }

    /// True when `service_key` currently owns slices.
    pub(crate) fn has_service(&self, service_key: &str) -> bool {
        self.inner.read().by_service.contains_key(service_key)
    }

    /// Service keys that currently own slices.
    pub(crate) fn service_keys(&self) -> Vec<String> {
        self.inner.read().by_service.keys().cloned().collect()
    }

    /// Number of synthetic slices (store-stats leak detection).
    pub(crate) fn len(&self) -> usize {
        self.inner.read().slices.len()
    }

    /// Drop every slice and reject writes from earlier resolver tasks.
    pub(crate) fn clear(&self) {
        let mut inner = self.inner.write();
        let epoch = inner.epoch.wrapping_add(1);
        *inner = Inner {
            epoch,
            ..Inner::default()
        };
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/services/dns_resolver/synthetic_test.rs"]
mod tests;
