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

#[test]
fn waf_selection_retains_only_the_final_winner_after_inputs_are_dropped() {
    let winner = waf_runtime(20);
    let owner = Arc::downgrade(&winner.waf_candidate().unwrap().owner());
    let route = [Some(waf_runtime(1)), None, Some(winner), Some(waf_runtime(20))];
    let selected = select_waf_owner(&[], &[], &route).expect("route WAF is selected");
    drop(route);
    assert!(Arc::ptr_eq(
        selected.owner(),
        &owner.upgrade().expect("selection retains the winning owner")
    ));
    drop(selected);
    assert!(owner.upgrade().is_none());
}

fn waf_runtime_with(
    priority: u16,
    precedence: edgion_resources::resources::edgion_plugins::WafDelegationPrecedence,
) -> Arc<PluginRuntime> {
    use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};

    let mut runtime = PluginRuntime::new();
    runtime
        .add_waf(&WafConfig {
            rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
            mode: WafMode::On,
            priority,
            delegation_precedence: precedence,
            ..WafConfig::default()
        })
        .expect("test WAF compiles");
    Arc::new(runtime)
}

fn parent_waf(priority: u16) -> Arc<PluginRuntime> {
    waf_runtime_with(
        priority,
        edgion_resources::resources::edgion_plugins::WafDelegationPrecedence::Parent,
    )
}

fn owns(selection: &crate::ctx::WafSelection, runtime: &Arc<PluginRuntime>) -> bool {
    Arc::ptr_eq(selection.owner(), &runtime.waf_candidate().unwrap().owner())
}

type Level = Vec<Option<Arc<PluginRuntime>>>;

fn elect(global: &[Arc<PluginRuntime>], gateway: &[Arc<PluginRuntime>], levels: &[Level]) -> crate::ctx::WafSelection {
    select_waf_owner_chain(global, gateway, levels.iter().map(Vec::as_slice)).expect("one WAF is selected")
}

#[test]
fn delegated_waf_default_child_wins_over_parent() {
    let parent = waf_runtime(50);
    let child = waf_runtime(1);
    let selected = elect(
        &[],
        &[],
        &[vec![Some(Arc::clone(&parent))], vec![Some(Arc::clone(&child))]],
    );
    assert!(owns(&selected, &child), "by default the deepest level with a WAF wins");

    // A level without a WAF does not reset the deepest candidate.
    let selected = elect(
        &[],
        &[],
        &[
            vec![Some(Arc::clone(&parent))],
            vec![Some(Arc::clone(&child))],
            vec![None],
        ],
    );
    assert!(owns(&selected, &child));
}

#[test]
fn delegated_waf_parent_precedence_beats_higher_priority_child() {
    let parent = parent_waf(0);
    let child = waf_runtime(u16::MAX);
    let selected = elect(&[], &[], &[vec![Some(Arc::clone(&parent))], vec![Some(child)]]);
    assert!(owns(&selected, &parent), "priority is compared only within one level");
}

#[test]
fn delegated_waf_outermost_parent_precedence_wins() {
    let root = parent_waf(1);
    let mid = parent_waf(100);
    let leaf = parent_waf(200);
    let selected = elect(
        &[],
        &[],
        &[
            vec![Some(Arc::clone(&root))],
            vec![Some(Arc::clone(&mid))],
            vec![Some(leaf)],
        ],
    );
    assert!(owns(&selected, &root));

    // Without a root WAF the next declaring ancestor is the outermost.
    let selected = elect(
        &[],
        &[],
        &[vec![None], vec![Some(Arc::clone(&mid))], vec![Some(waf_runtime(9))]],
    );
    assert!(owns(&selected, &mid));
}

#[test]
fn delegated_waf_level_candidate_is_chosen_by_priority_then_declaration() {
    // Within one level: the higher-priority non-parent candidate is that
    // level's candidate, so the lower-priority parent-precedence WAF does not
    // pin the election to this level.
    let low_parent = parent_waf(1);
    let high_child_pref = waf_runtime(10);
    let leaf = waf_runtime(0);
    let selected = elect(
        &[],
        &[],
        &[
            vec![Some(low_parent), Some(Arc::clone(&high_child_pref))],
            vec![Some(Arc::clone(&leaf))],
        ],
    );
    assert!(owns(&selected, &leaf));

    let first = parent_waf(5);
    let second = parent_waf(5);
    let selected = elect(
        &[],
        &[],
        &[vec![Some(Arc::clone(&first)), Some(second)], vec![Some(leaf)]],
    );
    assert!(owns(&selected, &first), "ties keep the first declaration");
}

