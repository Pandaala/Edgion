use super::*;
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
use edgion_common::conf_sync::traits::ConfHandler;
use serde_json::json;
use std::collections::HashMap;

fn owner() -> edgion_resources::EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"geo","namespace":"default"},"spec":{"requestPlugins":[{"type":"GeoIpLocation","config":{"defaultAction":"deny","failOpen":true,"denyRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"country","values":["RU"]},{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"blocked"}]}]}],"allowRuleGroups":[{"rules":[{"source":"RemoteIp","key":"cidr","ipRefs":[{"name":"clients"},{"name":"other"}]}]}]}}]}})).unwrap()
}
fn compiled(owner: &edgion_resources::EdgionPlugins) -> CompiledGeoIpRefs {
    let compiler = IpConditionCompiler::new(owner).unwrap();
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::GeoIpLocation(config) =
        &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
    else {
        unreachable!()
    };
    CompiledGeoIpRefs::from_http(config, &compiler).unwrap()
}
fn data(name: &str, cidrs: &[&str], enabled: bool, visibility: &str) -> Arc<edgion_resources::EdgionConfigData> {
    Arc::new(serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},"spec":{"enable":enabled,"visibility":visibility,"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":cidrs}]}}}})).unwrap())
}
fn publish(store: &EdgionConfigDataStore, clients: Arc<edgion_resources::EdgionConfigData>) {
    store.full_set(&HashMap::from([
        (
            "default/blocked".into(),
            data("blocked", &["10.0.0.0/8"], true, "Namespace"),
        ),
        ("default/clients".into(), clients),
        ("default/other".into(), data("other", &[], true, "Namespace")),
    ]));
}

#[test]
fn actual_owner_retained_grid_tracks_content_and_holds_one_generation_across_sources() {
    let owner = owner();
    let grid = Arc::new(compiled(&owner));
    let held_grid = grid.clone();
    let store = EdgionConfigDataStore::new();
    assert!(!grid.available_in(&store.capture_snapshot()));
    publish(&store, data("clients", &["192.0.2.0/24"], true, "Namespace"));
    let held = store.capture_snapshot();
    assert!(grid.available_in(&held));
    assert!(grid.contains_in(&held, GeoRuleSide::Allow, 0, 0, "192.0.2.1".parse().unwrap()));
    assert!(grid.contains_in(&held, GeoRuleSide::Deny, 0, 1, "10.1.2.3".parse().unwrap()));
    publish(&store, data("clients", &["198.51.100.0/24"], true, "Namespace"));
    assert!(Arc::ptr_eq(&grid, &held_grid));
    assert!(grid.contains_in(&held, GeoRuleSide::Allow, 0, 0, "192.0.2.1".parse().unwrap()));
    assert!(!grid.contains_in(
        &store.capture_snapshot(),
        GeoRuleSide::Allow,
        0,
        0,
        "192.0.2.1".parse().unwrap()
    ));
    store.full_set(&HashMap::new());
    assert!(!grid.available_in(&store.capture_snapshot()));
    publish(&store, data("clients", &["192.0.2.0/24"], true, "Namespace"));
    assert!(grid.available_in(&store.capture_snapshot()));
}

#[test]
fn all_refs_are_required_even_when_an_earlier_rule_or_union_member_matches() {
    let owner = owner();
    let grid = compiled(&owner);
    let store = EdgionConfigDataStore::new();
    publish(&store, data("clients", &["192.0.2.0/24"], true, "Namespace"));
    store.partial_update(
        Default::default(),
        Default::default(),
        std::collections::HashSet::from(["default/other".into()]),
    );
    let view = store.capture_snapshot();
    assert!(!grid.available_in(&view));
    assert!(
        grid.contains_in(&view, GeoRuleSide::Allow, 0, 0, "192.0.2.1".parse().unwrap()),
        "membership alone is not admission; completeness runs first"
    );
}

#[test]
fn central_partial_good_empty_and_identity_states_are_distinct() {
    let owner = owner();
    let grid = compiled(&owner);
    let store = EdgionConfigDataStore::new();
    publish(
        &store,
        data("clients", &["invalid-cidr", "192.0.2.0/24"], true, "Namespace"),
    );
    assert!(grid.available_in(&store.capture_snapshot()));
    assert!(grid.contains_in(
        &store.capture_snapshot(),
        GeoRuleSide::Allow,
        0,
        0,
        "192.0.2.1".parse().unwrap()
    ));
    publish(&store, data("clients", &["192.0.2.1"], true, "Namespace"));
    assert!(grid.contains_in(
        &store.capture_snapshot(),
        GeoRuleSide::Allow,
        0,
        0,
        "192.0.2.1".parse().unwrap()
    ));
    publish(&store, data("clients", &["invalid-cidr"], true, "Namespace"));
    assert!(grid.available_in(&store.capture_snapshot()));
    assert!(!grid.contains_in(
        &store.capture_snapshot(),
        GeoRuleSide::Allow,
        0,
        0,
        "192.0.2.1".parse().unwrap()
    ));
    publish(&store, data("clients", &[], false, "Namespace"));
    assert!(!grid.available_in(&store.capture_snapshot()));
    publish(&store, data("wrong-name", &["192.0.2.0/24"], true, "Namespace"));
    assert!(!grid.available_in(&store.capture_snapshot()));
    let wrong = Arc::new(
        serde_json::from_value(
            json!({"metadata":{"name":"clients","namespace":"default"},"spec":{"data":{"type":"Misc","config":{}}}}),
        )
        .unwrap(),
    );
    publish(&store, wrong);
    assert!(!grid.available_in(&store.capture_snapshot()));
}

