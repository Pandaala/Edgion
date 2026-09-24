//! Source inventory and conf-sync adapter for EdgionStreamPlugins.
//!
//! The canonical publisher owns connection and TLSRoute executable views.
//! Each admission resolves its logical Gateway scope and selects the matching
//! runtime family; allowed views share source-generation entry instances.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use super::stream_plugin_runtime::StreamPluginRuntime;
use super::tls_route::TlsRoutePluginRuntime;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::EdgionStreamPlugins;

static GLOBAL_STREAM_PLUGIN_STORE: LazyLock<Arc<StreamPluginStore>> =
    LazyLock::new(|| Arc::new(StreamPluginStore { isolated: None }));

pub fn get_global_stream_plugin_store() -> Arc<StreamPluginStore> {
    GLOBAL_STREAM_PLUGIN_STORE.clone()
}

/// Create a handler for EdgionStreamPlugins
pub fn create_stream_plugin_handler() -> Box<dyn ConfHandler<EdgionStreamPlugins> + Send + Sync> {
    Box::new(get_global_stream_plugin_store())
}

/// Compiled EdgionStreamPlugins: immutable schema + the two gateway stream runtimes.
///
/// `spec` holds the raw schema (no runtime state); the two runtimes are
/// built on demand by the publisher's source generation and Arc-shared through
/// immutable scoped views.
pub struct CompiledStreamPlugins {
    /// The serializable schema, retained for stage declaration checks and store stats.
    pub spec: Arc<EdgionStreamPlugins>,
    /// Connection-context runtime used by listener and route-owned lifecycles.
    pub stream_plugin_runtime: Arc<StreamPluginRuntime>,
    /// TLSRoute-context runtime used after route matching.
    pub tls_route_plugin_runtime: Arc<TlsRoutePluginRuntime>,
}

/// Source inventory and conf-sync adapter for the canonical scoped publication.
pub struct StreamPluginStore {
    isolated: Option<Arc<crate::plugins::publication::PluginPublisher>>,
}

impl StreamPluginStore {
    fn publisher(&self) -> &crate::plugins::publication::PluginPublisher {
        self.isolated
            .as_deref()
            .unwrap_or_else(|| crate::plugins::publication::global_publisher())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.publisher().snapshot().stream_sources().contains_key(key)
    }

    pub fn count(&self) -> usize {
        self.publisher().snapshot().stream_sources().len()
    }

    /// Per-kind plugin entry count across all stored EdgionStreamPlugins resources.
    ///
    /// Discriminator: `EdgionStreamPlugin::type_name()` / `TlsRouteStreamPlugin::type_name()`
    /// (enum variant name, e.g. "IpRestriction", "GlobalConnectionIpRestriction").
    /// Both Stage 1 (ConnectionFilter) and Stage 2 (TlsRoute) entries are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let snapshot = self.publisher().snapshot();
        let map = snapshot.stream_sources();
        for esp in map.values() {
            if let Some(entries) = &esp.spec.plugins {
                for entry in entries {
                    *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
                }
            }
            if let Some(entries) = &esp.spec.tls_route_plugins {
                for entry in entries {
                    *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
                }
            }
        }
        out
    }

    /// Sample resource keys (namespace/name) holding stream plugins of the given kind.
    #[allow(dead_code)] // Reserved for validator/sample-diagnostics; not yet called from production paths.
    pub fn keys_by_kind(&self, kind: &str) -> Vec<String> {
        let mut out = Vec::new();
        let snapshot = self.publisher().snapshot();
        let map = snapshot.stream_sources();
        for (key, esp) in map.iter() {
            let in_stage1 = esp
                .spec
                .plugins
                .as_ref()
                .is_some_and(|entries| entries.iter().any(|e| e.plugin.type_name() == kind));
            let in_stage2 = esp
                .spec
                .tls_route_plugins
                .as_ref()
                .is_some_and(|entries| entries.iter().any(|e| e.plugin.type_name() == kind));
            if in_stage1 || in_stage2 {
                out.push(key.clone());
            }
        }
        out
    }
}

impl ConfHandler<EdgionStreamPlugins> for StreamPluginStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionStreamPlugins>>) {
        #[cfg(test)]
        self.prepare_test_scope(data.keys().cloned().collect());
        self.publisher().replace_stream_plugins(data.clone());
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionStreamPlugins>>,
        update: HashMap<String, Arc<EdgionStreamPlugins>>,
        remove: HashSet<String>,
    ) {
        let mut upserts = add;
        upserts.extend(update);
        #[cfg(test)]
        {
            let mut keys: HashSet<_> = self.publisher().snapshot().stream_sources().keys().cloned().collect();
            keys.extend(upserts.keys().cloned());
            keys.retain(|key| !remove.contains(key));
            self.prepare_test_scope(keys);
        }
        self.publisher().update_stream_plugins(upserts, &remove);
    }
}

#[cfg(test)]
include!("../../../tests/support/plugins/stream/stream_plugin_store_test/137_impl_item.rs");

#[cfg(test)]
include!("../../../tests/support/plugins/stream/stream_plugin_store_test/181_impl_item.rs");

#[cfg(test)]
#[path = "../../../tests/unit/plugins/stream/stream_plugin_store_test.rs"]
mod tests;
