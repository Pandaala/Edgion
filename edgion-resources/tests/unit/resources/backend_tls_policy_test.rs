use super::*;
use crate::PolicyTargetKind;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn make_policy(options: Option<HashMap<String, String>>) -> BackendTLSPolicy {
    BackendTLSPolicy {
        metadata: ObjectMeta {
            name: Some("test".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: "".to_string(),
                kind: "Service".to_string(),
                name: "svc".to_string(),
                section_name: None,
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: "backend.example.com".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: false,
        },
        status: None,
    }
}

#[test]
fn test_client_certificate_secret_ref_none() {
    let policy = make_policy(None);
    assert!(parse_client_certificate_option(policy.spec.options.as_ref())
        .unwrap()
        .is_none());
}

#[test]
fn test_client_certificate_secret_ref_valid() {
    let mut options = HashMap::new();
    options.insert(
        OPTION_CLIENT_CERTIFICATE_REF.to_string(),
        "client-cert-secret".to_string(),
    );
    let policy = make_policy(Some(options));
    let ref_ = parse_client_certificate_option(policy.spec.options.as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(ref_.name, "client-cert-secret");
    assert!(ref_.namespace.is_none());
}

#[test]
fn test_client_certificate_secret_ref_invalid_namespace_value() {
    let mut options = HashMap::new();
    options.insert(
        OPTION_CLIENT_CERTIFICATE_REF.to_string(),
        "default/client-cert-secret".to_string(),
    );
    let policy = make_policy(Some(options));
    assert!(parse_client_certificate_option(policy.spec.options.as_ref()).is_err());
}

#[test]
fn test_client_certificate_secret_ref_rejects_empty_and_invalid_names() {
    for value in ["", "   ", "Invalid_Name", "-leading", "trailing-"] {
        let policy = make_policy(Some(HashMap::from([(
            OPTION_CLIENT_CERTIFICATE_REF.to_string(),
            value.to_string(),
        )])));
        assert!(
            parse_client_certificate_option(policy.spec.options.as_ref()).is_err(),
            "accepted {value:?}"
        );
    }
}

#[test]
fn backend_tls_runtime_identity_validation_covers_hostname_and_typed_sans() {
    let mut validation = BackendTLSPolicyValidation {
        ca_certificate_refs: None,
        hostname: "backend.example.com".to_string(),
        subject_alt_names: Some(vec![
            SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("*.example.com".to_string()),
                uri: None,
            },
            SubjectAltName {
                san_type: SubjectAltNameType::URI,
                hostname: None,
                uri: Some("spiffe://trust.example/workload".to_string()),
            },
        ]),
        well_known_ca_certificates: Some(WellKnownCACertificates::System),
    };
    assert!(backend_tls_identity_validation_errors(&validation).is_empty());

    validation.hostname.clear();
    assert!(!backend_tls_identity_validation_errors(&validation).is_empty());
    validation.hostname = "backend.example.com".to_string();

    for sans in [
        Vec::new(),
        vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: None,
            uri: None,
        }],
        vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("backend.example.com".to_string()),
            uri: Some("spiffe://trust.example/workload".to_string()),
        }],
        vec![SubjectAltName {
            san_type: SubjectAltNameType::URI,
            hostname: None,
            uri: Some("not an absolute uri".to_string()),
        }],
        vec![SubjectAltName {
            san_type: SubjectAltNameType::URI,
            hostname: None,
            uri: Some(format!("https://example.com/{}", "a".repeat(240))),
        }],
        (0..6)
            .map(|index| SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some(format!("backend-{index}.example.com")),
                uri: None,
            })
            .collect(),
    ] {
        validation.subject_alt_names = Some(sans);
        assert!(!backend_tls_identity_validation_errors(&validation).is_empty());
    }
}

#[test]
fn test_well_known_ca_certificates_system_serialization() {
    let validation = BackendTLSPolicyValidation {
        ca_certificate_refs: None,
        hostname: "backend.example.com".to_string(),
        subject_alt_names: None,
        well_known_ca_certificates: Some(WellKnownCACertificates::System),
    };

    let json = serde_json::to_value(&validation).unwrap();
    assert_eq!(json["wellKnownCACertificates"], "System");
    assert!(json.get("caCertificateRefs").is_none());
}

#[test]
fn test_well_known_ca_certificates_deserialization() {
    let json = serde_json::json!({
        "hostname": "backend.example.com",
        "wellKnownCACertificates": "System"
    });

    let validation: BackendTLSPolicyValidation = serde_json::from_value(json).unwrap();
    assert_eq!(
        validation.well_known_ca_certificates,
        Some(WellKnownCACertificates::System)
    );
    assert!(validation.ca_certificate_refs.is_none());
}

#[test]
fn named_well_known_ca_round_trips_without_becoming_system() {
    let source = serde_json::json!("example.com/Custom");
    let parsed: WellKnownCACertificates = serde_json::from_value(source.clone()).unwrap();
    assert_eq!(parsed, WellKnownCACertificates::Other("example.com/Custom".to_string()));
    assert_eq!(serde_json::to_value(parsed).unwrap(), source);
}

#[test]
fn test_use_system_ca_default_false() {
    let policy = make_policy(None);
    assert!(!policy.spec.use_system_ca);
}

#[test]
fn test_use_system_ca_skip_serialization_when_false() {
    let policy = make_policy(None);
    let json = serde_json::to_value(&policy.spec).unwrap();
    assert!(json.get("useSystemCa").is_none());
}

#[test]
fn canonical_target_identity_is_typed() {
    let service = BackendTLSPolicyTargetRef {
        group: "".to_string(),
        kind: "Service".to_string(),
        name: "shared".to_string(),
        section_name: Some("https".to_string()),
    };
    let backend = BackendTLSPolicyTargetRef {
        group: "edgion.io".to_string(),
        kind: "EdgionBackend".to_string(),
        name: "shared".to_string(),
        section_name: None,
    };

    assert_eq!(
        service.canonical_key("tenant").unwrap().target_kind,
        PolicyTargetKind::Service
    );
    assert_eq!(
        backend.canonical_key("tenant").unwrap().target_kind,
        PolicyTargetKind::EdgionBackend
    );
    assert_ne!(
        service.canonical_key("tenant").unwrap(),
        backend.canonical_key("tenant").unwrap()
    );
}

#[test]
fn resolved_target_class_is_transport_only() {
    let policy = make_policy(None);
    let json = serde_json::to_value(&policy.spec).unwrap();
    assert_eq!(json["resolvedTargetClass"], "service");

    let schema = schemars::schema_for!(BackendTLSPolicySpec);
    let schema_json = serde_json::to_value(schema).unwrap();
    assert!(!schema_json.to_string().contains("resolvedTargetClass"));
}

#[test]
fn standard_policy_target_does_not_alias_literal_core() {
    let target = BackendTLSPolicyTargetRef {
        group: "core".to_string(),
        kind: "Service".to_string(),
        name: "api".to_string(),
        section_name: None,
    };
    assert!(target.canonical_key("tenant").is_err());
}
