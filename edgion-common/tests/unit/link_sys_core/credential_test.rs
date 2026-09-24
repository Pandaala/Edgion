use super::*;
use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, TlsConfig as CommonTlsConfig};
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
use edgion_resources::resources::link_sys::LinkSysSpec;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;

fn mk_secret(data: &[(&str, &[u8])]) -> Secret {
    let mut bmap = BTreeMap::new();
    for (k, v) in data {
        bmap.insert(k.to_string(), ByteString(v.to_vec()));
    }
    Secret {
        data: Some(bmap),
        ..Default::default()
    }
}

fn mk_redis_linksys(auth: Option<RedisAuth>) -> LinkSys {
    LinkSys {
        metadata: ObjectMeta {
            name: Some("r".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://localhost:6379".into()],
                auth,
                db: None,
                timeout: None,
                pool: None,
                topology: None,
                tls: None,
            }),
        },
        status: None,
    }
}

fn mk_redis_auth(secret: Option<Secret>) -> RedisAuth {
    use edgion_resources::resources::common::SecretSlot;
    RedisAuth {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            },
            secret,
        },
    }
}

// ------------------------------------------------------------
// CredentialState helpers
// ------------------------------------------------------------

#[test]
fn is_buildable_accepts_not_required_and_resolved() {
    let not_required: CredentialState<BasicAuthMaterial> = CredentialState::NotRequired;
    let resolved = CredentialState::Resolved(BasicAuthMaterial {
        username: None,
        password: "p".into(),
    });
    let pending: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
    let invalid: CredentialState<BasicAuthMaterial> =
        CredentialState::Invalid(InvalidReason::ParseFailed("boom".into()));

    assert!(not_required.is_buildable());
    assert!(resolved.is_buildable());
    assert!(!pending.is_buildable());
    assert!(!invalid.is_buildable());
}

#[test]
fn debug_impl_redacts_resolved_material() {
    let s = CredentialState::Resolved(BasicAuthMaterial {
        username: Some("alice".into()),
        password: "hunter2".into(),
    });
    let rendered = format!("{s:?}");
    assert!(rendered.contains("Resolved"), "got {rendered}");
    assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
    assert!(!rendered.contains("alice"), "username leaked: {rendered}");
}

#[test]
fn kind_returns_variant_label() {
    let pending: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
    assert_eq!(pending.kind(), "Pending");
    let resolved = CredentialState::Resolved(BasicAuthMaterial {
        username: None,
        password: "p".into(),
    });
    assert_eq!(resolved.kind(), "Resolved");
}

// ------------------------------------------------------------
// basic_state — 4 variants
// ------------------------------------------------------------

#[test]
fn basic_state_not_required_when_no_auth() {
    let ls = mk_redis_linksys(None);
    assert!(matches!(basic_state(&ls), CredentialState::NotRequired));
}

#[test]
fn basic_state_pending_when_secret_missing() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(None)));
    assert!(matches!(basic_state(&ls), CredentialState::Pending));
}

#[test]
fn basic_state_resolved_when_secret_present() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[
        ("username", b"u"),
        ("password", b"p"),
    ])))));
    let state = basic_state(&ls);
    let m = match state {
        CredentialState::Resolved(m) => m,
        other => panic!("expected Resolved, got {other:?}"),
    };
    assert_eq!(m.username.as_deref(), Some("u"));
    assert_eq!(m.password, "p");
}

#[test]
fn basic_state_invalid_when_secret_malformed() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("wrong_key", b"x")])))));
    match basic_state(&ls) {
        CredentialState::Invalid(InvalidReason::ParseFailed(msg)) => {
            assert!(msg.contains("missing") || msg.contains("Missing") || msg.contains("password"));
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// When a LinkSys with embedded secret is JSON-round-tripped (simulating
/// conf_sync), Gateway can still materialise.
#[test]
fn basic_state_survives_conf_sync_roundtrip() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("password", b"hunter2")])))));
    let json = serde_json::to_string(&ls).unwrap();
    let ls2: LinkSys = serde_json::from_str(&json).unwrap();
    match basic_state(&ls2) {
        CredentialState::Resolved(m) => assert_eq!(m.password, "hunter2"),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

// ------------------------------------------------------------
// resolved_state — ES variants
// ------------------------------------------------------------

fn mk_es_linksys(auth: Option<ElasticsearchAuth>) -> LinkSys {
    use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
    LinkSys {
        metadata: ObjectMeta {
            name: Some("es".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                auth,
                ..Default::default()
            }),
        },
        status: None,
    }
}

