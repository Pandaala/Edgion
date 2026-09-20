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
impl StreamPluginStore {
    pub fn new() -> Self {
        Self {
            isolated: Some(Arc::new(crate::plugins::publication::PluginPublisher::new())),
        }
    }

    fn prepare_test_scope(&self, keys: HashSet<String>) {
        if self.isolated.is_none() {
            return;
        }
        use crate::plugins::publication::inputs::GatewayInput;
        use edgion_resources::{Gateway, GatewayClass};
        let class: GatewayClass = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"stream-store-test"},
            "spec":{"controllerName":"edgion.io/gateway-controller"}
        }))
        .unwrap();
        let gateway: Gateway = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"Gateway","metadata":{"name":"scope","namespace":"stream-store-test"},
            "spec":{"gatewayClassName":"stream-store-test","listeners":[{"name":"tcp","port":18089,"protocol":"TCP"}]}
        })).unwrap();
        let base = GatewayInput::from_gateway(&gateway);
        let mut gateways = HashMap::from([("stream-store-test/scope".into(), base.clone())]);
        for (index, key) in keys.into_iter().enumerate() {
            let mut input = base.clone();
            input.connection_key = Some(key);
            gateways.insert(format!("stream-store-test/attachment-{index}"), input);
        }
        self.publisher()
            .replace_gateway_classes(HashMap::from([("stream-store-test".into(), Arc::new(class))]));
        self.publisher().replace_gateways(gateways);
    }

    pub fn get(&self, key: &str) -> Option<Arc<CompiledStreamPlugins>> {
        self.publisher()
            .snapshot()
            .gateway("stream-store-test/scope")?
            .scope
            .stream(key)
    }
}

#[cfg(test)]
impl Default for StreamPluginStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamLayer, StreamPluginResult};
    use edgion_resources::resource::meta::ResourceMeta;
    use std::net::IpAddr;

    fn local_limit_resource(name: &str, rate: u64, enabled: bool) -> Arc<EdgionStreamPlugins> {
        let yaml = format!(
            r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata: {{name: {name}, namespace: default}}
spec:
  plugins:
    - type: ConnectionRateLimit
      enable: {enabled}
      config:
        perListener: {{rate: {rate}, interval: 1h}}
"#
        );
        Arc::new(serde_yaml::from_str(&yaml).unwrap())
    }

    #[tokio::test]
    async fn local_budget_survives_full_delta_delete_and_disabled_generations() {
        let store = StreamPluginStore::new();
        let name = "local-budget-store-reload";
        let key = format!("default/{name}");
        let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18089, StreamLayer::Stage1);
        store.full_set(&HashMap::from([(key.clone(), local_limit_resource(name, 2, true))]));
        let old = store.get(&key).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                old.stream_plugin_runtime.run(&ctx).await,
                StreamPluginResult::Allow
            ));
        }
        assert!(matches!(
            old.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));

        store.full_set(&HashMap::from([(key.clone(), local_limit_resource(name, 2, true))]));
        assert!(matches!(
            store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
            HashSet::new(),
        );
        let raised = store.get(&key).unwrap();
        assert!(matches!(
            raised.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Allow
        ));
        assert!(matches!(
            old.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Allow
        ));
        assert!(matches!(
            raised.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));

        store.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
        assert!(store.get(&key).is_none());
        assert!(matches!(
            old.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));
        store.partial_update(
            HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
            HashMap::new(),
            HashSet::new(),
        );
        assert!(matches!(
            store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), local_limit_resource(name, 100, false))]),
            HashSet::new(),
        );
        assert!(matches!(
            store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Allow
        ));
        store.partial_update(
            HashMap::new(),
            HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
            HashSet::new(),
        );
        assert!(matches!(
            store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Deny(_)
        ));
    }

    fn fixture(name: &str) -> Arc<EdgionStreamPlugins> {
        let yaml = format!(
            r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: {name}
  namespace: default
spec: {{}}
"#,
        );
        Arc::new(serde_yaml::from_str(&yaml).expect("minimal EdgionStreamPlugins fixture must parse"))
    }

    #[test]
    fn empty_store_reports_empty_breakdown() {
        let store = StreamPluginStore::new();
        let by_kind = store.count_by_kind();
        assert!(by_kind.is_empty());
        let keys = store.keys_by_kind("IpRestriction");
        assert!(keys.is_empty());
    }

    #[test]
    fn full_set_publishes_then_empty_full_set_clears() {
        let store = StreamPluginStore::new();
        let resource = fixture("full-set");
        let key = resource.key_name();
        store.full_set(&HashMap::from([(key.clone(), resource)]));

        let published = store.get(&key).expect("full set must publish the compiled resource");
        assert_eq!(store.count(), 1);
        assert_eq!(published.spec.key_name(), key);

        store.full_set(&HashMap::new());
        assert_eq!(store.count(), 0);
        assert!(!store.contains(&key));
        assert_eq!(published.spec.key_name(), "default/full-set");
    }

    #[test]
    fn partial_delta_preserves_replaces_adds_and_removes_snapshots() {
        let store = StreamPluginStore::new();
        let replace_key = "default/replace".to_string();
        let keep_key = "default/keep".to_string();
        let remove_key = "default/remove".to_string();
        store.full_set(&HashMap::from([
            (replace_key.clone(), fixture("replace")),
            (keep_key.clone(), fixture("keep")),
            (remove_key.clone(), fixture("remove")),
        ]));

        let old_replaced = store.get(&replace_key).expect("replacement predecessor must exist");
        let old_kept = store.get(&keep_key).expect("unaffected entry must exist");
        let mut changed = (*fixture("replace")).clone();
        changed.spec.plugins = Some(Vec::new());
        store.partial_update(
            HashMap::from([("default/add".to_string(), fixture("add"))]),
            HashMap::from([(replace_key.clone(), Arc::new(changed))]),
            HashSet::from([remove_key.clone()]),
        );

        let replacement = store.get(&replace_key).expect("updated entry must remain present");
        let kept = store.get(&keep_key).expect("unaffected entry must remain present");
        assert!(!Arc::ptr_eq(&old_replaced, &replacement));
        assert!(Arc::ptr_eq(&old_kept, &kept));
        assert!(store.contains("default/add"));
        assert!(!store.contains(&remove_key));
        assert_eq!(store.count(), 3);
        assert_eq!(old_replaced.spec.key_name(), replace_key);
    }
}
