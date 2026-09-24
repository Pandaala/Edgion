use super::*;
use base64::Engine;

#[test]
fn operator_validation_requires_secret_ref() {
    let cfg = JweDecryptConfig::default();
    let err = cfg.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("secretRef is required"));
}

#[test]
fn test_detect_validation_error_allowed_algorithms_empty() {
    let cfg = JweDecryptConfig {
        secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        allowed_algorithms: Some(vec![]),
        ..Default::default()
    };
    let err = cfg.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("allowedAlgorithms cannot be empty"));
}

#[test]
fn test_detect_validation_error_key_len_mismatch() {
    let cfg = JweDecryptConfig {
        secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode("short-key")),
        }),
        ..Default::default()
    };
    assert!(cfg.detect_validation_error().is_none());
    assert_eq!(
        cfg.resolved_key_material().unwrap_err(),
        ResolvedJweCredentialError::InvalidLength {
            required: 32,
            actual: 9,
        }
    );
}

#[test]
fn test_detect_validation_error_valid_resolved_secret() {
    let cfg = JweDecryptConfig {
        secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode("0123456789abcdef0123456789abcdef")),
        }),
        ..Default::default()
    };
    let err = cfg.detect_validation_error();
    assert!(err.is_none());
}

#[test]
fn resolved_credential_cannot_replace_operator_secret_ref() {
    use crate::resources::edgion_plugins::PluginValidation;
    let config = JweDecryptConfig {
        resolved_credential: Some(ResolvedJweCredential { secret: None }),
        ..Default::default()
    };
    assert_eq!(config.validation_error(), Some("secretRef is required".to_string()));
    assert_eq!(
        config.resolved_key_material().unwrap_err(),
        ResolvedJweCredentialError::Missing
    );
}

#[test]
fn resolved_key_material_classifies_transport_and_inner_base64_failures() {
    let secret_ref = Some(SecretObjectReference {
        group: None,
        kind: None,
        name: "jwe-secret".to_string(),
        namespace: None,
    });
    let transport_invalid = JweDecryptConfig {
        secret_ref: secret_ref.clone(),
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some("not-base64!".to_string()),
        }),
        ..Default::default()
    };
    assert_eq!(
        transport_invalid.resolved_key_material().unwrap_err(),
        ResolvedJweCredentialError::TransportDecodeFailed
    );

    let inner_invalid = JweDecryptConfig {
        secret_ref,
        base64_secret: true,
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode("not-base64!")),
        }),
        ..Default::default()
    };
    assert_eq!(
        inner_invalid.resolved_key_material().unwrap_err(),
        ResolvedJweCredentialError::SecretDecodeFailed
    );

    let inner_non_utf8 = JweDecryptConfig {
        secret_ref: transport_invalid.secret_ref.clone(),
        base64_secret: true,
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode([0xff])),
        }),
        ..Default::default()
    };
    assert_eq!(
        inner_non_utf8.resolved_key_material().unwrap_err(),
        ResolvedJweCredentialError::SecretDecodeFailed
    );

    let key = b"0123456789abcdef0123456789abcdef";
    let double_encoded = JweDecryptConfig {
        secret_ref: transport_invalid.secret_ref,
        base64_secret: true,
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(
                base64::engine::general_purpose::STANDARD.encode(base64::engine::general_purpose::STANDARD.encode(key)),
            ),
        }),
        ..Default::default()
    };
    assert_eq!(double_encoded.resolved_key_material().unwrap().as_slice(), key);
}

#[test]
fn validation_rejects_case_insensitive_payload_header_destination_collision() {
    let config = JweDecryptConfig {
        secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        payload_to_headers: Some(HashMap::from([
            ("sub".to_string(), "X-Identity".to_string()),
            ("email".to_string(), "x-identity".to_string()),
        ])),
        ..Default::default()
    };

    assert_eq!(
        config.validation_error().as_deref(),
        Some(super::super::DUPLICATE_HEADER_DESTINATION)
    );
}

#[test]
fn validation_rejects_payload_destination_owned_by_forward_header() {
    let config = JweDecryptConfig {
        secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        forward_header: "X-Decrypted".to_string(),
        payload_to_headers: Some(HashMap::from([("sub".to_string(), "x-decrypted".to_string())])),
        ..Default::default()
    };

    assert_eq!(
        config.validation_error().as_deref(),
        Some(super::super::DUPLICATE_HEADER_DESTINATION)
    );
}
