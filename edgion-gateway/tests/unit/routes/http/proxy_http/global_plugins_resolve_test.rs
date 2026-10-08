use super::*;
use crate::plugins::http::PluginStore;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry};
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
            current_status: None,
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
            request_body: Default::default(),
            ..WafConfig::default()
        })
        .expect("test WAF compiles");
    Arc::new(runtime)
}

#[test]
fn waf_selection_uses_priority_scope_and_declaration_order() {
    let global_high = waf_runtime(20);
    let route_low = waf_runtime(10);
    let selected = select_waf_owner(&[Arc::clone(&global_high)], &[], &[Some(Arc::clone(&route_low))])
        .expect("one WAF is selected");
    assert!(Arc::ptr_eq(
        selected.owner(),
        &route_low.waf_candidate().unwrap().owner()
    ));

    let gateway_low = waf_runtime(1);
    let selected = select_waf_owner(&[global_high], &[Arc::clone(&gateway_low)], &[]).expect("gateway WAF is selected");
    assert!(Arc::ptr_eq(
        selected.owner(),
        &gateway_low.waf_candidate().unwrap().owner()
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

fn install_global_refs(store: &PluginStore, refs: Vec<PluginReference>) -> Arc<PluginScope> {
    let mut config = (**store.scope().config.as_ref().unwrap()).clone();
    config.spec.global_plugins_ref = Some(refs);
    store
        .publisher()
        .replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
    store.scope()
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
    let resolved = resolve_global_plugins(&store.scope());
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
    let resolved = resolve_global_plugins(&install_global_refs(&store, refs));
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
    let resolved = resolve_global_plugins(&install_global_refs(&store, refs));
    assert_eq!(resolved.len(), 1);
    assert!(Arc::ptr_eq(&resolved[0], &global1));
}

#[test]
fn resolve_route_ext_refs_preserves_position_with_none_for_dangling() {
    let store = PluginStore::new();
    install_plugins(&store, vec![("ns1", "p1")]);
    let p1 = store.get_plugin_runtime("ns1/p1").expect("p1 is installed");
    let keys = vec![
        Some("ns1/p1".to_string()),
        Some("ns1/missing".to_string()),
        Some("ns1/p1".to_string()), // duplicate ok
    ];
    let resolved = resolve_route_ext_refs(&store.scope(), &keys);
    assert_eq!(resolved.len(), 3);
    assert!(Arc::ptr_eq(resolved[0].as_ref().expect("p1 resolves"), &p1));
    assert!(resolved[1].is_none(), "dangling slot must be None");
    assert!(Arc::ptr_eq(resolved[2].as_ref().expect("duplicate p1 resolves"), &p1));
}

#[test]
fn resolve_route_ext_refs_empty_keys_yields_empty_vec() {
    let store = PluginStore::new();
    let resolved = resolve_route_ext_refs(&store.scope(), &[]);
    assert!(resolved.is_empty());
}

#[test]
fn invalid_slots_make_zero_queries_and_preserve_valid_missing_and_duplicate_positions() {
    let target = Arc::new(PluginRuntime::new());
    let keys = vec![
        None,
        Some("default/missing".into()),
        Some("default/hit".into()),
        None,
        Some("default/hit".into()),
    ];
    let mut reads = Vec::new();
    let resolved = resolve_route_ext_refs_with(&keys, |key| {
        reads.push(key.to_owned());
        (key == "default/hit").then(|| Arc::clone(&target))
    });
    assert_eq!(reads, ["default/missing", "default/hit", "default/hit"]);
    assert_eq!(resolved.len(), 5);
    assert!(resolved[0].is_none() && resolved[1].is_none() && resolved[3].is_none());
    assert!(Arc::ptr_eq(resolved[2].as_ref().unwrap(), &target));
    assert!(Arc::ptr_eq(resolved[4].as_ref().unwrap(), &target));
}
