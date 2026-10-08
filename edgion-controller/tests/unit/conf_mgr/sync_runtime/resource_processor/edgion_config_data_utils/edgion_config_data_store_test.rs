use super::*;
use edgion_resources::EdgionPlugins;

#[test]
fn waf_capture_skips_invalid_slots_and_rejects_wrong_policy_owner() {
    let plugin: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins", "metadata":{"name":"owner"},
        "spec":{"waf":{"policyRef":{"name":"policy"},"activeProfileRef":{"name":"selector","namespace":""}}}
    }))
    .unwrap();
    let policy: EdgionConfigData = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionConfigData", "metadata":{"name":"policy","namespace":"default"},
        "spec":{"data":{"type":"WafPolicy","config":{"defaultProfile":"base","profiles":{"base":{"bundleRefs":[{"name":"rules"},{"name":"poison","namespace":"","optional":true}]}}}}}
    })).unwrap();
    let store = EdgionConfigDataStore::new();
    store.replace_all(HashMap::from([
        ("default/policy".into(), policy.clone()),
        ("/poison".into(), policy.clone()),
        ("/selector".into(), policy.clone()),
    ]));
    let fields = WafPluginReferences::from_plugins(&plugin).unwrap();
    let (captured, children) = store.get_waf_policy_snapshot(&fields);
    assert!(captured.is_some());
    assert_eq!(children.len(), 1);
    assert!(children["default/rules"].is_none());
    let mut wrong_owner = policy.clone();
    wrong_owner.metadata.name = Some("different".into());
    store.replace_all(HashMap::from([("default/policy".into(), wrong_owner)]));
    let (captured, children) = store.get_waf_policy_snapshot(&fields);
    assert!(captured.is_none());
    assert!(children.is_empty());
    let mut invalid = plugin;
    invalid.spec.waf.as_mut().unwrap().policy_ref.namespace = Some("".into());
    store.replace_all(HashMap::from([("/policy".into(), policy)]));
    let (captured, children) = store.get_waf_policy_snapshot(&WafPluginReferences::from_plugins(&invalid).unwrap());
    assert!(captured.is_none());
    assert!(children.is_empty());
}
