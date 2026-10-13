use super::*;
use crate::plugins::runtime::conditions::ip_refs::IpConditionCompiler;
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::{EdgionPlugin, EdgionPlugins};
use serde_json::json;
use std::collections::HashMap;

fn owner(allow: bool) -> EdgionPlugins {
    let refs = if allow {
        json!({"allowRefs":[{"name":"clients"},{"name":"missing"}],"denyRefs":[{"name":"blocked"}]})
    } else {
        json!({"denyRefs":[{"name":"blocked"}]})
    };
    serde_json::from_value(json!({"metadata":{"name":"acl","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"IpRestriction","config":refs}]}}))
    .unwrap()
}
fn plan(owner: &EdgionPlugins) -> Arc<CompiledIpAcl> {
    let compiler = IpConditionCompiler::new(owner).unwrap();
    let EdgionPlugin::IpRestriction(config) = &owner.spec.request_plugins.as_ref().unwrap()[0].plugin else {
        unreachable!()
    };
    compiler.bind_acl(&config.refs).unwrap()
}
fn data(name: &str, cidr: &str, group: &str) -> Arc<edgion_resources::EdgionConfigData> {
    Arc::new(
        serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},
        "spec":{"data":{"type":"IpList","config":{"items":[{"name":group,"cidrs":[cidr]}]}}}}))
        .unwrap(),
    )
}
#[test]
fn live_acl_plan_keeps_omission_policy_and_one_snapshot() {
    let owner = owner(true);
    let plan = plan(&owner);
    let store = EdgionConfigDataStore::new();
    let ip = Some("192.0.2.1".parse().unwrap());
    assert!(matches!(
        plan.evaluate_in(None, ip),
        AccessDecision::DenyByWhitelistMiss
    ));
    store.full_set(&HashMap::from([(
        "default/clients".into(),
        data("clients", "192.0.2.0/24", "clients"),
    )]));
    let held = store.capture_snapshot();
    assert!(matches!(
        plan.evaluate_in(Some(&held), ip),
        AccessDecision::AllowByGroup(_)
    ));
    store.full_set(&HashMap::from([(
        "default/blocked".into(),
        data("blocked", "192.0.2.1/32", "blocked"),
    )]));
    assert!(matches!(
        plan.evaluate_in(Some(&held), ip),
        AccessDecision::AllowByGroup(_)
    ));
    let fresh = store.capture_snapshot();
    assert!(matches!(plan.evaluate_in(Some(&fresh), ip),AccessDecision::DenyByGroup(ref name) if &**name == "blocked"));
    store.full_set(&HashMap::new());
    assert!(matches!(
        plan.evaluate_in(Some(&store.capture_snapshot()), ip),
        AccessDecision::DenyByWhitelistMiss
    ));
}
#[test]
fn deny_only_missing_lists_preserve_default_allow() {
    let owner = owner(false);
    let plan = plan(&owner);
    assert!(matches!(
        plan.evaluate_in(None, Some("192.0.2.1".parse().unwrap())),
        AccessDecision::Default(DefaultAction::Allow)
    ));
}

