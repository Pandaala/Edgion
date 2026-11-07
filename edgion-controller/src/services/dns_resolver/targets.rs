//! Work set of the control-plane DNS resolver.
//!
//! `ServiceHandler` keeps this table in step with the processed Service cache:
//! only Services the resolver must publish for (controller-mode ExternalName)
//! are present. The leader-only resolver iterates the table instead of listing
//! Services from the apiserver.

use std::collections::HashMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::Service;
use parking_lot::RwLock;
use tokio::sync::Notify;

use edgion_resources::constants::annotations::edgion::DNS_RESOLUTION_MODE;

use super::slice_builder::{decide_action, SliceAction};
use crate::config::dns_resolver_config;

/// One processed Service and the revision of its last content change.
#[derive(Clone)]
pub(crate) struct DnsResolverTarget {
    pub(crate) service: Arc<Service>,
    pub(crate) revision: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, DnsResolverTarget>,
    next_revision: u64,
}

/// Controller-mode ExternalName Services keyed by `namespace/name`.
#[derive(Default)]
pub(crate) struct DnsResolverTargets {
    inner: RwLock<Inner>,
    changed: Notify,
}

impl DnsResolverTargets {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replace the entry for `service` with its current published content.
    ///
    /// Must be called with the processed (published) object. Services that do not
    /// require resolver publication are removed, so an in-place edit off
    /// ExternalName or controller mode releases the entry.
    pub(crate) fn sync_published(&self, service: &Service) {
        let Some(key) = service_key(service) else {
            return;
        };
        if !is_resolver_target(service) {
            self.remove(&key);
            return;
        }
        let mut inner = self.inner.write();
        if inner
            .entries
            .get(&key)
            .is_some_and(|existing| existing.service.as_ref() == service)
        {
            return;
        }
        inner.next_revision += 1;
        let revision = inner.next_revision;
        inner.entries.insert(
            key,
            DnsResolverTarget {
                service: Arc::new(service.clone()),
                revision,
            },
        );
        drop(inner);
        self.changed.notify_one();
    }

    /// Remove the entry for an unpublished Service.
    pub(crate) fn remove_published(&self, service: &Service) {
        if let Some(key) = service_key(service) {
            self.remove(&key);
        }
    }

    fn remove(&self, key: &str) {
        if self.inner.write().entries.remove(key).is_some() {
            self.changed.notify_one();
        }
    }

    /// Point-in-time copy of every target.
    pub(crate) fn snapshot(&self) -> Vec<(String, DnsResolverTarget)> {
        self.inner
            .read()
            .entries
            .iter()
            .map(|(key, target)| (key.clone(), target.clone()))
            .collect()
    }

    /// Resolves once the table changed since the last wakeup was consumed.
    pub(crate) async fn changed(&self) {
        self.changed.notified().await;
    }

    pub(crate) fn clear(&self) {
        let mut inner = self.inner.write();
        if !inner.entries.is_empty() {
            inner.entries.clear();
            drop(inner);
            self.changed.notify_one();
        }
    }
}

fn service_key(service: &Service) -> Option<String> {
    let name = service.metadata.name.as_deref()?;
    let namespace = service.metadata.namespace.as_deref().unwrap_or("default");
    Some(format!("{namespace}/{name}"))
}

fn is_resolver_target(service: &Service) -> bool {
    let is_external_name = service
        .spec
        .as_ref()
        .is_some_and(|spec| spec.type_.as_deref() == Some("ExternalName") && spec.external_name.is_some());
    let mode = service
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(DNS_RESOLUTION_MODE))
        .map(String::as_str);
    decide_action(dns_resolver_config().enabled, is_external_name, mode) == SliceAction::Resolve
}

#[cfg(test)]
#[path = "../../../tests/unit/services/dns_resolver/targets_test.rs"]
mod tests;
