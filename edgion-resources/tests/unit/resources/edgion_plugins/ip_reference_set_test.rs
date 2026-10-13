use super::*;
use crate::resources::edgion_plugins::conditions::IpAccessMatch;
use crate::resources::edgion_plugins::plugin_configs::IpAccessRuleConfig;
use serde_json::json;

#[test]
fn rejected_values_are_discarded_and_invalidity_survives_transport() {
    for field in ["ips", "resolvedIps", "unknownIpField"] {
        let mut value = json!({"ipRefs":[{"name":"trusted"}], "invalidInput":false});
        value[field] = json!(["192.0.2.0/24"]);
        let config: IpAccessMatch = serde_json::from_value(value).unwrap();
        assert!(config.refs.invalid_input);
        assert!(config.refs.validate().is_err());
        let transported = serde_json::to_value(&config).unwrap();
        assert!(transported.get(field).is_none());
        assert_eq!(transported["invalidInput"], true);
        let receiver: IpAccessMatch = serde_json::from_value(transported).unwrap();
        assert!(receiver.refs.invalid_input);
    }
}

#[test]
fn primary_negate_presence_is_invalid_including_false_and_null() {
    for negate in [json!(false), json!(true), json!(null)] {
        let config: IpAccessRuleConfig = serde_json::from_value(json!({
            "ipRefs":[{"name":"trusted"}], "negate":negate
        }))
        .unwrap();
        assert!(config.refs.invalid_input);
        assert!(serde_json::to_value(config).unwrap().get("negate").is_none());
    }
}

#[test]
fn valid_flattened_primary_and_secondary_negation_remain_distinct() {
    let primary: IpAccessRuleConfig = serde_json::from_value(json!({
        "ipRefs":[{"name":"trusted"}],
        "conditions":{"allOf":[{"type":"remoteIpMatch","ipRefs":[{"name":"excluded"}],"negate":true}]}
    }))
    .unwrap();
    assert!(!primary.refs.invalid_input);
    assert!(primary.refs.validate().is_ok());
    let secondary: IpAccessMatch =
        serde_json::from_value(json!({"ipRefs":[{"name":"trusted"}],"negate":true})).unwrap();
    assert!(!secondary.refs.invalid_input);
    assert!(secondary.negate);
}

#[test]
fn explicit_invalid_marker_only_denies_and_clean_generation_recovers() {
    let invalid: IpReferenceSet =
        serde_json::from_value(json!({"ipRefs":[{"name":"trusted"}],"invalidInput":true})).unwrap();
    assert!(invalid.validate().is_err());
    let clean: IpReferenceSet = serde_json::from_value(json!({"ipRefs":[{"name":"trusted"}]})).unwrap();
    assert!(clean.validate().is_ok());
}

#[test]
fn read_proof_domain_is_required_transport_metadata_and_never_operator_schema() {
    let wire = serde_json::json!({"source":{"group":"edgion.io","kind":"EdgionPlugins","namespace":"apps","name":"owner","uid":null,"generation":null},"domain":"ipList","root":0,"leaf":null,"slots":[{"namespace":"apps","name":"networks","scope":"sameNamespace"}]});
    let proof: IpReadResolution = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(proof.domain, IpReadDomain::IpList);
    assert_eq!(serde_json::to_value(proof).unwrap(), wire);
    let mut missing = wire;
    missing.as_object_mut().unwrap().remove("domain");
    assert!(serde_json::from_value::<IpReadResolution>(missing).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(IpReferenceSet)).unwrap();
    assert!(schema["properties"].get("resolvedRefs").is_none());
    assert!(schema["properties"].get("invalidInput").is_none());
}
