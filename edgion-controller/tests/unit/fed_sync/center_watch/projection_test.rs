use super::*;
use serde_json::json;

fn setting_source() -> EdgionConfigData {
    serde_json::from_value(json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionConfigData",
        "metadata":{"namespace":"apps","name":"regional","uid":"uid-1","resourceVersion":"7",
            "annotations":{"private":"credential-marker"}},
        "spec":{"enable":false,"data":{"type":"ClusterRegionSetting","config":{
            "clusterName":"fleet","regionName":"east","regions":[{
                "name":"east","hashRange":[0,999],"backendEndpoint":"credential-marker:443","tls":true
            }]
        }}}
    }))
    .unwrap()
}

#[test]
fn setting_projection_is_safe_and_disabled_invalid_identities_remain_visible() {
    let mut source = setting_source();
    let row = setting(&source).unwrap().unwrap();
    let encoded = serde_json::to_string(&row).unwrap();
    assert!(!encoded.contains("credential-marker"));
    assert!(!encoded.contains("regions"));
    assert!(!encoded.contains("annotations"));
    let Record::Setting(row) = row else {
        panic!("setting");
    };
    assert!(!row.enabled);
    assert_eq!(row.region_name.as_deref(), Some("east"));
    let ConfigEntry::ClusterRegionSetting(config) = &mut source.spec.data else {
        panic!("setting");
    };
    config.cluster_name = "x".repeat(100_000);
    config.regions.clear();
    let Record::Setting(row) = setting(&source).unwrap().unwrap() else {
        panic!("setting");
    };
    assert!(row.invalid_identity);
    assert!(row.cluster_name.is_none() && row.region_name.is_none());
}

fn plugin_source() -> EdgionPlugins {
    serde_json::from_value(json!({
        "apiVersion":"edgion.io/v1","kind":"EdgionPlugins",
        "metadata":{"namespace":"apps","name":"routes","annotations":{"private":"credential-marker"}},
        "spec":{"requestPlugins":[{"type":"RegionRoute","enable":false,"alias":"routing",
            "config":{"clusterRegionRef":{"name":"regional"},"failOpen":false,"statusOnError":500,
                "keyGet":[{"type":"header","name":"credential-marker"}]}}]}
    }))
    .unwrap()
}

#[test]
fn plugin_projection_only_contains_identifiers_reference_and_failure_policy() {
    let source = plugin_source();
    let record = plugins(&source).unwrap().unwrap();
    let encoded = serde_json::to_string(&record).unwrap();
    assert!(!encoded.contains("credential-marker"));
    assert!(!encoded.contains("keyGet"));
    assert!(record.budget_bytes() >= encoded.len());
    let Record::Plugin(row) = record else {
        panic!("plugin");
    };
    assert_eq!(row.entries.len(), 1);
    let entry = &row.entries[0];
    assert!(!entry.enabled && !entry.fail_open);
    assert_eq!(entry.status_on_error, 500);
    assert_eq!(entry.setting_namespace, "apps");
    assert_eq!(entry.setting_name, "regional");
}

#[test]
fn entry_and_alias_limits_are_projection_failures_without_source_mutation() {
    let mut source = plugin_source();
    let entry = source.spec.request_plugins.as_ref().unwrap()[0].clone();
    source.spec.request_plugins = Some(vec![entry; MAX_ENTRIES + 1]);
    assert_eq!(plugins(&source).unwrap_err(), StoreError::Projection);
    assert_eq!(source.spec.request_plugins.as_ref().unwrap().len(), MAX_ENTRIES + 1);
    source.spec.request_plugins.as_mut().unwrap().truncate(1);
    source.spec.request_plugins.as_mut().unwrap()[0].alias = Some("x".repeat(257));
    assert_eq!(plugins(&source).unwrap_err(), StoreError::Projection);
    source.spec.request_plugins = None;
    assert!(plugins(&source).unwrap().is_none());
}

#[test]
fn changed_setting_type_removes_projection_and_metadata_is_bounded() {
    let mut source = setting_source();
    source.metadata.uid = Some("x".repeat(129));
    assert_eq!(setting(&source).unwrap_err(), StoreError::Projection);
    let mut value = serde_json::to_value(setting_source()).unwrap();
    value["spec"]["data"] = json!({"type":"KeyList","config":{"items":[]}});
    let source: EdgionConfigData = serde_json::from_value(value).unwrap();
    assert!(setting(&source).unwrap().is_none());
}

#[test]
fn shared_near_limit_fixture_exercises_actual_producer_capacity() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/center-watch-near-limit.json")).unwrap();
    let source: EdgionPlugins = serde_json::from_value(fixture["source"].clone()).unwrap();
    let record = plugins(&source).unwrap().unwrap();
    let ledger = record.budget_bytes() + record.key().budget_bytes();
    assert!((63 * 1024..=64 * 1024).contains(&ledger), "actual ledger: {ledger}");
    assert_eq!(serde_json::to_value(record).unwrap(), fixture["list"]["records"][0]);
}