#[test]
fn delegated_waf_route_scope_still_beats_gateway_and_global() {
    let global = parent_waf(u16::MAX);
    let gateway = parent_waf(u16::MAX);
    let child = waf_runtime(0);
    let selected = elect(
        &[Arc::clone(&global)],
        &[Arc::clone(&gateway)],
        &[vec![None], vec![Some(Arc::clone(&child))]],
    );
    assert!(owns(&selected, &child));

    // No route-level WAF anywhere on the chain: the flat Gateway/Global election applies.
    let selected = elect(&[Arc::clone(&global)], &[Arc::clone(&gateway)], &[vec![None], vec![]]);
    assert!(owns(&selected, &gateway));
    assert!(select_waf_owner_chain(&[], &[], [&[][..], &[][..]].into_iter()).is_none());
}

#[test]
fn ordinary_route_waf_election_unchanged_by_delegation_precedence() {
    // Ordinary routes use the flat election: precedence is ignored and the
    // higher priority wins among one route's ExtensionRefs.
    let parent_low = parent_waf(1);
    let child_high = waf_runtime(2);
    let selected = select_waf_owner(&[], &[], &[Some(parent_low), Some(Arc::clone(&child_high))]).unwrap();
    assert!(owns(&selected, &child_high));
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
fn runtime_projection_outlives_scope_without_retaining_compiled_resource() {
    let store = PluginStore::new();
    install_plugins(&store, vec![("ns1", "p1")]);
    let scope = store.scope();
    let compiled = scope.http("ns1/p1").unwrap();
    let runtime = scope.http_runtime("ns1/p1").unwrap();
    assert!(Arc::ptr_eq(&runtime, &compiled.plugin_runtime));
    assert!(scope.http_runtime("ns1/missing").is_none());
    let compiled_weak = Arc::downgrade(&compiled);
    let runtime_weak = Arc::downgrade(&runtime);
    drop(compiled);
    drop(scope);
    drop(store);
    assert!(compiled_weak.upgrade().is_none());
    assert_eq!(runtime.request_plugins_count(), 1);
    assert!(runtime_weak.upgrade().is_some());
    drop(runtime);
    assert!(runtime_weak.upgrade().is_none());
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

#[tokio::test]
async fn ambiguous_view_is_present_at_global_gateway_and_rule_resolution() {
    use edgion_common::conf_sync::traits::ConfHandler;
    let store = PluginStore::new();
    let (_, mut bare) = make_plugin("default", "ambiguous");
    bare.metadata.namespace = None;
    let (_, qualified) = make_plugin("default", "ambiguous");
    store.full_set(&HashMap::from([
        ("ambiguous".into(), Arc::new(bare)),
        ("default/ambiguous".into(), Arc::new(qualified)),
    ]));
    let scope = store.scope();
    assert!(resolve_global_plugins(&scope)
        .iter()
        .any(|runtime| runtime.rejects_invalid_config()));
    let gateway = resolve_gateway_plugins(&scope, &["default/missing".into(), "default/ambiguous".into()]);
    assert_eq!(gateway.len(), 1, "ordinary missing drops but collision does not");
    let rule = resolve_route_ext_refs(&scope, &[Some("default/ambiguous".into())]);
    assert!(rule[0].as_ref().unwrap().rejects_invalid_config());
    assert!(gateway[0].rejects_invalid_config());
    // Exercise the same preflight gate used before native or plugin CORS.
    use tokio::io::AsyncWriteExt;
    for runtime in [gateway[0].clone(), rule[0].as_ref().unwrap().clone()] {
        let (mut client, server) = tokio::io::duplex(4096);
        let mut session = pingora_proxy::Session::new_h1(Box::new(server));
        client.write_all(b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n").await.unwrap();
        assert!(session.read_request().await.unwrap());
        let mut ctx = crate::EdgionHttpContext::new();
        runtime.run_preflight_gate(&mut session, &mut ctx).await;
        assert!(ctx.plugin_terminated());
        assert_eq!(session.response_written().unwrap().status.as_u16(), 500);
        assert!(!session
            .response_written()
            .unwrap()
            .headers
            .contains_key("access-control-allow-origin"));
    }
}
