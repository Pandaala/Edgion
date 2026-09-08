//! Per-request resolve helpers for global plugin refs, Gateway-level plugin
//! keys, and rule-level ExtensionRefs.
//!
//! All helpers are pure: input refs/keys -> output Vec. They are called
//! from `pg_request_filter` once per request to populate the lazily allocated
//! plugin-runtime snapshot state.
//! Dangling references are silently dropped (global/gateway refs) or kept as a
//! `None` placeholder (ext refs); they are intentionally NOT logged here
//! because this is the data-plane hot path, where `tracing` is forbidden
//! (see `skills/03-coding/01-log-safety.md` Iron Rule 3 — a single
//! misconfigured ref would otherwise emit one warn per request, scaling
//! the log volume with QPS).

use std::sync::Arc;

use crate::ctx::{WafLogSource, WafSelection};
use crate::plugins::http::PluginStore;
use crate::plugins::runtime::PluginRuntime;
use edgion_resources::resources::edgion_gateway_config::PluginReference;

/// Resolve `EdgionGatewayConfig.spec.global_plugins_ref` to a Vec of plugin
/// runtimes, preserving order. Dangling refs are silently dropped from the
/// output — no logging, this runs on the data-plane hot path.
///
/// Takes `&PluginStore` so the caller can fetch the global store once per
/// request and share it across multiple resolve calls, avoiding redundant
/// `Arc::clone` of `GLOBAL_PLUGIN_STORE`.
pub fn resolve_global_plugins(store: &PluginStore, refs: Option<&[PluginReference]>) -> Vec<Arc<PluginRuntime>> {
    let Some(refs) = refs else { return Vec::new() };
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let ns = r.namespace.as_deref().unwrap_or("default");
        let key = format!("{}/{}", ns, r.name);
        if let Some(rt) = store.get_plugin_runtime(&key) {
            out.push(rt);
        }
    }
    out
}

/// Resolve a rule's ExtensionRef keys (built by
/// `PluginRuntime::ext_ref_keys()`) to positional Vec of resolved runtimes.
///
/// `None` slots are dangling refs; position is preserved because
/// `ExtensionRefFilter.index` directly indexes into this Vec. Dangling refs
/// are not logged here — this runs on the data-plane hot path.
///
/// Takes `&PluginStore` so the caller can fetch the global store once per
/// request and share it across multiple resolve calls.
pub fn resolve_route_ext_refs(store: &PluginStore, keys: &[(String, String)]) -> Vec<Option<Arc<PluginRuntime>>> {
    let mut out = Vec::with_capacity(keys.len());
    for (ns, name) in keys {
        let key = format!("{}/{}", ns, name);
        out.push(store.get_plugin_runtime(&key));
    }
    out
}

/// Resolve already-formed "namespace/name" store keys (from a Gateway's
/// `edgion.io/edgion-plugins` annotation) into plugin runtimes, preserving order and
/// dropping dangling refs (hot path: no logging).
pub fn resolve_gateway_plugins(store: &PluginStore, keys: &[String]) -> Vec<Arc<PluginRuntime>> {
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(rt) = store.get_plugin_runtime(key) {
            out.push(rt);
        }
    }
    out
}

#[derive(Clone, Copy)]
enum WafScope {
    Global = 0,
    Gateway = 1,
    Route = 2,
}

