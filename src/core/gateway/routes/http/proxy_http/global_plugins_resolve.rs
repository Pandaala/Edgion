//! Per-request resolve helpers for global plugin refs and rule-level
//! ExtensionRefs.
//!
//! Both helpers are pure: input refs/keys -> output Vec, with one
//! `tracing::warn!` per dangling reference. They are called from
//! `pg_request_filter` once per request to populate
//! `ctx.resolved_global_plugins` and `ctx.resolved_route_ext_refs`.

use std::sync::Arc;

use crate::core::gateway::plugins::http::get_global_plugin_store;
use crate::core::gateway::plugins::runtime::PluginRuntime;
use crate::types::resources::edgion_gateway_config::PluginReference;

/// Resolve `EdgionGatewayConfig.spec.global_plugins_ref` to a Vec of plugin
/// runtimes, preserving order. Dangling refs are warned once and dropped from
/// the output.
///
/// `gateway_config_name` is for diagnostic logging — pass the
/// `EdgionGatewayConfig` resource name so warnings are traceable to a
/// specific config.
pub fn resolve_global_plugins(refs: Option<&[PluginReference]>, gateway_config_name: &str) -> Vec<Arc<PluginRuntime>> {
    let Some(refs) = refs else { return Vec::new() };
    let store = get_global_plugin_store();
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let ns = r.namespace.as_deref().unwrap_or("default");
        let key = format!("{}/{}", ns, r.name);
        match store.get_plugin_runtime(&key) {
            Some(rt) => out.push(rt),
            None => {
                tracing::warn!(
                    gateway_config = %gateway_config_name,
                    namespace = %ns,
                    name = %r.name,
                    "global_plugins_ref points to non-existent EdgionPlugins; skipping",
                );
            }
        }
    }
    out
}

/// Resolve a rule's ExtensionRef keys (built by
/// `PluginRuntime::ext_ref_keys()`) to positional Vec of resolved runtimes.
///
/// `None` slots are dangling refs (warned once each); position is preserved
/// because `ExtensionRefFilter.index` directly indexes into this Vec.
///
/// `route_name` is for diagnostic logging — pass the resource key
/// (`namespace/name`) of the route that owns this rule.
pub fn resolve_route_ext_refs(keys: &[(String, String)], route_name: &str) -> Vec<Option<Arc<PluginRuntime>>> {
    let store = get_global_plugin_store();
    let mut out = Vec::with_capacity(keys.len());
    for (ns, name) in keys {
        let key = format!("{}/{}", ns, name);
        match store.get_plugin_runtime(&key) {
            Some(rt) => out.push(Some(rt)),
            None => {
                tracing::warn!(
                    route = %route_name,
                    namespace = %ns,
                    name = %name,
                    "ExtensionRef target EdgionPlugins not found in store; using None placeholder",
                );
                out.push(None);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
    use crate::types::resources::http_route::HTTPHeaderFilter;
    use kube::core::ObjectMeta;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::LazyLock;

    /// Serialize tests within this module since they all mutate the global
    /// plugin store via `replace_all`. Cross-module collisions with other
    /// tests that touch the store are still possible — that matches the
    /// existing convention in `extension_ref.rs` tests.
    static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    /// Build a minimal preparsed EdgionPlugins keyed by `{ns}/{name}`.
    fn make_plugin(ns: &str, name: &str) -> (String, EdgionPlugins) {
        let mut ep = EdgionPlugins {
            metadata: ObjectMeta {
                namespace: Some(ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                    HTTPHeaderFilter {
                        set: None,
                        add: None,
                        remove: None,
                    },
                ))]),
                ..Default::default()
            },
            status: None,
        };
        ep.preparse();
        (format!("{}/{}", ns, name), ep)
    }

    /// Replace the entire global store contents with the given plugins.
    fn install_plugins(entries: Vec<(&str, &str)>) {
        let mut map = HashMap::new();
        for (ns, name) in entries {
            let (k, ep) = make_plugin(ns, name);
            map.insert(k, ep);
        }
        get_global_plugin_store().replace_all(map);
    }

    fn clear_store() {
        get_global_plugin_store().replace_all(HashMap::new());
    }

    fn pref(name: &str, ns: Option<&str>) -> PluginReference {
        PluginReference {
            name: name.to_string(),
            namespace: ns.map(|s| s.to_string()),
        }
    }

    #[test]
    fn resolve_global_plugins_empty_when_none() {
        let _g = TEST_LOCK.lock();
        clear_store();
        let resolved = resolve_global_plugins(None, "gw1");
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_global_plugins_keeps_order_and_drops_dangling() {
        let _g = TEST_LOCK.lock();
        install_plugins(vec![("ns1", "p1"), ("ns1", "p3")]);

        let refs = vec![
            pref("p1", Some("ns1")),
            pref("p2", Some("ns1")), // dangling
            pref("p3", Some("ns1")),
        ];
        let resolved = resolve_global_plugins(Some(&refs), "gw1");
        assert_eq!(resolved.len(), 2, "dangling p2 should be dropped");
    }

    #[test]
    fn resolve_global_plugins_default_namespace_when_none() {
        let _g = TEST_LOCK.lock();
        install_plugins(vec![("default", "global1")]);
        let refs = vec![pref("global1", None)];
        let resolved = resolve_global_plugins(Some(&refs), "gw1");
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn resolve_route_ext_refs_preserves_position_with_none_for_dangling() {
        let _g = TEST_LOCK.lock();
        install_plugins(vec![("ns1", "p1")]);
        let keys = vec![
            ("ns1".to_string(), "p1".to_string()),
            ("ns1".to_string(), "missing".to_string()),
            ("ns1".to_string(), "p1".to_string()), // duplicate ok
        ];
        let resolved = resolve_route_ext_refs(&keys, "route-x");
        assert_eq!(resolved.len(), 3);
        assert!(resolved[0].is_some());
        assert!(resolved[1].is_none(), "dangling slot must be None");
        assert!(resolved[2].is_some());
    }

    #[test]
    fn resolve_route_ext_refs_empty_keys_yields_empty_vec() {
        let _g = TEST_LOCK.lock();
        clear_store();
        let resolved = resolve_route_ext_refs(&[], "route-x");
        assert!(resolved.is_empty());
    }
}