#[test]
fn resolved_state_es_basic_resolved() {
    use edgion_resources::resources::common::SecretSlot;
    let ls = mk_es_linksys(Some(ElasticsearchAuth::Basic {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            },
            secret: Some(mk_secret(&[("username", b"u"), ("password", b"p")])),
        },
    }));
    let got = resolved_state(&ls);
    assert!(matches!(got, CredentialState::Resolved(ResolvedCredential::Basic(_))));
}

#[test]
fn resolved_state_es_api_key_pending() {
    use edgion_resources::resources::common::SecretSlot;
    let ls = mk_es_linksys(Some(ElasticsearchAuth::ApiKey {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            },
            secret: None,
        },
    }));
    assert!(matches!(resolved_state(&ls), CredentialState::Pending));
}

#[test]
fn resolved_state_es_bearer_invalid_on_missing_key() {
    use edgion_resources::resources::common::SecretSlot;
    let ls = mk_es_linksys(Some(ElasticsearchAuth::Bearer {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            },
            secret: Some(mk_secret(&[("nope", b"x")])),
        },
    }));
    assert!(matches!(
        resolved_state(&ls),
        CredentialState::Invalid(InvalidReason::ParseFailed(_))
    ));
}

#[test]
fn resolved_state_no_auth_is_not_required() {
    let ls = mk_es_linksys(None);
    assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
}

// ------------------------------------------------------------
// Webhook has no provider-level credential slot. Request credentials are
// supplied through resolved request templates instead.
// ------------------------------------------------------------

