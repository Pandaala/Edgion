use super::*;

fn valid_yaml() -> &'static str {
    r#"
provider:
  type: oauth2ClientCredentials
  tokenEndpoint: https://auth.example.com/oauth2/token
  clientAuthentication:
    activeSecretRef:
      name: oauth-client
  scopes: [inference.invoke]
"#
}

#[test]
fn minimal_config_parses_with_secure_defaults() {
    let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).expect("valid config must parse");
    assert_eq!(config.rotation, CredentialRotationConfig::default());
    assert!(config.egress.block_private);
    let CredentialSourceProvider::OAuth2ClientCredentials(provider) = &config.provider;
    assert_eq!(
        provider.client_authentication.method,
        OAuth2ClientAuthenticationMethod::ClientSecretBasic
    );
    assert!(config.validation_errors().is_empty());
}

#[test]
fn rejects_unsafe_endpoint_invalid_scopes_refs_and_rotation() {
    let yaml = r#"
provider:
  type: oauth2ClientCredentials
  tokenEndpoint: http://user:pass@auth.example.com/token?tenant=x#fragment
  clientAuthentication:
    activeSecretRef: {group: edgion.io, kind: LinkSys, name: source}
    previousSecretRef: {group: edgion.io, kind: LinkSys, name: source}
  scopes: [valid, "bad scope", valid]
rotation:
  interval: 1s
  requestTimeout: 61s
  retryInitialBackoff: 1m
  retryMaxBackoff: 1s
"#;
    let config: CredentialSourceConfig = serde_yaml::from_str(yaml).expect("structurally valid config must parse");
    let errors = config.validation_errors().join("; ");
    for expected in [
        "must use https",
        "group",
        "duplicates",
        "rotation.interval",
        "must not exceed",
    ] {
        assert!(errors.contains(expected), "missing {expected:?} in {errors}");
    }
}

#[test]
fn provider_and_system_discriminators_round_trip_exactly() {
    let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).expect("valid config must parse");
    let system = super::super::SystemConfig::CredentialSource(config);
    let value = serde_json::to_value(system).expect("system config must serialize");
    assert_eq!(value["type"], "credentialSource");
    assert_eq!(value["config"]["provider"]["type"], "oauth2ClientCredentials");
}

#[test]
fn rejects_unknown_fields() {
    let yaml = format!("{}unknownField: ignored\n", valid_yaml());
    assert!(serde_yaml::from_str::<CredentialSourceConfig>(&yaml).is_err());
}

#[test]
fn omitted_publication_defaults_to_persist_on_with_ten_thousand_key_cap() {
    let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).unwrap();
    assert!(config.publication.persist);
    assert_eq!(config.publication.memory_max_keys, 10_000);
    assert!(config.validation_errors().is_empty());
}

#[test]
fn persist_false_is_accepted_and_cap_bounds_are_enforced() {
    let ok: CredentialSourceConfig =
        serde_yaml::from_str(&(valid_yaml().to_string() + "publication:\n  persist: false\n  memoryMaxKeys: 1\n"))
            .unwrap();
    assert!(!ok.publication.persist);
    assert_eq!(ok.publication.memory_max_keys, 1);

    let high: CredentialSourceConfig =
        serde_yaml::from_str(&(valid_yaml().to_string() + "publication:\n  persist: false\n  memoryMaxKeys: 10001\n"))
            .unwrap();
    assert!(high.validation_errors().join(";").contains("memoryMaxKeys"));

    let zero: CredentialSourceConfig =
        serde_yaml::from_str(&(valid_yaml().to_string() + "publication:\n  persist: true\n  memoryMaxKeys: 0\n"))
            .unwrap();
    assert!(zero.validation_errors().join(";").contains("memoryMaxKeys"));
}

#[test]
fn publication_map_is_not_a_config_field() {
    let config: CredentialSourceConfig = serde_yaml::from_str(valid_yaml()).unwrap();
    let json = serde_json::to_value(&config).unwrap();
    assert!(json.get("generated").is_none());
    assert!(json.get("entries").is_none());
    assert!(json.get("memoryKeys").is_none());
}