#[test]
fn unbound_copy_and_empty_authored_criterion_cannot_become_an_open_runtime() {
    let owner = owner();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::GeoIpLocation(config) =
        &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
    else {
        unreachable!()
    };
    assert!(CompiledGeoIpRefs::from_http(&config.clone(), &compiler).is_err());
    let mut empty = config.clone();
    empty.allow_rule_groups[0].rules[0].cidr.ip_refs.clear();
    assert!(empty.check_fields(false).is_some());
    let grid = CompiledGeoIpRefs::from_http(config, &compiler).unwrap();
    let mut runtime_copy = config.clone();
    runtime_copy.validate_and_init(false).unwrap();
    CompiledGeoIpRefs::sanitize(&mut runtime_copy);
    assert!(runtime_copy.allow_rule_groups[0].rules[0].cidr.ip_refs.is_empty());
    assert!(
        runtime_copy.check_fields(false).is_some(),
        "sanitized copies are never readmitted or used to mint authority"
    );
    let store = EdgionConfigDataStore::new();
    publish(&store, data("clients", &["192.0.2.0/24"], true, "Namespace"));
    let view = store.capture_snapshot();
    let ctx = edgion_resources::resources::edgion_plugins::plugin_configs::GeoRuleContext {
        remote_ip: Some("192.0.2.1".parse().unwrap()),
        ..Default::default()
    };
    assert_eq!(
        runtime_copy.decide(&ctx, |side, group, rule, ip| grid
            .contains_in(&view, side, group, rule, ip)),
        edgion_resources::resources::edgion_plugins::plugin_configs::GeoDecision::Allow
    );
}

#[test]
fn geo_cross_namespace_proofs_check_current_visibility_and_disabled_validation_keeps_identity_checks() {
    use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
    use edgion_resources::resources::edgion_plugins::ip_reference_set::IpReadScope;
    for policy in [
        None,
        Some(IpReadScope::ClusterOnly),
        Some(IpReadScope::GrantAuthorized),
        Some(IpReadScope::ValidationDisabled),
    ] {
        let mut owner = owner();
        let edgion_resources::resources::edgion_plugins::EdgionPlugin::GeoIpLocation(config) =
            &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
        else {
            unreachable!()
        };
        for rule in config
            .deny_rule_groups
            .iter_mut()
            .chain(&mut config.allow_rule_groups)
            .flat_map(|group| &mut group.rules)
        {
            for reference in &mut rule.cidr.ip_refs {
                reference.namespace = Some("ops".into());
            }
        }
        if let Some(policy) = policy {
            let prepared = PreparedIpListReferences::from_plugins(&owner);
            prepared
                .visit_geo_plugins_mut(&mut owner, |mut leaf| {
                    if leaf.is_supported() {
                        leaf.set_read_scopes(&vec![policy; leaf.config().ip_refs.len()])
                            .unwrap();
                    }
                })
                .unwrap();
        }
        let grid = compiled(&owner);
        let store = EdgionConfigDataStore::new();
        let generation = |visibility: &str, wrong_identity: bool| {
            ["blocked", "clients", "other"]
                .into_iter()
                .map(|name| {
                    let mut value = data(name, &["192.0.2.0/24"], true, visibility);
                    Arc::make_mut(&mut value).metadata.namespace =
                        Some(if wrong_identity { "wrong" } else { "ops" }.into());
                    (format!("ops/{name}"), value)
                })
                .collect::<HashMap<_, _>>()
        };
        store.full_set(&generation("Cluster", false));
        let held = store.capture_snapshot();
        assert!(grid.available_in(&held));
        store.full_set(&generation("Namespace", false));
        assert_eq!(
            grid.available_in(&store.capture_snapshot()),
            matches!(
                policy,
                Some(IpReadScope::GrantAuthorized | IpReadScope::ValidationDisabled)
            )
        );
        assert!(grid.available_in(&held), "held public generation remains consistent");
        store.full_set(&generation("Cluster", true));
        assert!(
            !grid.available_in(&store.capture_snapshot()),
            "validation flag never skips identity"
        );
    }
}