/// Select one request-wide WAF without allocating a candidate list.
///
/// Higher configured priority wins. Equal priority prefers the more specific
/// scope; equal priority and scope retains the first declaration.
pub(crate) fn select_waf_owner(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    route: &[Option<Arc<PluginRuntime>>],
) -> Option<WafSelection> {
    let mut winner = None;
    for (scope, runtime) in global
        .iter()
        .map(|runtime| (WafScope::Global, runtime.as_ref()))
        .chain(gateway.iter().map(|runtime| (WafScope::Gateway, runtime.as_ref())))
        .chain(
            route
                .iter()
                .filter_map(Option::as_deref)
                .map(|runtime| (WafScope::Route, runtime)),
        )
    {
        let Some(candidate) = runtime.waf_candidate() else {
            continue;
        };
        let replace = winner.as_ref().is_none_or(|(priority, selected_scope, _, _)| {
            candidate.priority() > *priority
                || (candidate.priority() == *priority && scope as u8 > *selected_scope as u8)
        });
        if replace {
            winner = Some((candidate.priority(), scope, candidate.mode(), candidate.owner()));
        }
    }
    winner.map(|(priority, scope, mode, owner)| {
        let source = match scope {
            WafScope::Global => WafLogSource::Global,
            WafScope::Gateway => WafLogSource::Gateway,
            WafScope::Route => WafLogSource::Route,
        };
        WafSelection::new(owner, source, priority, mode)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{
        EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry,
    };
    use edgion_resources::resources::http_route::HTTPHeaderFilter;
    use kube::core::ObjectMeta;
    use std::collections::HashMap;

    /// Build a minimal EdgionPlugins keyed by `{ns}/{name}`.
    fn make_plugin(ns: &str, name: &str) -> (String, EdgionPlugins) {
        let ep = EdgionPlugins {
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
        (format!("{}/{}", ns, name), ep)
    }

    /// Replace the store contents with the given plugins.
    fn install_plugins(store: &PluginStore, entries: Vec<(&str, &str)>) {
        use crate::plugins::http::compile_plugins;
        let mut map = HashMap::new();
        for (ns, name) in entries {
            let (k, ep) = make_plugin(ns, name);
            map.insert(k, compile_plugins(&ep));
        }
        store.replace_all(map);
    }

    fn waf_runtime(priority: u16) -> Arc<PluginRuntime> {
        use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};

        let mut runtime = PluginRuntime::new();
        runtime
            .add_waf(&WafConfig {
                rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
                mode: WafMode::On,
                priority,
            })
            .expect("test WAF compiles");
        Arc::new(runtime)
    }

    #[test]
    fn waf_selection_uses_priority_scope_and_declaration_order() {
        let global_high = waf_runtime(20);
        let route_low = waf_runtime(10);
        let selected =
            select_waf_owner(&[Arc::clone(&global_high)], &[], &[Some(route_low)]).expect("one WAF is selected");
        assert!(Arc::ptr_eq(
            selected.owner(),
            &global_high.waf_candidate().unwrap().owner()
        ));

        let global_equal = waf_runtime(30);
        let gateway_equal = waf_runtime(30);
        let route_first = waf_runtime(30);
        let route_second = waf_runtime(30);
        let selected = select_waf_owner(
            &[global_equal],
            &[gateway_equal],
            &[Some(Arc::clone(&route_first)), None, Some(route_second)],
        )
        .expect("one WAF is selected");
        assert!(Arc::ptr_eq(
            selected.owner(),
            &route_first.waf_candidate().unwrap().owner()
        ));
        assert!(select_waf_owner(&[], &[], &[]).is_none());
    }

    fn pref(name: &str, ns: Option<&str>) -> PluginReference {
        PluginReference {
            name: name.to_string(),
            namespace: ns.map(|s| s.to_string()),
        }
    }

    #[test]
    fn resolve_global_plugins_empty_when_none() {
        let store = PluginStore::new();
        let resolved = resolve_global_plugins(&store, None);
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_global_plugins_keeps_order_and_drops_dangling() {
        let store = PluginStore::new();
        install_plugins(&store, vec![("ns1", "p1"), ("ns1", "p3")]);
        let p1 = store.get_plugin_runtime("ns1/p1").expect("p1 is installed");
        let p3 = store.get_plugin_runtime("ns1/p3").expect("p3 is installed");

        let refs = vec![
            pref("p1", Some("ns1")),
            pref("p2", Some("ns1")), // dangling
            pref("p3", Some("ns1")),
        ];
        let resolved = resolve_global_plugins(&store, Some(&refs));
        assert_eq!(resolved.len(), 2, "dangling p2 should be dropped");
        assert!(Arc::ptr_eq(&resolved[0], &p1));
        assert!(Arc::ptr_eq(&resolved[1], &p3));
    }

    #[test]
    fn resolve_global_plugins_default_namespace_when_none() {
        let store = PluginStore::new();
        install_plugins(&store, vec![("default", "global1")]);
        let global1 = store
            .get_plugin_runtime("default/global1")
            .expect("global1 is installed");
        let refs = vec![pref("global1", None)];
        let resolved = resolve_global_plugins(&store, Some(&refs));
        assert_eq!(resolved.len(), 1);
        assert!(Arc::ptr_eq(&resolved[0], &global1));
    }

    #[test]
    fn resolve_route_ext_refs_preserves_position_with_none_for_dangling() {
        let store = PluginStore::new();
        install_plugins(&store, vec![("ns1", "p1")]);
        let p1 = store.get_plugin_runtime("ns1/p1").expect("p1 is installed");
        let keys = vec![
            ("ns1".to_string(), "p1".to_string()),
            ("ns1".to_string(), "missing".to_string()),
            ("ns1".to_string(), "p1".to_string()), // duplicate ok
        ];
        let resolved = resolve_route_ext_refs(&store, &keys);
        assert_eq!(resolved.len(), 3);
        assert!(Arc::ptr_eq(resolved[0].as_ref().expect("p1 resolves"), &p1));
        assert!(resolved[1].is_none(), "dangling slot must be None");
        assert!(Arc::ptr_eq(resolved[2].as_ref().expect("duplicate p1 resolves"), &p1));
    }

    #[test]
    fn resolve_route_ext_refs_empty_keys_yields_empty_vec() {
        let store = PluginStore::new();
        let resolved = resolve_route_ext_refs(&store, &[]);
        assert!(resolved.is_empty());
    }
}

#[cfg(test)]
mod gateway_plugins_resolve_tests {
    use super::*;

    #[test]
    fn empty_keys_yield_empty() {
        let store = PluginStore::new();
        assert!(resolve_gateway_plugins(&store, &[]).is_empty());
    }

    #[test]
    fn dangling_keys_dropped() {
        let store = PluginStore::new();
        // A key that does not exist in the empty store is dropped.
        let out = resolve_gateway_plugins(&store, &["nope/missing".to_string()]);
        assert!(out.is_empty());
    }
}
