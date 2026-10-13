use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
use edgion_resources::resources::edgion_plugins::plugin_configs::{RequestAccessPolicyConfig, UrlAccessCandidate};
use edgion_resources::resources::edgion_plugins::{
    AnyOfClause, ConditionSet, ConditionValidationProfile, RequestRestrictionConfig,
};

#[test]
fn accepts_each_bounded_dnf_shape() {
    let all_of: ConditionSet = serde_yaml::from_str("allOf:\n  - type: probability\n    ratio: 1.0\n").unwrap();
    assert_eq!(all_of.atomic_count(), 1);

    let any_of: ConditionSet = serde_yaml::from_str(
        r#"
anyOf:
  - type: probability
    ratio: 1.0
  - name: authenticated-production
    allOf:
      - type: probability
        ratio: 0.5
      - type: keyExist
        key: { type: header, name: X-User }
"#,
    )
    .unwrap();
    assert_eq!(any_of.atomic_count(), 3);
    let ConditionSet::AnyOf { any_of } = any_of else {
        panic!("expected anyOf")
    };
    assert!(matches!(any_of[0], AnyOfClause::Condition(_)));
    assert!(matches!(any_of[1], AnyOfClause::AllOf(_)));
}

#[test]
fn json_yaml_and_schemars_expose_the_same_literal_shapes() {
    for document in [
        r#"{"allOf":[{"type":"probability","ratio":1.0}]}"#,
        r#"{"anyOf":[{"type":"probability","ratio":1.0},{"name":"pair","allOf":[{"type":"probability","ratio":0.5}]}]}"#,
    ] {
        let from_json: ConditionSet = serde_json::from_str(document).unwrap();
        let yaml = serde_yaml::to_string(&from_json).unwrap();
        let from_yaml: ConditionSet = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(from_json.atomic_count(), from_yaml.atomic_count());
    }

    let schema = serde_json::to_value(schemars::schema_for!(ConditionSet)).unwrap();
    let rendered = schema.to_string();
    assert!(rendered.contains("\"allOf\""), "{rendered}");
    assert!(rendered.contains("\"anyOf\""), "{rendered}");
    assert!(rendered.contains("\"name\""), "{rendered}");
}

#[test]
fn structurally_rejects_ambiguous_unknown_and_recursive_shapes() {
    for yaml in [
        "allOf: []\nanyOf: []\n",
        "unknown: []\n",
        "anyOf:\n  - type: probability\n    ratio: 1.0\n    allOf: []\n",
        "anyOf:\n  - allOf: []\n    unknown: true\n",
        "allOf:\n  - anyOf: []\n",
        "anyOf:\n  - anyOf: []\n",
        "anyOf:\n  - allOf:\n      - allOf: []\n",
    ] {
        assert!(serde_yaml::from_str::<ConditionSet>(yaml).is_err(), "accepted: {yaml}");
    }

    let legacy = r#"
run:
  - type: probability
    ratio: 1.0
"#;
    assert!(serde_yaml::from_str::<PluginConditions>(legacy).is_err());
}

#[test]
fn semantic_validation_rejects_empty_and_blank_groups() {
    let empty: ConditionSet = serde_yaml::from_str("allOf: []\n").unwrap();
    assert!(empty.validate(ConditionValidationProfile::EntrySync).is_err());

    let blank: ConditionSet = serde_yaml::from_str("anyOf:\n  - name: '  '\n    allOf: []\n").unwrap();
    let errors = blank.validate(ConditionValidationProfile::EntrySync).unwrap_err();
    assert!(
        errors.iter().any(|error| error.contains("name cannot be empty")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|error| error.contains("allOf must be non-empty")),
        "{errors:?}"
    );
}

#[test]
fn walker_and_profiles_cover_direct_and_nested_atoms() {
    let yaml = r#"
anyOf:
  - type: directPeerIpMatch
    ipRefs: [{name: private-networks}]
  - allOf:
      - type: hmacAuth
        secretGroups:
          - name: default
            secretRefs:
              - name: hmac-credentials
        algorithms: [hmac-sha256]
        validateRequestBody: true
      - type: remoteIpMatch
        ipRefs: [{name: peer-networks}]
"#;
    let mut set: ConditionSet = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(set.atomic_count(), 3);
    assert_eq!(set.ip_match_count(), 2);
    assert!(set.validates_request_body());
    assert!(set.validate(ConditionValidationProfile::EntrySync).is_err());
    assert!(set.validate(ConditionValidationProfile::RequestAsync).is_ok());
    set.compile().unwrap();
    assert!(set
        .iter_conditions()
        .all(|condition| !condition.descriptor().is_empty()));
}

#[test]
fn consumer_containers_reject_retired_fields() {
    let old_rr = r#"
mode: Deny
anyOf:
  - allOf:
      - type: probability
        ratio: 1.0
"#;
    assert!(serde_yaml::from_str::<RequestRestrictionConfig>(old_rr).is_err());

    let new_rr = r#"
mode: Deny
conditions:
  allOf:
    - type: probability
      ratio: 1.0
"#;
    let mut rr: RequestRestrictionConfig = serde_yaml::from_str(new_rr).unwrap();
    rr.validate().unwrap();

    let old_rap = r#"
defaultProfile: default
profiles:
  default:
    requiredRuleGroups:
      - name: clients
        anyOfRules:
          - name: internal
            type: directPeerIp
            config:
              ipRefs: [{name: private-networks}]
              allOf:
                - type: probability
                  ratio: 1.0
"#;
    let invalid_rap: RequestAccessPolicyConfig = serde_yaml::from_str(old_rap).unwrap();
    assert!(edgion_resources::resources::edgion_plugins::PluginValidation::validation_error(&invalid_rap).is_some());

    let mixed_rr = r#"
mode: Deny
anyOf: []
conditions:
  allOf:
    - type: probability
      ratio: 1.0
"#;
    assert!(serde_yaml::from_str::<RequestRestrictionConfig>(mixed_rr).is_err());

    let mixed_candidate = r#"
name: health
hosts: [api.example.com]
paths:
  - type: Exact
    value: /health
allOf: []
conditions:
  allOf:
    - type: probability
      ratio: 1.0
"#;
    assert!(serde_yaml::from_str::<UrlAccessCandidate>(mixed_candidate).is_err());
}

#[test]
fn validation_profiles_apply_to_direct_and_nested_placements() {
    for yaml in [
        r#"
anyOf:
  - type: hmacAuth
    secretGroups:
      - name: default
        secretRefs:
          - name: hmac-credentials
    algorithms: [hmac-sha256]
    validateRequestBody: true
"#,
        r#"
anyOf:
  - allOf:
      - type: hmacAuth
        secretGroups:
          - name: default
            secretRefs:
              - name: hmac-credentials
        algorithms: [hmac-sha256]
        validateRequestBody: true
"#,
    ] {
        let set: ConditionSet = serde_yaml::from_str(yaml).unwrap();
        assert!(set.validate(ConditionValidationProfile::EntrySync).is_err());
        assert!(set.validate(ConditionValidationProfile::RequestAsync).is_ok());
    }
}