fn mk_webhook_linksys() -> LinkSys {
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };
    LinkSys {
        metadata: ObjectMeta {
            name: Some("wh".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            kind: None,
                            name: Some("webhook-svc".into()),
                            namespace: Some("default".into()),
                            port: Some(8080),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        status: None,
    }
}

#[test]
fn resolved_state_webhook_no_auth_is_not_required() {
    let ls = mk_webhook_linksys();
    assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
}

// ------------------------------------------------------------
// DR-4: tls_ready must NOT fail open for the webhook-shaped
// providers (Webhook / HttpDns) whose `connection.tls` declares
// an unresolved client-certificate ref. Before the explicit arms
// these fell through `_ => None` and reported ready == true.
// ------------------------------------------------------------

fn tls_with_unresolved_client_cert() -> edgion_resources::resources::common::TlsConfig {
    serde_json::from_str(r#"{"enabled":true,"clientCertificateRef":{"name":"id"}}"#).unwrap()
}

#[test]
fn tls_ready_disabled_references_are_inert_for_every_provider() {
    use serde_json::json;

    for (provider, mut config, tls_path) in [
        ("redis", json!({"endpoints": ["rediss://localhost:6380"]}), "/tls"),
        ("etcd", json!({"endpoints": ["https://localhost:2379"]}), "/tls"),
        ("otlp", json!({"endpoint": "https://collector"}), "/tls"),
        (
            "elasticsearch",
            json!({"endpoints": ["https://localhost:9200"]}),
            "/tls",
        ),
        ("kafka", json!({"brokers": ["localhost:9092"]}), "/tls"),
        ("webhook", json!({"target": {"url": "https://example.com"}}), "/tls"),
        (
            "httpdns",
            json!({"preset": "aliyun", "connection": {}}),
            "/connection/tls",
        ),
        (
            "credentialSource",
            json!({"provider": {"type": "oauth2ClientCredentials", "tokenEndpoint": "https://example.com/token", "clientAuthentication": {"activeSecretRef": {"name": "bootstrap"}}}}),
            "/provider/tls",
        ),
    ] {
        for tls in [
            json!({"enabled": false, "validation": {"caCertificateRefs": [{"kind": "Secret", "name": "missing"}]}}),
            json!({"enabled": false, "clientCertificateRef": {"name": "missing"}}),
        ] {
            match tls_path {
                "/connection/tls" => config["connection"]["tls"] = tls,
                "/provider/tls" => config["provider"]["tls"] = tls,
                _ => config["tls"] = tls,
            }
            for enabled in [false, true, false] {
                config.pointer_mut(tls_path).unwrap()["enabled"] = json!(enabled);
                let spec = serde_json::from_value(json!({"type": provider, "config": config})).unwrap();
                let ls = LinkSys::new("tls-readiness", spec);
                assert_eq!(tls_ready(&ls), !enabled, "{provider}, enabled={enabled}");
            }
        }
    }
}

fn mk_httpdns_linksys(
    connection: Option<edgion_resources::resources::link_sys::httpdns::HttpDnsConnection>,
) -> LinkSys {
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };
    LinkSys {
        metadata: ObjectMeta {
            name: Some("dns".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::HttpDns(HttpDnsResolverConfig {
                preset: Some(HttpDnsPreset::Aliyun),
                url_template: None,
                response: HttpDnsResponseFormat::default(),
                fallback: HttpDnsFallback::System,
                connection,
            }),
        },
        status: None,
    }
}

#[test]
fn tls_ready_httpdns_false_when_client_cert_unresolved() {
    use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
    let ls = mk_httpdns_linksys(Some(HttpDnsConnection {
        tls: Some(tls_with_unresolved_client_cert()),
        timeout_ms: None,
    }));
    assert!(!tls_ready(&ls), "httpdns connection.tls readiness must not fail open");
}

#[test]
fn tls_ready_httpdns_true_when_no_tls() {
    let ls = mk_httpdns_linksys(None);
    assert!(tls_ready(&ls), "no connection.tls means nothing to wait for");
}

#[test]
fn tls_ready_webhook_false_when_client_cert_unresolved() {
    let mut ls = mk_webhook_linksys();
    if let SystemConfig::Webhook(w) = &mut ls.spec.config {
        w.conn.tls = Some(tls_with_unresolved_client_cert());
    }
    assert!(!tls_ready(&ls), "webhook conn.tls readiness must not fail open");
}

/// The exact gate expression used by both Webhook dispatch arms in
/// `runtime::store`. Locks "client is registered iff the credential is
/// buildable".
fn webhook_dispatch_gate(ls: &LinkSys) -> bool {
    resolved_state(ls).is_buildable()
}

#[test]
fn webhook_dispatch_gate_open_without_auth() {
    assert!(webhook_dispatch_gate(&mk_webhook_linksys()));
}

// ------------------------------------------------------------
// Dispatch-gate composition: mirrors the exact expression used in
// `runtime::store::dispatch_*` so these tests lock-in the invariant
// "client is built iff auth.is_buildable() && tls_ready()".
// ------------------------------------------------------------

fn dispatch_gate(ls: &LinkSys) -> bool {
    basic_state(ls).is_buildable() && tls_ready(ls)
}

#[test]
fn dispatch_gate_without_auth_is_open() {
    let ls = mk_redis_linksys(None);
    assert!(dispatch_gate(&ls));
}

#[test]
fn dispatch_gate_blocks_pending_auth() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(None)));
    assert!(!dispatch_gate(&ls));
}

#[test]
fn dispatch_gate_opens_with_resolved_auth() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("password", b"p")])))));
    assert!(dispatch_gate(&ls));
}

#[test]
fn dispatch_gate_blocks_invalid_auth() {
    let ls = mk_redis_linksys(Some(mk_redis_auth(Some(mk_secret(&[("wrong", b"x")])))));
    assert!(!dispatch_gate(&ls));
}

// ------------------------------------------------------------
// require_basic_material helpers
// ------------------------------------------------------------

#[test]
fn require_basic_resolved_returns_some() {
    let m = BasicAuthMaterial {
        username: Some("u".into()),
        password: "p".into(),
    };
    let state = CredentialState::Resolved(m);
    let got = require_basic_material(&state, "Redis").unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().username.as_deref(), Some("u"));
}

#[test]
fn require_basic_pending_bails() {
    let state: CredentialState<BasicAuthMaterial> = CredentialState::Pending;
    let err = require_basic_material(&state, "Redis").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Redis"), "msg should include provider: {msg}");
    assert!(msg.contains("has not been resolved yet"), "msg: {msg}");
}

