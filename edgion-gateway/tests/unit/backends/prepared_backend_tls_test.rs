use super::*;

#[test]
fn ca_preparation_keeps_all_pem_blocks_and_rejects_invalid_bundles() {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["ca.example".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap()
        .pem();
    let mut policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"tls"},"spec":{"targetRefs":[],"validation":{"hostname":"backend.example"}}
    }))
    .unwrap();
    let ca = |value: &str| Secret {
        data: Some(std::collections::BTreeMap::from([(
            CA_CERT.into(),
            k8s_openapi::ByteString(value.as_bytes().to_vec()),
        )])),
        ..Default::default()
    };
    policy.spec.resolved_ca_certificates = Some(vec![ca(&format!("{cert}{cert}")), ca(&cert)]);
    assert_eq!(build_ca_chain(&policy).unwrap().unwrap().len(), 3);
    for bad in ["", "invalid"] {
        policy.spec.resolved_ca_certificates = Some(vec![ca(bad)]);
        assert!(matches!(
            PreparedBackendTls::new(&policy).ca,
            Err(EdgionErrorCode::BackendTlsCaInvalid)
        ));
        policy.spec.use_system_ca = true;
        assert!(matches!(PreparedBackendTls::new(&policy).ca, Ok(None)));
        policy.spec.use_system_ca = false;
    }
    policy.spec.resolved_ca_certificates = Some(vec![Secret::default()]);
    assert!(matches!(build_ca_chain(&policy), Ok(None)));
}

#[test]
fn shared_san_configuration_has_independent_results_and_rejects_panics() {
    let sans = serde_json::from_value::<Vec<edgion_resources::resources::backend_tls_policy::SubjectAltName>>(
        serde_json::json!([
            {"type":"Hostname", "hostname":"backend.example"}
        ]),
    )
    .unwrap();
    let passed = evaluate_sans(&sans, || vec!["backend.example".into()]);
    let failed = evaluate_sans(&sans, || vec!["other.example".into()]);
    let panicked = evaluate_sans(&sans, || panic!("test callback failure"));
    assert!(passed.passed);
    assert!(!failed.passed);
    assert!(!panicked.passed);
    assert_eq!(panicked.reason, "backend SAN validation panicked");
}
