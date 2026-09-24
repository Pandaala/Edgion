use super::*;
use crate::redact::AdminRedactGuard;

fn config(target: CredentialInjectionTarget) -> CredentialInjectorConfig {
    CredentialInjectorConfig {
        credential_ref: SecretObjectReference {
            group: None,
            kind: None,
            name: "provider-token".to_string(),
            namespace: None,
        },
        target,
        resolved_credential: None,
    }
}

#[test]
fn omitted_target_defaults_to_bearer_and_round_trips_pascal_case() {
    let parsed: CredentialInjectorConfig = serde_yaml::from_str("credentialRef:\n  name: provider-token\n").unwrap();
    assert_eq!(parsed.target, CredentialInjectionTarget::Bearer);

    let value = serde_json::to_value(parsed).unwrap();
    assert_eq!(value["target"]["type"], "Bearer");
}

#[test]
fn header_target_uses_camel_case_variant_fields() {
    let parsed: CredentialInjectorConfig = serde_yaml::from_str(
            "credentialRef:\n  name: provider-token\ntarget:\n  type: Header\n  name: X-Provider-Token\n  valuePrefix: 'Token '\n",
        )
        .unwrap();
    assert_eq!(
        parsed.target,
        CredentialInjectionTarget::Header {
            name: "X-Provider-Token".to_string(),
            value_prefix: "Token ".to_string(),
        }
    );
    assert!(parsed.validation_error().is_none());
}

#[test]
fn rejects_invalid_refs_headers_prefixes_and_reserved_targets() {
    let mut cfg = config(CredentialInjectionTarget::Bearer);
    cfg.credential_ref.group = Some("example.io".to_string());
    assert!(cfg.validation_error().unwrap().contains("group"));

    for name in ["bad header", "Content-Length", "X-Forwarded-Proto", "Cookie"] {
        let cfg = config(CredentialInjectionTarget::Header {
            name: name.to_string(),
            value_prefix: String::new(),
        });
        assert!(cfg.validation_error().is_some(), "{name} must be rejected");
    }

    for name in RESERVED_TARGET_HEADER_NAMES {
        let cfg = config(CredentialInjectionTarget::Header {
            name: (*name).to_string(),
            value_prefix: String::new(),
        });
        assert!(cfg.validation_error().is_some(), "{name} must be rejected");
    }

    let authorization = config(CredentialInjectionTarget::Header {
        name: "Authorization".to_string(),
        value_prefix: "Bearer ".to_string(),
    });
    assert!(authorization.validation_error().is_none());

    let cfg = config(CredentialInjectionTarget::Header {
        name: "X-Token".to_string(),
        value_prefix: "\r\n".to_string(),
    });
    assert!(cfg.validation_error().unwrap().contains("header value"));
}

#[test]
fn resolved_value_is_schema_hidden_redacted_in_debug_and_admin_output() {
    let secret = "private-credential-value";
    let mut cfg = config(CredentialInjectionTarget::Bearer);
    cfg.resolved_credential = Some(ResolvedCredentialInjector {
        value: secret.to_string(),
        valid_until: Some("2026-08-27T12:00:00Z".to_string()),
    });

    let debug = format!("{:?}", cfg.resolved_credential.as_ref().unwrap());
    assert!(!debug.contains(secret));
    assert!(debug.contains("[redacted]"));

    let schema = serde_json::to_value(schemars::schema_for!(CredentialInjectorConfig)).unwrap();
    assert!(schema["properties"].get("resolvedCredential").is_none());

    let redacted = {
        let _guard = AdminRedactGuard::new();
        serde_json::to_value(&cfg).unwrap()
    };
    assert_eq!(redacted["resolvedCredential"], "[redacted]");
    assert!(!redacted.to_string().contains(secret));

    let wire = serde_json::to_value(&cfg).unwrap();
    assert_eq!(wire["resolvedCredential"]["value"], secret);
    assert_eq!(wire["resolvedCredential"]["validUntil"], "2026-08-27T12:00:00Z");
}

#[test]
fn accepts_only_secret_or_link_sys_reference_pairs() {
    let mut cfg = config(CredentialInjectionTarget::Bearer);
    assert!(cfg.validation_error().is_none());

    cfg.credential_ref.group = Some("edgion.io".to_string());
    cfg.credential_ref.kind = Some("LinkSys".to_string());
    assert!(cfg.validation_error().is_none());
    assert!(cfg.is_link_sys_ref());

    for (group, kind) in [("edgion.io", "Secret"), ("", "LinkSys"), ("example.io", "Vault")] {
        cfg.credential_ref.group = Some(group.to_string());
        cfg.credential_ref.kind = Some(kind.to_string());
        assert!(cfg.validation_error().is_some(), "{group}/{kind} must be rejected");
        assert!(!cfg.is_link_sys_ref());
    }
}

#[test]
fn static_resolved_credential_omits_valid_until_on_the_wire() {
    let mut cfg = config(CredentialInjectionTarget::Bearer);
    cfg.resolved_credential = Some(ResolvedCredentialInjector {
        value: "static-value".to_string(),
        valid_until: None,
    });

    let wire = serde_json::to_value(&cfg).unwrap();
    assert!(wire["resolvedCredential"].get("validUntil").is_none());
}
