//! Source-scoped LIST publication. Only complete native snapshots become visible.

use std::collections::HashSet;
use std::hash::Hash;

use anyhow::{bail, Result};
use kube::runtime::reflector::{store::Writer, Store};
use kube::runtime::watcher::Event;
use kube::{Resource, ResourceExt};

use crate::conf_mgr::sync_runtime::resource_processor::make_resource_key;

#[derive(Clone, Debug)]
pub(super) enum WatchScope {
    AllNamespaces,
    Namespace(String),
    Cluster,
}

pub(super) struct SourceState<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    pub scope: WatchScope,
    writer: Writer<K>,
    pub store: Store<K>,
    listing: bool,
    pub ever_committed: bool,
}

impl<K> SourceState<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    pub fn new(scope: WatchScope) -> Self {
        let writer = Writer::new(K::DynamicType::default());
        let store = writer.as_reader();
        Self {
            scope,
            writer,
            store,
            listing: false,
            ever_committed: false,
        }
    }

    /// Validate before touching Writer: an unguarded duplicate InitDone empties it.
    /// Returned keys refer to committed state; no locks survive this call.
    pub fn apply(&mut self, event: Event<K>) -> Result<Vec<String>> {
        match &event {
            Event::InitApply(obj) | Event::Apply(obj) | Event::Delete(obj) => {
                if obj.meta().name.as_deref().is_none_or(str::is_empty) {
                    bail!("Watch source object has no name");
                }
                let namespace = obj.namespace();
                let valid = match &self.scope {
                    WatchScope::AllNamespaces => namespace.as_deref().is_some_and(|ns| !ns.is_empty()),
                    WatchScope::Namespace(expected) => namespace.as_ref() == Some(expected),
                    WatchScope::Cluster => namespace.is_none(),
                };
                if !valid {
                    bail!("Watch source object has invalid namespace ownership");
                }
            }
            _ => {}
        }
        let keys = match &event {
            Event::Init => {
                self.listing = true;
                Vec::new()
            }
            Event::InitApply(_) if self.listing => Vec::new(),
            Event::InitDone if self.listing => {
                let old: HashSet<_> = self
                    .store
                    .state()
                    .iter()
                    .map(|obj| make_resource_key(obj.as_ref()))
                    .collect();
                self.writer.apply_watcher_event(&event);
                self.listing = false;
                self.ever_committed = true;
                let mut keys = old;
                keys.extend(self.store.state().iter().map(|obj| make_resource_key(obj.as_ref())));
                return Ok(keys.into_iter().collect());
            }
            Event::InitDone => return Ok(Vec::new()),
            Event::Apply(obj) | Event::Delete(obj) if self.ever_committed && !self.listing => {
                vec![make_resource_key(obj)]
            }
            _ => bail!("Invalid watch source lifecycle transition"),
        };
        self.writer.apply_watcher_event(&event);
        Ok(keys)
    }
}

impl<K> Drop for SourceState<K>
where
    K: Resource + Clone + 'static,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    fn drop(&mut self) {
        // Diagnostic readers outlive the driver. Release committed and staging objects.
        self.writer.apply_watcher_event(&Event::Init);
        self.writer.apply_watcher_event(&Event::InitDone);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/watch_source_test.rs"]
mod tests;
