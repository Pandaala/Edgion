use super::*;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::otlp::OtlpClientConfig;

fn prepare(config: &OtlpClientConfig, resolver: ResolverSpec) -> Result<Arc<PreparedTransport>, &'static str> {
    let owner = serde_json::from_value(serde_json::json!({"metadata":{"name":"otlp","namespace":"test"},
        "spec":{"type":"otlp","config":config}}))
    .unwrap();
    let input = PreparedOtlpTls::with_snapshot(
        Arc::new(owner),
        Arc::new(edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot::from_configs([])),
    )
    .map_err(|_| "invalid_config")?;
    PreparedTransport::prepare(Arc::new(input), resolver)
}

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
    assert!(matches!(prepare(&config, ResolverSpec::System), Err("unresolved_auth")));
    config.auth.as_mut().unwrap().slot.secret = Some(Secret {
        data: Some([(String::from("bearerToken"), ByteString(b"wrong value".to_vec()))].into()),
        ..Default::default()
    });
    assert!(matches!(prepare(&config, ResolverSpec::System), Err("invalid_auth")));
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
    let prepared = prepare(&config, ResolverSpec::System).unwrap();
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
    assert!(prepare(&config, ResolverSpec::System).is_ok());
    config.tls.as_mut().unwrap().enabled = true;
    assert!(matches!(prepare(&config, ResolverSpec::System), Err("invalid_config")));
    config.tls.as_mut().unwrap().outbound = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            hostname: Some("override.example.test".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(prepare(&config, ResolverSpec::System).is_err());
}

impl PreparedTransport {
    pub(in crate::link_sys::providers::otlp) fn assert_retained_input(
        &self,
        input: &Arc<PreparedOtlpTls>,
        actual: &edgion_resources::LinkSys,
    ) {
        assert!(Arc::ptr_eq(&self._input, input));
        assert!(std::ptr::eq(self._input.owner(), actual));
        assert_eq!(self.host.as_ref(), input.endpoint().host);
        assert_eq!(self.port, input.endpoint().port);
        let reconnect = self.endpoint.clone();
        drop(reconnect);
        assert!(Arc::ptr_eq(&self._input, input));
    }
}
