//! Global store for EdgionStreamPlugins resources.
//!
//! The store holds **compiled** stream plugins (`Arc<CompiledStreamPlugins>`):
//! the immutable `EdgionStreamPlugins` schema paired with the two gateway-built
//! stream runtimes (Stage 1 ConnectionFilter, Stage 2 TlsRoute). The runtimes
//! are built once per (namespace/name) by `compile_stream_plugins` and
//! Arc-shared via the store, supporting hot reload through the dynamic store key.

use arc_swap::ArcSwap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use super::stream_plugin_runtime::StreamPluginRuntime;
use super::tls_route::TlsRoutePluginRuntime;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::EdgionStreamPlugins;

static GLOBAL_STREAM_PLUGIN_STORE: LazyLock<Arc<StreamPluginStore>> =
    LazyLock::new(|| Arc::new(StreamPluginStore::new()));

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
/// built by `compile_stream_plugins` and Arc-shared via the store.
pub struct CompiledStreamPlugins {
    /// The serializable EdgionStreamPlugins schema (retained for store stats).
    pub spec: Arc<EdgionStreamPlugins>,
    /// Stage 1 (ConnectionFilter) runtime — pre-TLS, IP-only.
    pub stream_plugin_runtime: Arc<StreamPluginRuntime>,
    /// Stage 2 (TlsRoute) runtime — post-handshake, post-route-match.
    pub tls_route_plugin_runtime: Arc<TlsRoutePluginRuntime>,
}

/// Build the compiled runtimes for an EdgionStreamPlugins resource.
///
/// When a stage's entries are absent, the runtime defaults to empty (operator
/// intent: declare an ESP with no rules as a no-op pass-through).
/// Built once per (namespace/name) and Arc-shared.
pub fn compile_stream_plugins(esp: &EdgionStreamPlugins) -> Arc<CompiledStreamPlugins> {
    let ns = esp.metadata.namespace.as_deref().unwrap_or("default");

    let stream_plugin_runtime = match &esp.spec.plugins {
        Some(plugins) => Arc::new(StreamPluginRuntime::from_stream_plugins(plugins, ns)),
        None => Arc::<StreamPluginRuntime>::default(),
    };
    let tls_route_plugin_runtime = match &esp.spec.tls_route_plugins {
        Some(entries) => Arc::new(TlsRoutePluginRuntime::from_entries(entries)),
        None => Arc::<TlsRoutePluginRuntime>::default(),
    };

    Arc::new(CompiledStreamPlugins {
        spec: Arc::new(esp.clone()),
        stream_plugin_runtime,
        tls_route_plugin_runtime,
    })
}

/// Type alias for the compiled stream plugin map (key: namespace/name)
type StreamPluginMap = HashMap<String, Arc<CompiledStreamPlugins>>;

pub struct StreamPluginStore {
    plugins: ArcSwap<StreamPluginMap>,
}

impl StreamPluginStore {
    pub fn new() -> Self {
        Self {
            plugins: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a stream plugin exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.plugins.load();
        map.contains_key(key)
    }

    /// Get the compiled stream plugins by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<CompiledStreamPlugins>> {
        let map = self.plugins.load();
        map.get(key).cloned()
    }

    /// Get the compiled stream plugins by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<CompiledStreamPlugins>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Execute a function with the compiled stream plugins reference
    pub fn with_plugin<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&CompiledStreamPlugins) -> R,
    {
        let map = self.plugins.load();
        map.get(key).map(|c| f(c))
    }

    /// Replace all compiled stream plugins atomically
    pub fn replace_all(&self, plugins: HashMap<String, Arc<CompiledStreamPlugins>>) {
        self.plugins.store(Arc::new(plugins));
    }

    /// Update compiled stream plugins atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<CompiledStreamPlugins>>, remove: &HashSet<String>) {
        let current = self.plugins.load();
        let mut new_map = (**current).clone();

        for (key, plugin) in add_or_update {
            new_map.insert(key, plugin);
        }

        for key in remove {
            new_map.remove(key);
        }

        self.plugins.store(Arc::new(new_map));
    }

    /// Get all compiled plugins
    pub fn get_all(&self) -> Arc<StreamPluginMap> {
        self.plugins.load_full()
    }

    /// Get total count of stream plugins
    pub fn count(&self) -> usize {
        self.plugins.load().len()
    }

    /// Per-kind plugin entry count across all stored EdgionStreamPlugins resources.
    ///
    /// Discriminator: `EdgionStreamPlugin::type_name()` / `TlsRouteStreamPlugin::type_name()`
    /// (enum variant name, e.g. "IpRestriction", "GlobalConnectionIpRestriction").
    /// Both Stage 1 (ConnectionFilter) and Stage 2 (TlsRoute) entries are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let map = self.plugins.load();
        for compiled in map.values() {
            let esp = &compiled.spec;
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
        let map = self.plugins.load();
        for (key, compiled) in map.iter() {
            let esp = &compiled.spec;
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

impl Default for StreamPluginStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile every incoming EdgionStreamPlugins into its gateway runtime companion.
fn compile_map(data: HashMap<String, Arc<EdgionStreamPlugins>>) -> HashMap<String, Arc<CompiledStreamPlugins>> {
    data.into_iter().map(|(k, v)| (k, compile_stream_plugins(&v))).collect()
}

impl ConfHandler<EdgionStreamPlugins> for StreamPluginStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionStreamPlugins>>) {
        tracing::info!(component = "stream_plugin_store", cnt = data.len(), "full set");
        let compiled = data
            .iter()
            .map(|(k, v)| (k.clone(), compile_stream_plugins(v)))
            .collect();
        self.replace_all(compiled);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionStreamPlugins>>,
        update: HashMap<String, Arc<EdgionStreamPlugins>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "stream_plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        let mut add_or_update = compile_map(add);
        add_or_update.extend(compile_map(update));
        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod kind_stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_empty_breakdown() {
        let store = StreamPluginStore::new();
        let by_kind = store.count_by_kind();
        assert!(by_kind.is_empty());
        let keys = store.keys_by_kind("IpRestriction");
        assert!(keys.is_empty());
    }
}
