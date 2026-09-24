use super::*;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, SecretSlot};
use edgion_resources::resources::link_sys::otlp::OtlpAuth;
use k8s_openapi::{api::core::v1::Secret, ByteString};

fn config() -> OtlpClientConfig {
    OtlpClientConfig {
        endpoint: "https://collector.example.test:4317".into(),
        timeout_ms: 1000,
        auth: None,
        tls: None,
    }
}

#[test]
fn auth_unresolved_invalid_and_resolved_are_distinct_safe_states() {
    let mut config = config();
    config.auth = Some(OtlpAuth {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                name: "auth".into(),
                namespace: None,
                group: None,
                kind: None,
            },
            secret: None,
        },
    });
    assert!(matches!(
        PreparedTransport::prepare(&config, ResolverSpec::System),
        Err("unresolved_auth")
    ));
    config.auth.as_mut().unwrap().slot.secret = Some(Secret {
        data: Some([(String::from("bearerToken"), ByteString(b"wrong value".to_vec()))].into()),
        ..Default::default()
    });
    assert!(matches!(
        PreparedTransport::prepare(&config, ResolverSpec::System),
        Err("invalid_auth")
    ));
    config
        .auth
        .as_mut()
        .unwrap()
        .slot
        .secret
        .as_mut()
        .unwrap()
        .data
        .as_mut()
        .unwrap()
        .insert("bearerToken".into(), ByteString(b"valid-token".to_vec()));
    let prepared = PreparedTransport::prepare(&config, ResolverSpec::System).unwrap();
    assert_eq!(prepared.metadata.get("authorization").unwrap(), "Bearer valid-token");
    assert!(prepared.metadata.get("authorization").unwrap().is_sensitive());
}

#[test]
fn inactive_tls_is_inert_and_unsupported_effective_identity_fails() {
    let mut config = config();
    config.tls = Some(TlsConfig {
        enabled: false,
        outbound: OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        },
        ..Default::default()
    });
    assert!(PreparedTransport::prepare(&config, ResolverSpec::System).is_ok());
    config.tls.as_mut().unwrap().enabled = true;
    assert!(matches!(
        PreparedTransport::prepare(&config, ResolverSpec::System),
        Err("invalid_config")
    ));
    config.tls.as_mut().unwrap().outbound = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            hostname: Some("override.example.test".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(PreparedTransport::prepare(&config, ResolverSpec::System).is_err());
}
