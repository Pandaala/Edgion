use super::*;

#[test]
fn date_pattern_accepts_defaults_literals_and_utc_formats() {
    let mut config = ElasticsearchClientConfig::default();
    assert_eq!(config.validate_date_pattern(), Ok(()));
    config.index = Some(EsIndex {
        prefix: None,
        date_pattern: None,
    });
    assert_eq!(config.validate_date_pattern(), Ok(()));
    for pattern in [
        "",
        "literal",
        "%%",
        "%Y.%m.%d",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Z %z %s %+",
    ] {
        config.index.as_mut().unwrap().date_pattern = Some(pattern.into());
        assert_eq!(config.validate_date_pattern(), Ok(()), "{pattern}");
    }
}

#[test]
fn date_pattern_rejects_malformed_and_parsing_only_directives_without_echoing_input() {
    for pattern in ["%", "private-literal%", "%Q", "%#z"] {
        let config = ElasticsearchClientConfig {
            index: Some(EsIndex {
                prefix: None,
                date_pattern: Some(pattern.into()),
            }),
            ..Default::default()
        };
        assert_eq!(
            config.validate_date_pattern(),
            Err("Elasticsearch index.datePattern must be a strftime pattern that supports UTC formatting"),
            "{pattern}"
        );
    }
}

#[test]
fn test_serde_simple_config() {
    let yaml = r#"
endpoints:
  - "http://127.0.0.1:9200"
"#;
    let config: ElasticsearchClientConfig = yaml_serde::from_str(yaml).unwrap();
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
    let config: ElasticsearchClientConfig = yaml_serde::from_str(yaml).unwrap();
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
    let config: ElasticsearchClientConfig = yaml_serde::from_str(yaml).unwrap();
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
    let config: ElasticsearchClientConfig = yaml_serde::from_str(yaml).unwrap();
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
    let r: Result<ElasticsearchAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_password() {
    let yaml = "type: basic\nsecretRef:\n  name: s\npassword: leaked";
    let r: Result<ElasticsearchAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_api_key() {
    let yaml = "type: apiKey\nsecretRef:\n  name: s\napiKeyId: leaked";
    let r: Result<ElasticsearchAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline apiKeyId must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_rejects_inline_bearer_token() {
    let yaml = "type: bearer\nsecretRef:\n  name: s\nbearerToken: leaked";
    let r: Result<ElasticsearchAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline bearerToken must be rejected by deny_unknown_fields");
}

#[test]
fn es_auth_basic_yaml_roundtrip() {
    let yaml = "type: basic\nsecretRef:\n  name: es-creds";
    let a: super::ElasticsearchAuth = yaml_serde::from_str(yaml).unwrap();
    match a {
        super::ElasticsearchAuth::Basic { slot } => assert_eq!(slot.secret_ref.name, "es-creds"),
        _ => panic!("expected Basic"),
    }
}

#[test]
fn es_auth_apikey_yaml_roundtrip() {
    let yaml = "type: apiKey\nsecretRef:\n  name: es-creds";
    let a: super::ElasticsearchAuth = yaml_serde::from_str(yaml).unwrap();
    assert!(matches!(a, super::ElasticsearchAuth::ApiKey { .. }));
}

#[test]
fn es_auth_rejects_unknown_field() {
    let yaml = "type: basic\nsecretRef:\n  name: x\nbogus: 1";
    assert!(yaml_serde::from_str::<super::ElasticsearchAuth>(yaml).is_err());
}

#[test]
fn bulk_response_limit_is_serialized_and_preserved_in_crd_config() {
    let absent: EsBulk = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.max_response_bytes, None);
    assert!(serde_json::to_value(absent).unwrap().get("maxResponseBytes").is_none());
    let config: ElasticsearchClientConfig =
        yaml_serde::from_str("endpoints: [http://es.example:9200]\nbulk:\n  maxResponseBytes: 2097152\n").unwrap();
    assert_eq!(config.bulk.as_ref().unwrap().max_response_bytes, Some(2097152));
    let wire = serde_json::to_value(&config).unwrap();
    assert_eq!(wire["bulk"]["maxResponseBytes"], 2097152);
    let roundtrip: ElasticsearchClientConfig = serde_json::from_value(wire).unwrap();
    assert_eq!(roundtrip.bulk.unwrap().max_response_bytes, Some(2097152));
    let crd: yaml_serde::Value =
        yaml_serde::from_str(include_str!("../../../../../config/crd/edgion-crd/link_sys_crd.yaml")).unwrap();
    for version in crd["spec"]["versions"].as_sequence().unwrap() {
        if version["served"].as_bool() == Some(true) {
            assert_eq!(
                version["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["config"]
                    ["x-kubernetes-preserve-unknown-fields"]
                    .as_bool(),
                Some(true)
            );
        }
    }
}