#[test]
fn require_basic_invalid_bails_with_reason() {
    let state: CredentialState<BasicAuthMaterial> =
        CredentialState::Invalid(InvalidReason::ParseFailed("missing key".into()));
    let err = require_basic_material(&state, "Etcd").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Etcd credential material is invalid"), "msg: {msg}");
    assert!(msg.contains("missing key"), "msg: {msg}");
}

#[test]
fn require_basic_not_required_returns_none() {
    let state: CredentialState<BasicAuthMaterial> = CredentialState::NotRequired;
    let got = require_basic_material(&state, "Redis").unwrap();
    assert!(got.is_none());
}

#[test]
fn dispatch_gate_blocks_tls_ref_without_secret() {
    let ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("r".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["rediss://localhost:6379".into()],
                auth: None,
                db: None,
                timeout: None,
                pool: None,
                topology: None,
                tls: Some(CommonTlsConfig {
                    enabled: true,
                    // CA certificate refs declared but not yet resolved
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                group: String::new(),
                                kind: "Secret".into(),
                                name: "tls".into(),
                                namespace: None,
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    resolved_ca_certificates: None,
                    resolved_client_certificate: None,
                }),
            }),
        },
        status: None,
    };
    assert!(!dispatch_gate(&ls));
}

// ------------------------------------------------------------
// resolved_state — Kafka SASL variants
// ------------------------------------------------------------

fn mk_kafka_linksys(sasl: Option<edgion_resources::resources::link_sys::kafka::KafkaSaslConfig>) -> LinkSys {
    use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;
    LinkSys {
        metadata: kube::api::ObjectMeta {
            name: Some("kafka".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: edgion_resources::resources::link_sys::LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Kafka(KafkaClientConfig {
                brokers: vec!["broker1:9092".to_string()],
                sasl,
                tls: None,
                channel_size: None,
                max_topics: None,
                max_pending_records: None,
                max_pending_bytes: None,
                linger_ms: None,
            }),
        },
        status: None,
    }
}

#[test]
fn resolved_state_kafka_no_sasl_is_not_required() {
    let ls = mk_kafka_linksys(None);
    assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
}

#[test]
fn resolved_state_kafka_sasl_pending_when_secret_missing() {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
    let ls = mk_kafka_linksys(Some(KafkaSaslConfig {
        username: "user".to_string(),
        password: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "kafka-secret".into(),
                namespace: None,
            },
            secret: None,
        },
    }));
    assert!(matches!(resolved_state(&ls), CredentialState::Pending));
}

#[test]
fn resolved_state_kafka_sasl_resolved_when_secret_present() {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::kafka::KafkaSaslConfig;
    let ls = mk_kafka_linksys(Some(KafkaSaslConfig {
        username: "user".to_string(),
        password: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "kafka-secret".into(),
                namespace: None,
            },
            secret: Some(mk_secret(&[("password", b"kafka-pass")])),
        },
    }));
    assert!(matches!(
        resolved_state(&ls),
        CredentialState::Resolved(ResolvedCredential::Basic(_))
    ));
}

#[test]
fn tls_ready_kafka_no_tls_is_true() {
    let ls = mk_kafka_linksys(None);
    assert!(tls_ready(&ls));
}
#[test]
fn otlp_credentials_cover_every_readiness_state() {
    let mut ls: LinkSys = serde_json::from_value(serde_json::json!({"metadata":{"namespace":"default","name":"otlp"},"spec":{"type":"otlp","config":{"endpoint":"http://collector"}}})).unwrap();
    assert!(matches!(resolved_state(&ls), CredentialState::NotRequired));
    if let SystemConfig::Otlp(config) = &mut ls.spec.config {
        config.auth = Some(serde_json::from_value(serde_json::json!({"secretRef":{"name":"auth"}})).unwrap());
    }
    assert!(matches!(resolved_state(&ls), CredentialState::Pending));
    for (token, valid) in [
        ("valid-token".to_owned(), true),
        ("private bad token".to_owned(), false),
        ("a".repeat(8193), false),
    ] {
        if let SystemConfig::Otlp(config) = &mut ls.spec.config {
            config.auth.as_mut().unwrap().slot.secret = Some(serde_json::from_value(serde_json::json!({"data":{"bearerToken": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, token.as_bytes())}})).unwrap());
        }
        let state = resolved_state(&ls);
        assert_eq!(state.is_buildable(), valid);
        assert!(!format!("{state:?}").contains(&token));
    }
}
