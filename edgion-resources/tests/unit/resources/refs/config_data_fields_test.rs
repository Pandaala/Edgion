use super::*;
use serde_json::json;

fn http(kind: &str, field: &str, reference: serde_json::Value) -> EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{"requestPlugins":[{
        "type":kind,"enable":false,"config":role_config(kind,field,reference)
    }]}}))
    .unwrap()
}

#[test]
fn finite_actual_fields_default_namespace_and_separate_owner_failures() {
    for (kind, field, role) in [
        ("RequestAccessPolicy", "activeProfileRef", ConfigDataRole::Selector),
        ("Canary", "activeProfileRef", ConfigDataRole::Selector),
        ("RegionRoute", "overrideRef", ConfigDataRole::RegionOverride),
    ] {
        let owner = http(kind, field, json!({"name":"target"}));
        let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
        let loan = entry.field().unwrap();
        assert_eq!(loan.role(), role);
        assert_eq!(loan.key().unwrap(), "default/target");
        assert_eq!(loan.query().unwrap().source().group_kind().kind(), "EdgionPlugins");
        for reference in [
            json!({"name":"bad/name"}),
            json!({"name":""}),
            json!({"name":"target","namespace":""}),
        ] {
            let owner = http(kind, field, reference);
            let loan = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap().field().unwrap();
            assert!(loan.validate_owner().is_ok());
            assert!(loan.query().is_err());
        }
        let mut owner = owner;
        owner.metadata.namespace = Some(String::new());
        assert!(HttpConfigDataEntry::from_plugins(&owner, 0)
            .unwrap()
            .field()
            .unwrap()
            .validate_owner()
            .is_err());
    }
}

#[test]
fn stream_selector_and_authored_url_fields_keep_distinct_roles_and_sources() {
    let owner: EdgionStreamPlugins = serde_json::from_value(json!({"metadata":{"name":"stream"},"spec":{"plugins":[{
        "type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfileRef":{"name":"selector"},"activeProfile":"base","profiles":{"base":{}}}
    }]}})).unwrap();
    let loan = StreamConfigDataEntry::from_stream_plugins(&owner, 0)
        .unwrap()
        .field()
        .unwrap();
    assert_eq!(loan.role(), ConfigDataRole::Selector);
    assert_eq!(loan.key().unwrap(), "default/selector");
    assert_eq!(
        loan.query().unwrap().source().group_kind().kind(),
        "EdgionStreamPlugins"
    );
    let owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"http","namespace":"client"},"spec":{"requestPlugins":[{
        "type":"RequestAccessPolicy","config":{"defaultProfile":"base","profiles":{"base":{"requiredRuleGroups":[{"name":"urls","anyOfRules":[{
            "name":"url","type":"url","config":{"configRefs":[{"name":"document","namespace":"shared"}]}
        }]}]}}}
    }]}})).unwrap();
    let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
    let loan = entry.url_field("base", 0, 0, 0).unwrap();
    assert_eq!(loan.role(), ConfigDataRole::UrlAllowList);
    assert_eq!(loan.key().unwrap(), "shared/document");
    assert_eq!(loan.query().unwrap().source().scope().namespace(), Some("client"));
    assert!(entry.url_field("base", 0, 0, 1).is_none());
}

fn role_config(kind: &str, field: &str, reference: serde_json::Value) -> serde_json::Value {
    let mut config = match kind {
        "RequestAccessPolicy" => json!({"defaultProfile":"base","profiles":{"base":{"requiredRuleGroups":[]}}}),
        "Canary" => json!({"enable":true,"activeProfile":"base","profiles":{"base":{"name":"backend"}}}),
        _ => json!({}),
    };
    config[field] = reference;
    config
}