#[test]
fn acl_authority_filtered_lazy_union_preserves_group_arc_and_short_circuit() {
    use std::cell::Cell;
    let owner:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"acl","namespace":"default"},"spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"first"},{"name":"later"}]}}]}})).unwrap();
    let plan = plan(&owner);
    let store = EdgionConfigDataStore::new();
    let first=Arc::new(serde_json::from_value(json!({"metadata":{"name":"first","namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"broad","cidrs":["192.0.2.0/24"]},{"name":"inner","cidrs":["192.0.2.0/25"]}]}}}})).unwrap());
    store.full_set(&HashMap::from([
        ("default/first".into(), first),
        ("default/later".into(), data("later", "192.0.2.1/32", "later")),
    ]));
    let view = store.capture_snapshot();
    let ip = "192.0.2.1".parse().unwrap();
    let expected = view
        .ip_list_matchers(&plan.allow)
        .next()
        .unwrap()
        .matched_group_arc(&ip)
        .unwrap();
    assert_eq!(&*expected, "inner");
    let consumed = Cell::new(0);
    let allow = view
        .ip_list_matchers(&plan.allow)
        .inspect(|_| consumed.set(consumed.get() + 1))
        .chain(std::iter::once_with(|| {
            panic!("a decision must not consume later sources")
        }));
    let AccessDecision::AllowByGroup(actual) = evaluate_ip_access_multi([], allow, true, &DefaultAction::Allow, &ip)
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(&expected, &actual));
    assert_eq!(consumed.get(), 1);
    let deny = view.ip_list_matchers(&plan.allow);
    let allow = std::iter::once_with(|| panic!("deny priority must not consume allow sources"));
    assert!(
        matches!(evaluate_ip_access_multi(deny,allow,true,&DefaultAction::Allow,&ip),AccessDecision::DenyByGroup(ref actual) if Arc::ptr_eq(&expected,actual))
    );
    for ip in ["192.0.2.1", "::ffff:192.0.2.1"] {
        assert!(
            matches!(plan.evaluate_in(Some(&view),Some(ip.parse().unwrap())),AccessDecision::AllowByGroup(ref actual) if Arc::ptr_eq(&expected,actual))
        );
    }
}

#[test]
fn acl_retained_instance_observes_content_and_current_visibility() {
    use edgion_resources::refs::ip_list_fields::{IpListSide, PreparedIpListReferences};
    use edgion_resources::resources::edgion_plugins::ip_reference_set::IpReadScope;
    let mut owner:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"acl","namespace":"default"},"spec":{"requestPlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"clients","namespace":"other"}]}}]}})).unwrap();
    let plan_before = plan(&owner);
    let store = EdgionConfigDataStore::new();
    let ip = Some("192.0.2.1".parse().unwrap());
    let publish = |visibility: &str, enable: bool, name: &str, kind: &str| {
        let value=serde_json::from_value(json!({"metadata":{"name":name,"namespace":"other"},"spec":{"visibility":visibility,"enable":enable,"data":{"type":kind,"config":{"items":[{"name":"clients","cidrs":["192.0.2.0/24"]}]}}}})).unwrap();
        store.full_set(&HashMap::from([("other/clients".into(), Arc::new(value))]));
    };
    publish("Cluster", true, "clients", "IpList");
    let held = store.capture_snapshot();
    assert!(matches!(
        plan_before.evaluate_in(Some(&held), ip),
        AccessDecision::AllowByGroup(_)
    ));
    publish("Namespace", true, "clients", "IpList");
    assert!(matches!(
        plan_before.evaluate_in(Some(&store.capture_snapshot()), ip),
        AccessDecision::DenyByWhitelistMiss
    ));
    assert!(matches!(
        plan_before.evaluate_in(Some(&held), ip),
        AccessDecision::AllowByGroup(_)
    ));
    let prepared = PreparedIpListReferences::from_plugins(&owner);
    prepared
        .visit_acl_plugins_mut(&mut owner, |mut refs| {
            refs.set_read_scopes(IpListSide::Allow, &[IpReadScope::GrantAuthorized])
                .unwrap()
        })
        .unwrap();
    let granted = plan(&owner);
    assert!(matches!(
        granted.evaluate_in(Some(&store.capture_snapshot()), ip),
        AccessDecision::AllowByGroup(_)
    ));
    for (visibility, enabled, name, kind) in [
        ("Cluster", false, "clients", "IpList"),
        ("Cluster", true, "wrong", "IpList"),
        ("Cluster", true, "clients", "Misc"),
    ] {
        publish(visibility, enabled, name, kind);
        assert!(matches!(
            granted.evaluate_in(Some(&store.capture_snapshot()), ip),
            AccessDecision::DenyByWhitelistMiss
        ));
    }
    store.full_set(&HashMap::new());
    assert!(matches!(
        granted.evaluate_in(Some(&store.capture_snapshot()), ip),
        AccessDecision::DenyByWhitelistMiss
    ));
    publish("Namespace", true, "clients", "IpList");
    assert!(matches!(
        granted.evaluate_in(Some(&store.capture_snapshot()), ip),
        AccessDecision::AllowByGroup(_)
    ));
}
