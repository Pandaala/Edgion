use super::*;

fn check_credential_debug<T>(value: T)
where
    T: Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let wire = serde_json::to_value(&value).unwrap();
    let decoded: T = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
    for debug in [
        format!("{value:?}"),
        format!("{value:#?}"),
        format!("{:?}", Some(&decoded)),
    ] {
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("private-"));
        assert!(!debug.contains("[112, 114, 105"));
    }
}

#[test]
fn credential_containers_hide_debug_but_preserve_transport() {
    use crate::resources::common::{OutboundTlsConfig, SecretSlot, TlsConfig};
    use crate::resources::edgion_plugins::plugin_configs::*;
    use serde_json::json;

    let secret = json!({"stringData": {"private-key": "private-material"}});
    macro_rules! check_config {
        ($ty:ty, $extra:expr) => {{
            let mut wire = serde_json::to_value(<$ty>::default()).unwrap();
            wire.as_object_mut()
                .unwrap()
                .extend($extra.as_object().unwrap().clone());
            let value: $ty = serde_json::from_value(wire).unwrap();
            check_credential_debug(value);
        }};
    }
    let hmac = json!({"secret": b"private-material", "headers": {"private-header": "private-identity"}, "group": "private-group"});
    check_credential_debug(serde_json::from_value::<HmacCredential>(hmac.clone()).unwrap());
    check_config!(HmacMatchConfig, json!({"resolvedCredentials": {"private-user": hmac}}));
    check_config!(
        BasicAuthConfig,
        json!({"resolvedUsers": {"private-user": {"password": "private-password", "group": "private-group"}}})
    );
    let metadata = json!({"headers": {"private-header": "private-identity"}, "group": "private-group"});
    check_credential_debug(serde_json::from_value::<KeyMetadata>(metadata.clone()).unwrap());
    check_config!(KeyAuthConfig, json!({"resolvedKeys": {"private-api-key": metadata}}));
    let jwt = json!({"key": "private-kid", "secret": "private-key", "publicKey": "private-cert"});
    check_credential_debug(serde_json::from_value::<ResolvedJwtCredential>(jwt.clone()).unwrap());
    let issuer = json!({"group": "private-group", "algorithm": "HS256", "credentials": {"private-kid": jwt}});
    check_credential_debug(serde_json::from_value::<ResolvedJwtIssuerGroup>(issuer.clone()).unwrap());
    check_config!(
        JwtAuthConfig,
        json!({"resolvedGroupsByIss": {"private-issuer": issuer}})
    );
    check_config!(
        OpenidConnectConfig,
        json!({"resolvedOidcClientSecret": "private-client", "resolvedSessionSecret": "private-session", "resolvedClientCertificate": secret})
    );
    check_config!(
        LdapAuthConfig,
        json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret, "resolvedBindPassword": "private-bind-password"})
    );
    check_config!(HeaderCertAuthConfig, json!({"resolvedCaSecrets": [secret]}));
    check_config!(CsrfConfig, json!({"resolvedKey": "private-key"}));
    check_credential_debug(ResolvedJweCredential {
        secret: Some("private-key".into()),
    });
    check_config!(
        JweDecryptConfig,
        json!({"resolvedCredential": {"secret": "private-key"}})
    );
    check_config!(
        TlsConfig,
        json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret})
    );
    check_config!(
        OutboundTlsConfig,
        json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret})
    );
    check_config!(WasmFetchSpec, json!({"resolvedAuthHeader": "private-token"}));
    check_config!(WasmOciSpec, json!({"resolvedPullSecret": "private-registry-token"}));
    check_credential_debug(
        serde_json::from_value::<SecretSlot>(json!({"secretRef": {"name": "source"}, "secret": secret})).unwrap(),
    );

    macro_rules! check_wire {
        ($ty:ty, $wire:expr) => {
            check_credential_debug(serde_json::from_value::<$ty>($wire).unwrap());
        };
    }
    check_wire!(
        crate::resources::edgion_plugins::conditions::SecretMatchCondition,
        json!({"key": {"type": "header", "name": "x-token"}, "secretKey": "token", "secretRef": {"name": "source"}, "resolvedValues": ["private-token"]})
    );
    check_wire!(
        crate::resources::edgion_tls::ClientAuthConfig,
        json!({"caSecret": secret, "allowedCns": ["private-identity"]})
    );
    check_wire!(
        crate::resources::edgion_tls::EdgionTlsSpec,
        json!({"hosts": ["example.com"], "secretRef": {"name": "source"}, "secret": secret})
    );
    check_wire!(
        crate::resources::backend_tls_policy::BackendTLSPolicySpec,
        json!({"targetRefs": [], "validation": {"hostname": "example.com"}, "resolvedClientCertificate": secret})
    );
    check_wire!(
        crate::resources::gateway::GatewayBackendTLS,
        json!({"resolvedClientCertificate": secret})
    );
    check_wire!(
        crate::resources::gateway::ResolvedServingCertificateReference,
        json!({"index": 0, "source": {"group": "", "kind": "Secret", "namespace": "default", "name": "source"}, "secret": secret})
    );
    check_wire!(
        crate::resources::gateway::ResolvedFrontendCAReference,
        json!({"index": 0, "source": {"kind": "Secret", "namespace": "default", "name": "source"}, "caCertificate": "cHJpdmF0ZS1jZXJ0"})
    );
    check_wire!(
        crate::resources::edgion_acme::ActiveHttpChallenge,
        json!({"domain": "example.com", "token": "private-token", "keyAuthorization": "private-key", "expireAt": 1})
    );

    // Debug must not depend on the thread-local admin guard, and the guard
    // must still redact the real transport fields independently.
    let cfg = HmacMatchConfig {
        resolved_credentials: Some(std::collections::HashMap::from([(
            "private-user".to_string(),
            HmacCredential {
                secret: b"private-material".to_vec(),
                headers: Default::default(),
                group: "private-group".into(),
            },
        )])),
        ..Default::default()
    };
    assert!(serde_json::to_string(&cfg).unwrap().contains("private-user"));
    let _guard = AdminRedactGuard::new();
    assert_eq!(serde_json::to_value(&cfg).unwrap()["resolvedCredentials"], "[redacted]");
}

