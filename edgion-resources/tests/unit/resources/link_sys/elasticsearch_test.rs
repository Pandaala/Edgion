use super::*;

#[test]
fn test_serde_simple_config() {
    let yaml = r#"
endpoints:
  - "http://127.0.0.1:9200"
"#;
    let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.endpoints.len(), 1);
    assert!(config.auth.is_none());
}

#[test]
fn test_serde_full_config() {
    let yaml = r#"
endpoints:
  - "https://es-node-1:9200"
  - "https://es-node-2:9200"
auth:
  type: basic
  secretRef:
    name: es-creds
tls:
  enabled: true
timeout:
  connect: 5000
  request: 30000
pool:
  maxIdlePerHost: 10
  idleTimeout: 90000
bulk:
  batchSize: 500
  flushInterval: 5000
  maxRetries: 3
  backoffMs: 1000
  maxBodyBytes: 10485760
  channelSize: 20000
index:
  prefix: "edgion-logs"
  datePattern: "%Y.%m.%d"
"#;
    let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.endpoints.len(), 2);
    match config.auth.as_ref().unwrap() {
        ElasticsearchAuth::Basic { slot } => {
            assert_eq!(slot.secret_ref.name, "es-creds");
        }
        _ => panic!("Expected Basic auth"),
    }
    assert!(config.tls.as_ref().unwrap().enabled);
    assert_eq!(config.timeout.as_ref().unwrap().connect, Some(5000));
    assert_eq!(config.bulk.as_ref().unwrap().batch_size, Some(500));
    assert_eq!(config.bulk.as_ref().unwrap().channel_size, Some(20_000));
    assert_eq!(config.index.as_ref().unwrap().prefix.as_deref(), Some("edgion-logs"));
}

#[test]
fn test_serde_api_key_auth() {
    let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  type: apiKey
  secretRef:
    name: es-api-key
"#;
    let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
    match config.auth.unwrap() {
        ElasticsearchAuth::ApiKey { slot } => {
            assert_eq!(slot.secret_ref.name, "es-api-key");
        }
        _ => panic!("Expected ApiKey auth"),
    }
}

#[test]
fn test_serde_bearer_auth() {
    let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  type: bearer
  secretRef:
    name: es-bearer-token
"#;
    let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
    match config.auth.unwrap() {
        ElasticsearchAuth::Bearer { slot } => {
            assert_eq!(slot.secret_ref.name, "es-bearer-token");
        }
        _ => panic!("Expected Bearer auth"),
    }
}

#[test]
fn test_serde_roundtrip_json() {
    use crate::resources::common::{SecretObjectReference, SecretSlot};
    let config = ElasticsearchClientConfig {
        endpoints: vec!["http://localhost:9200".to_string()],
        auth: Some(ElasticsearchAuth::Basic {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "es-creds".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let _: ElasticsearchClientConfig = serde_json::from_str(&json).unwrap();
}

#[test]
fn es_auth_rejects_inline_username() {
    let yaml = "type: basic\nsecretRef:\n  name: s\nusername: leaked";
    let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_password() {
    let yaml = "type: basic\nsecretRef:\n  name: s\npassword: leaked";
    let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_api_key() {
    let yaml = "type: apiKey\nsecretRef:\n  name: s\napiKeyId: leaked";
    let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline apiKeyId must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_bearer_token() {
    let yaml = "type: bearer\nsecretRef:\n  name: s\nbearerToken: leaked";
    let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline bearerToken must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_basic_yaml_roundtrip() {
    let yaml = "type: basic\nsecretRef:\n  name: es-creds";
    let a: super::ElasticsearchAuth = serde_yaml::from_str(yaml).unwrap();
    match a {
        super::ElasticsearchAuth::Basic { slot } => assert_eq!(slot.secret_ref.name, "es-creds"),
        _ => panic!("expected Basic"),
    }
}

#[test]
fn es_auth_apikey_yaml_roundtrip() {
    let yaml = "type: apiKey\nsecretRef:\n  name: es-creds";
    let a: super::ElasticsearchAuth = serde_yaml::from_str(yaml).unwrap();
    assert!(matches!(a, super::ElasticsearchAuth::ApiKey { .. }));
}

#[test]
fn es_auth_rejects_unknown_field() {
    let yaml = "type: basic\nsecretRef:\n  name: x\nbogus: 1";
    assert!(serde_yaml::from_str::<super::ElasticsearchAuth>(yaml).is_err());
}