#[derive(Serialize)]
struct Demo {
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "redact_opt::serialize")]
    opt_secret: Option<String>,
    #[serde(serialize_with = "redact_required::serialize")]
    bare_secret: String,
    public_field: String,
}

fn demo() -> Demo {
    Demo {
        opt_secret: Some("OPT-SECRET".to_string()),
        bare_secret: "BARE-SECRET".to_string(),
        public_field: "visible".to_string(),
    }
}

#[test]
fn redacts_when_guard_active() {
    let json = {
        let _g = AdminRedactGuard::new();
        serde_json::to_value(demo()).unwrap()
    };
    assert_eq!(json["opt_secret"], "[redacted]");
    assert_eq!(json["bare_secret"], "[redacted]");
    assert_eq!(json["public_field"], "visible");
    let s = json.to_string();
    assert!(!s.contains("OPT-SECRET"), "opt secret leaked: {s}");
    assert!(!s.contains("BARE-SECRET"), "bare secret leaked: {s}");
}

#[test]
fn emits_real_value_without_guard() {
    let json = serde_json::to_value(demo()).unwrap();
    assert_eq!(json["opt_secret"], "OPT-SECRET");
    assert_eq!(json["bare_secret"], "BARE-SECRET");
}

#[test]
fn none_option_is_omitted_when_active() {
    let _g = AdminRedactGuard::new();
    let json = serde_json::to_value(Demo {
        opt_secret: None,
        bare_secret: "x".to_string(),
        public_field: "p".to_string(),
    })
    .unwrap();
    assert!(json.get("opt_secret").is_none(), "None field should be omitted");
}

#[test]
fn guard_clears_on_drop() {
    assert!(!redact_active());
    {
        let _g = AdminRedactGuard::new();
        assert!(redact_active());
    }
    assert!(!redact_active());
}
