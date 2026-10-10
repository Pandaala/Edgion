use super::*;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::SecretObjectReference;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn paired_local_wire() -> serde_json::Value {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    serde_json::json!({"metadata":{"name":"plugins","namespace":"apps","uid":"uid","generation":1},"spec":{"requestPlugins":[
        {"type":"OpenidConnect","config":{"discovery":"https://issuer.example","clientId":"client",
            "tls":{"verify":false,"validation":{"caCertificateRefs":[
                {"group":"core","kind":"Secret","name":"bundle"},{"kind":"ConfigMap","name":"bundle"}
            ]},"clientCertificateRef":{"name":"client"}},
            "resolvedCaCertificates":[
                {"metadata":{"name":"bundle","namespace":"apps"},"data":{"ca.crt":"bm90LXBlbQ=="}},
                {"metadata":{"name":"bundle","namespace":"apps","annotations":{CONTROLLER_CA_SOURCE:"ConfigMap"}},"data":{"ca.crt":"bm90LXBlbQ=="}}
            ],"resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"},"data":{"tls.key":"cHJpdmF0ZS10ZXN0LWtleQ=="}}}}
    ]}})
}

fn local_proof_result(wire: serde_json::Value) -> Result<(), TlsMaterialProofError> {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(wire).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let proof = validate_outbound_tls_field(entry.outbound_tls().unwrap().unwrap())?;
    assert!(std::ptr::eq(
        proof.field().ca_material().unwrap().as_ptr(),
        entry.outbound_tls().unwrap().unwrap().ca_material().unwrap().as_ptr()
    ));
    assert!(!format!("{proof:?}").contains("private-test-key"));
    Ok(())
}

#[test]
fn local_material_proof_preserves_order_kind_and_strict_markers_even_without_verification() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    assert_eq!(local_proof_result(paired_local_wire()), Ok(()));
    let cases = [
        ("reorder", TlsMaterialProofError::MaterialProvenance),
        ("missing-ca", TlsMaterialProofError::CaCardinality),
        ("extra-ca", TlsMaterialProofError::CaCardinality),
        ("name", TlsMaterialProofError::MaterialIdentity),
        ("namespace", TlsMaterialProofError::MaterialIdentity),
        ("missing-namespace", TlsMaterialProofError::MaterialIdentity),
        ("secret-forged-marker", TlsMaterialProofError::MaterialProvenance),
        ("secret-empty-marker", TlsMaterialProofError::MaterialProvenance),
        ("secret-unknown-marker", TlsMaterialProofError::MaterialProvenance),
        ("configmap-no-marker", TlsMaterialProofError::MaterialProvenance),
        ("configmap-empty-marker", TlsMaterialProofError::MaterialProvenance),
        ("configmap-unknown-marker", TlsMaterialProofError::MaterialProvenance),
        ("client-marker", TlsMaterialProofError::MaterialProvenance),
        ("client-name", TlsMaterialProofError::MaterialIdentity),
        ("client-namespace", TlsMaterialProofError::MaterialIdentity),
        ("missing-client", TlsMaterialProofError::ClientPresence),
        ("extra-client", TlsMaterialProofError::ClientPresence),
        (
            "wrong-group",
            TlsMaterialProofError::Reference(ReferenceError::UnsupportedTarget),
        ),
        (
            "invalid-kind",
            TlsMaterialProofError::Reference(ReferenceError::InvalidKind),
        ),
        (
            "invalid-name",
            TlsMaterialProofError::Reference(ReferenceError::InvalidName),
        ),
        (
            "invalid-namespace",
            TlsMaterialProofError::Reference(ReferenceError::InvalidNamespace),
        ),
    ];
    for (case, expected) in cases {
        let mut wire = paired_local_wire();
        let config = &mut wire["spec"]["requestPlugins"][0]["config"];
        match case {
            "reorder" => config["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "missing-ca" => {
                config["resolvedCaCertificates"].as_array_mut().unwrap().pop();
            }
            "extra-ca" => {
                let extra = config["resolvedCaCertificates"][0].clone();
                config["resolvedCaCertificates"].as_array_mut().unwrap().push(extra);
            }
            "name" => config["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::json!("other"),
            "namespace" => config["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!("other"),
            "missing-namespace" => {
                config["resolvedCaCertificates"][0]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("namespace");
            }
            "secret-forged-marker" => {
                config["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"ConfigMap"})
            }
            "secret-empty-marker" => {
                config["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:""})
            }
            "secret-unknown-marker" => {
                config["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"unknown"})
            }
            "configmap-no-marker" => {
                config["resolvedCaCertificates"][1]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("annotations");
            }
            "configmap-empty-marker" => {
                config["resolvedCaCertificates"][1]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:""})
            }
            "configmap-unknown-marker" => {
                config["resolvedCaCertificates"][1]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"unknown"})
            }
            "client-marker" => {
                config["resolvedClientCertificate"]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"ConfigMap"})
            }
            "client-name" => config["resolvedClientCertificate"]["metadata"]["name"] = serde_json::json!("other"),
            "client-namespace" => {
                config["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!("other")
            }
            "missing-client" => {
                config.as_object_mut().unwrap().remove("resolvedClientCertificate");
            }
            "extra-client" => {
                config["tls"].as_object_mut().unwrap().remove("clientCertificateRef");
            }
            "wrong-group" => {
                config["tls"]["validation"]["caCertificateRefs"][0]["group"] = serde_json::json!("foreign.example")
            }
            "invalid-kind" => config["tls"]["validation"]["caCertificateRefs"][0]["kind"] = serde_json::json!(""),
            "invalid-name" => config["tls"]["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!(""),
            "invalid-namespace" => {
                config["tls"]["validation"]["caCertificateRefs"][0]["namespace"] = serde_json::json!("")
            }
            _ => unreachable!(),
        }
        assert_eq!(local_proof_result(wire), Err(expected), "{case}");
    }
}

#[test]
fn local_proof_keeps_duplicate_slots_and_ignores_material_uid_rv() {
    let mut wire = paired_local_wire();
    let config = &mut wire["spec"]["requestPlugins"][0]["config"];
    config["tls"]["validation"]["caCertificateRefs"][1] = config["tls"]["validation"]["caCertificateRefs"][0].clone();
    config["resolvedCaCertificates"][1] = config["resolvedCaCertificates"][0].clone();
    config["resolvedCaCertificates"][1]["metadata"]["uid"] = serde_json::json!("replacement-material");
    config["resolvedCaCertificates"][1]["metadata"]["resourceVersion"] = serde_json::json!("999");
    assert_eq!(local_proof_result(wire), Ok(()));
}

#[test]
fn local_proof_rejects_extra_ca_without_a_declared_reference() {
    let mut wire = paired_local_wire();
    wire["spec"]["requestPlugins"][0]["config"]["tls"]
        .as_object_mut()
        .unwrap()
        .remove("validation");
    assert_eq!(local_proof_result(wire), Err(TlsMaterialProofError::CaCardinality));
}

#[test]
fn active_link_sys_skipped_ca_kind_is_unprovable_without_filtering() {
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    let owner: edgion_resources::LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"link","namespace":"apps"},"spec":{"type":"redis","config":{"tls":{
            "enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"ConfigMap","name":"bundle"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"bundle","namespace":"apps"}}]
        }}}
    }))
    .unwrap();
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let field = binding.outbound_tls().unwrap().unwrap();
    assert_eq!(
        validate_outbound_tls_field(field).unwrap_err(),
        TlsMaterialProofError::Reference(ReferenceError::UnsupportedTarget)
    );
}

fn selection_owner(tls: serde_json::Value) -> edgion_resources::LinkSys {
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":"resolver","namespace":"apps"},"spec":{"type":"httpdns","config":{
            "urlTemplate":"https://resolver.example?name={domain}","connection":{"tls":tls}
        }}
    }))
    .unwrap()
}

#[test]
fn local_selection_source_proof_cannot_be_masked_by_absence_disable_or_verify_override() {
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    for tls in [
        serde_json::Value::Null,
        serde_json::json!({"enabled":false}),
        serde_json::json!({"enabled":true,"verify":false}),
    ] {
        let mut owner = selection_owner(tls);
        owner.metadata.name = None;
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert_eq!(
            validate_local_tls_selection(binding.outbound_tls_selection().unwrap()).unwrap_err(),
            TlsMaterialProofError::Reference(ReferenceError::InvalidName)
        );
        if let Some(field) = binding.outbound_tls().unwrap() {
            assert_eq!(
                validate_outbound_tls_field(field).unwrap_err(),
                TlsMaterialProofError::Reference(ReferenceError::InvalidName)
            );
        }
    }
}

#[test]
fn inactive_selection_ignores_dormant_material_but_active_selection_proves_the_whole_field() {
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    for enabled in [false, true] {
        let owner = selection_owner(serde_json::json!({"enabled":enabled,"verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"other","namespace":"apps"}}]}));
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let proof = validate_local_tls_selection(binding.outbound_tls_selection().unwrap());
        if enabled {
            assert_eq!(proof.unwrap_err(), TlsMaterialProofError::MaterialIdentity);
        } else {
            assert!(proof.unwrap().active_field().is_none());
        }
    }
}

#[test]
fn validated_selection_resolves_only_its_actual_policy_and_captured_inherited_axes() {
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    let global_owner: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global","namespace":"invalid-cluster-namespace"},"spec":{"outboundTls":{
            "verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca","namespace":"apps"}]},
            "clientCertificateRef":{"name":"client","namespace":"apps"}
        }}
    }))
    .unwrap();
    let invalid = GlobalOutboundTlsSnapshot::from_configs([Arc::new(global_owner)]);
    for (validation, client, expected) in [
        (false, false, Some(EdgionErrorCode::OutboundTlsCaMissing)),
        (true, false, Some(EdgionErrorCode::OutboundTlsClientCertInvalid)),
        (false, true, Some(EdgionErrorCode::OutboundTlsCaMissing)),
        (true, true, None),
    ] {
        let mut tls = serde_json::json!({"enabled":true,"verify":false});
        if validation {
            tls["validation"] = serde_json::json!({});
        }
        if client {
            tls["clientCertificateRef"] = serde_json::json!({"name":"client"});
            tls["resolvedClientCertificate"] = serde_json::json!({"metadata":{"name":"client","namespace":"apps"}});
        }
        let owner = selection_owner(tls);
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let proof = validate_local_tls_selection(binding.outbound_tls_selection().unwrap()).unwrap();
        let resolved = resolve_validated_outbound_tls(&proof, true, &invalid);
        match expected {
            Some(code) => assert_eq!(resolved.err(), Some(code)),
            None => {
                assert!(!resolved.unwrap().verify);
                assert_eq!(
                    resolve_validated_outbound_tls(&proof, false, &invalid).err(),
                    Some(EdgionErrorCode::OutboundTlsVerifyUnsupported)
                );
            }
        }
    }
    let _guard = outbound_tls_test_guard();
    let a = outbound_global_default();
    assert!(a.verify(), "the test guard establishes the default verified generation");
    set_outbound_global_default(test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    }));
    let b = outbound_global_default();
    let owner = selection_owner(serde_json::Value::Null);
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let proof = validate_local_tls_selection(binding.outbound_tls_selection().unwrap()).unwrap();
    assert_eq!(
        resolve_validated_outbound_tls(&proof, true, &a).unwrap().verify,
        a.verify()
    );
    assert!(!resolve_validated_outbound_tls(&proof, true, &b).unwrap().verify);
    set_outbound_global_default(a);
}

fn cert_ref(kind: &str, name: &str) -> CustomCACertificateRef {
    CustomCACertificateRef {
        group: Some(String::new()),
        kind: kind.into(),
        name: name.into(),
        namespace: Some("default".into()),
    }
}

fn ca_secret(pem: &[u8]) -> Secret {
    let mut data = BTreeMap::new();
    data.insert(tls_keys::CA_CERT.to_string(), ByteString(pem.to_vec()));
    Secret {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("global-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

fn client_secret() -> Secret {
    let mut data = BTreeMap::new();
    data.insert(tls_keys::CERT.to_string(), ByteString(b"cert-pem".to_vec()));
    data.insert(tls_keys::KEY.to_string(), ByteString(b"key-pem".to_vec()));
    Secret {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("global-client".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

fn secret_ref(name: &str) -> SecretObjectReference {
    SecretObjectReference {
        group: None,
        kind: None,
        name: name.to_string(),
        namespace: Some("default".into()),
    }
}

fn hostname_san(h: &str) -> SubjectAltName {
    SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some(h.to_string()),
        uri: None,
    }
}

#[test]
fn aggregate_verify_safest_wins() {
    let owner = |name: &str, verify: bool| {
        let config: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
            "metadata":{"name":name},"spec":{"outboundTls":{"verify":verify}}
        }))
        .unwrap();
        Arc::new(config)
    };
    assert!(GlobalOutboundTlsSnapshot::from_configs([]).verify());
    assert!(GlobalOutboundTlsSnapshot::from_configs([owner("a", false), owner("b", true)]).verify());
    assert!(!GlobalOutboundTlsSnapshot::from_configs([owner("a", false), owner("c", false)]).verify());
}

#[test]
fn setting_global_default_reports_only_effective_changes() {
    let _guard = outbound_tls_test_guard();
    assert!(!set_outbound_global_default(
        crate::link_sys_core::outbound_tls::test_util::snapshot(OutboundTlsConfig::default())
    ));
    assert!(set_outbound_global_default(
        crate::link_sys_core::outbound_tls::test_util::snapshot(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        })
    ));
    assert!(!set_outbound_global_default(
        crate::link_sys_core::outbound_tls::test_util::snapshot(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        })
    ));
}

#[test]
fn verify_false_is_honored_when_provider_can_disable() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(&local, None, None, true, "Test", &outbound_global_default())
        .expect("verify=false must be honored");
    assert!(
        !r.verify,
        "resolved verify must be false when operator sets verify=false"
    );
}

#[test]
fn gate_rejects_skip_when_provider_cannot_disable() {
    let _guard = outbound_tls_test_guard();
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    assert!(
        resolve_outbound_tls_audited_with_defaults(&local, None, None, false, "Redis", &outbound_global_default())
            .is_err()
    );
}

#[test]
fn inherits_global_verify_true_when_local_none() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            verify: true,
            ..Default::default()
        },
    ));
    let r = resolve_outbound_tls_audited_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    assert!(r.verify);
}

#[test]
fn ca_refs_present_yields_multi_bundle_custom() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let s1 = ca_secret(b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n");
    let s2 = ca_secret(b"-----BEGIN CERTIFICATE-----\nB\n-----END CERTIFICATE-----\n");
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "a"), cert_ref("Secret", "b")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(
        &local,
        Some(&[s1, s2]),
        None,
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    match r.ca {
        EffectiveCa::Custom(b) => assert_eq!(b.len(), 2),
        _ => panic!("expected multi-bundle Custom"),
    }
}

#[test]
fn well_known_system_yields_system() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(&local, None, None, true, "Test", &outbound_global_default())
        .unwrap();
    assert!(matches!(r.ca, EffectiveCa::System));
}

#[test]
fn unsupported_named_ca_never_falls_back_to_system() {
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            well_known_ca_certificates: Some(WellKnownCACertificates::Other("example.com/Custom".to_string())),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = resolve_outbound_tls_with_defaults(&local, None, None, true, &OutboundTlsConfig::default());
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsCaMissing)));
}

#[test]
fn both_absent_defaults_to_system_public_roots() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let r = resolve_outbound_tls_audited_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    assert!(matches!(r.ca, EffectiveCa::System));
}

/// Fail-secure: a non-empty subjectAltNames policy overrides verify=false, so
/// the effective `verify` becomes true and the client mTLS identity is retained
/// (rather than dropped by the skip-verify path).
#[test]
fn san_policy_overrides_verify_false_and_keeps_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        verify: Some(false),
        validation: Some(OutboundTlsValidation {
            subject_alt_names: Some(vec![hostname_san("idp.example.com")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(
        &local,
        None,
        Some(&client_secret()),
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    assert!(r.verify, "SAN policy must force verify=true (fail-secure)");
    assert!(
        r.client_identity.is_some(),
        "client identity must be retained when SAN raises verify"
    );
}

/// verify=false with no SAN policy is a genuine skip-verify path: client
/// identity is dropped (M4).
#[test]
fn verify_false_without_san_drops_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(
        &local,
        None,
        Some(&client_secret()),
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    assert!(!r.verify);
    assert!(
        r.client_identity.is_none(),
        "skip-verify path must drop client identity"
    );
}

/// A local validation block declaring caCertificateRefs but with no per-call
/// resolved bytes must fail with OutboundTlsCaMissing — it must NOT silently
/// fall back to the global resolved CA (which belongs to the global refs).
#[test]
fn local_ca_refs_without_resolved_bytes_errors_no_global_fallback() {
    let _guard = outbound_tls_test_guard();
    // Global has its own resolved CA that must NOT be borrowed by the local block.
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
                ..Default::default()
            }),
            resolved_ca_certificates: Some(vec![ca_secret(
                b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
            )]),
            ..Default::default()
        },
    ));
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "local-ca")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls_audited_with_defaults(&local, None, None, true, "Test", &outbound_global_default());
    assert!(
        matches!(r, Err(EdgionErrorCode::OutboundTlsCaMissing)),
        "local CA refs with no resolved bytes must error, got: {:?}",
        r.map(|_| "Ok(resolved)")
    );
}

#[test]
fn partially_resolved_ca_refs_fail_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "ca-a"), cert_ref("Secret", "ca-b")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let only_one_secret = [ca_secret(
        b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n",
    )];

    let result = resolve_outbound_tls_audited_with_defaults(
        &local,
        Some(&only_one_secret),
        None,
        true,
        "Test",
        &outbound_global_default(),
    );
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsCaMissing)));
}

/// When the local block declares no validation, the global validation block
/// wins and its Controller-shipped resolved CA bundles are used.
#[test]
fn inherits_global_ca_when_local_validation_absent() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
                ..Default::default()
            }),
            resolved_ca_certificates: Some(vec![ca_secret(
                b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
            )]),
            ..Default::default()
        },
    ));
    let r = resolve_outbound_tls_audited_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        true,
        "Test",
        &outbound_global_default(),
    )
    .unwrap();
    match r.ca {
        EffectiveCa::Custom(b) => assert_eq!(b.len(), 1),
        _ => panic!("expected global resolved CA to be inherited as Custom"),
    }
}

#[test]
fn local_client_ref_without_resolved_secret_fails_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let local = OutboundTlsLocal {
        client_certificate_ref: Some(secret_ref("local-client")),
        ..Default::default()
    };

    let result =
        resolve_outbound_tls_audited_with_defaults(&local, None, None, true, "Test", &outbound_global_default());
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
}

#[test]
fn global_client_ref_without_resolved_secret_fails_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            ..Default::default()
        },
    ));

    let result = resolve_outbound_tls_audited_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        true,
        "Test",
        &outbound_global_default(),
    );
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

#[test]
fn explicit_defaults_do_not_inherit_process_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        },
    ));
    let resolved = resolve_outbound_tls_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        false,
        &OutboundTlsConfig::default(),
    )
    .expect("explicit defaults must resolve independently");
    assert!(resolved.verify);
    assert!(matches!(resolved.ca, EffectiveCa::System));
    assert!(resolved.client_identity.is_none());
}

#[test]
fn global_client_identity_is_inherited_when_local_ref_is_absent() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        },
    ));

    let resolved = resolve_outbound_tls_audited_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        true,
        "Test",
        &outbound_global_default(),
    )
    .expect("global client identity must resolve");
    assert!(resolved.client_identity.is_some());
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

#[test]
fn local_client_ref_does_not_borrow_global_resolved_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            client_certificate_ref: Some(secret_ref("global-client")),
            resolved_client_certificate: Some(client_secret()),
            ..Default::default()
        },
    ));
    let local = OutboundTlsLocal {
        client_certificate_ref: Some(secret_ref("local-client")),
        ..Default::default()
    };

    let result =
        resolve_outbound_tls_audited_with_defaults(&local, None, None, true, "Test", &outbound_global_default());
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
    set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
}

fn bootstrap_material() -> edgion_resources::credentials::materials::TlsMaterial {
    edgion_resources::credentials::materials::TlsMaterial {
        ca_pem: None,
        client_cert_pem: Some(b"disk-cert".to_vec()),
        client_key_pem: Some(Zeroizing::new(b"disk-key".to_vec())),
    }
}

#[test]
fn trusted_etcd_pem_uses_the_same_whole_block_and_client_precedence() {
    let _guard = outbound_tls_test_guard();
    let globals = test_util::snapshot(OutboundTlsConfig {
        validation: Some(OutboundTlsValidation {
            hostname: Some("global.example".into()),
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut disk = bootstrap_material();
    let inherited = resolve_etcd_bootstrap_tls_audited(&disk, &globals).unwrap();
    assert_eq!(
        inherited.san.as_ref().unwrap().hostname.as_deref(),
        Some("global.example")
    );
    assert_eq!(inherited.client_identity.as_ref().unwrap().0, b"disk-cert");
    disk.ca_pem = Some(b"disk-ca".to_vec());
    let local = resolve_etcd_bootstrap_tls_audited(&disk, &globals).unwrap();
    assert!(
        local.san.is_none(),
        "disk CA masks the entire inherited validation block"
    );
    let EffectiveCa::Custom(ca) = local.ca else {
        panic!("disk CA must remain custom")
    };
    assert_eq!(ca, vec![b"disk-ca".to_vec()]);

    let wire = serde_json::json!({"metadata":{"name":"global"},"spec":{"outboundTls":{
        "clientCertificateRef":{"name":"client","namespace":"apps"},
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"},"data":{"tls.crt":"Z2xvYmFsLWNlcnQ=","tls.key":"Z2xvYmFsLWtleQ=="}}
    }}});
    let owner: edgion_resources::EdgionGatewayConfig = serde_json::from_value(wire.clone()).unwrap();
    let global = GlobalOutboundTlsSnapshot::from_configs([Arc::new(owner)]);
    let resolved = resolve_etcd_bootstrap_tls_audited(&disk, &global).unwrap();
    let (cert, key) = resolved.client_identity.unwrap();
    assert_eq!(cert, b"global-cert");
    assert_eq!(key.as_slice(), b"global-key");
    let mut missing = wire;
    missing["spec"]["outboundTls"]["resolvedClientCertificate"] = serde_json::Value::Null;
    let owner: edgion_resources::EdgionGatewayConfig = serde_json::from_value(missing).unwrap();
    let invalid = GlobalOutboundTlsSnapshot::from_configs([Arc::new(owner)]);
    assert_eq!(
        resolve_etcd_bootstrap_tls_audited(&disk, &invalid).err().unwrap(),
        EdgionErrorCode::OutboundTlsClientCertInvalid,
        "declared global identity failure cannot fall back to a disk identity"
    );
}

#[test]
fn trusted_etcd_pem_inherits_verify_and_never_constructs_partial_identity() {
    let _guard = outbound_tls_test_guard();
    let insecure = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    assert_eq!(
        resolve_etcd_bootstrap_tls_audited(&bootstrap_material(), &insecure)
            .err()
            .unwrap(),
        EdgionErrorCode::OutboundTlsVerifyUnsupported
    );
    let empty = GlobalOutboundTlsSnapshot::from_configs([]);
    let mut disk = bootstrap_material();
    disk.client_key_pem = None;
    assert!(resolve_etcd_bootstrap_tls_audited(&disk, &empty)
        .unwrap()
        .client_identity
        .is_none());
    disk.client_cert_pem = None;
    disk.client_key_pem = Some(Zeroizing::new(b"disk-key".to_vec()));
    assert!(resolve_etcd_bootstrap_tls_audited(&disk, &empty)
        .unwrap()
        .client_identity
        .is_none());
}

#[test]
fn internal_route_tls_projection_keeps_exact_order_source_kind_and_client_identity() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    use edgion_resources::resources::refs::route_auth_fields::HttpRouteAuthRefs;
    let wire = serde_json::json!({"metadata":{"name":"root","namespace":"root"},"spec":{"resolvedRules":[{
        "filters":[{"type":"ExternalAuth","resolvedExternalAuth":{"namespace":"child"},"externalAuth":{
            "target":{"name":"auth","port":8443},"decision":{},"tls":{"enabled":true,"verify":false,
                "validation":{"caCertificateRefs":[{"kind":"Secret","name":"bundle"},{"kind":"ConfigMap","name":"bundle"}]},
                "clientCertificateRef":{"name":"client"},
                "resolvedCaCertificates":[{"metadata":{"name":"bundle","namespace":"child"}},
                    {"metadata":{"name":"bundle","namespace":"child","annotations":{CONTROLLER_CA_SOURCE:"ConfigMap"}}}],
                "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"child"}}
            }
        }}]
    }]}});
    let prove = |value| {
        let route: edgion_resources::HTTPRoute = serde_json::from_value(value).unwrap();
        let refs = HttpRouteAuthRefs::from_route(&route).unwrap();
        let field = refs.effective_at(0, None, 0).unwrap();
        validate_local_tls_selection(field.outbound_tls_selection().unwrap()).map(|_| ())
    };
    assert_eq!(prove(wire.clone()), Ok(()));
    let base = "/spec/resolvedRules/0/filters/0/externalAuth/tls";
    for (tail, value, expected) in [
        (
            "/resolvedCaCertificates/0/metadata/namespace",
            serde_json::json!("root"),
            TlsMaterialProofError::MaterialIdentity,
        ),
        (
            "/resolvedClientCertificate/metadata/name",
            serde_json::json!("decoy"),
            TlsMaterialProofError::MaterialIdentity,
        ),
        (
            "/resolvedClientCertificate/metadata/namespace",
            serde_json::json!("root"),
            TlsMaterialProofError::MaterialIdentity,
        ),
        (
            "/resolvedCaCertificates",
            serde_json::json!([]),
            TlsMaterialProofError::CaCardinality,
        ),
        (
            "/resolvedClientCertificate",
            serde_json::json!(null),
            TlsMaterialProofError::ClientPresence,
        ),
    ] {
        let mut value_wire = wire.clone();
        *value_wire.pointer_mut(&format!("{base}{tail}")).unwrap() = value;
        assert_eq!(prove(value_wire), Err(expected), "{tail}");
    }
    let mut reordered = wire.clone();
    reordered
        .pointer_mut(&format!("{base}/resolvedCaCertificates"))
        .unwrap()
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert_eq!(prove(reordered), Err(TlsMaterialProofError::MaterialProvenance));
    let mut disabled = wire;
    *disabled.pointer_mut(&format!("{base}/enabled")).unwrap() = serde_json::json!(false);
    *disabled
        .pointer_mut(&format!("{base}/resolvedClientCertificate/metadata/name"))
        .unwrap() = serde_json::json!("decoy");
    assert_eq!(prove(disabled), Ok(()));
}

#[test]
fn canonical_material_comparator_preserves_strict_namespace_and_error_precedence() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let mut wire = paired_local_wire();
    wire["metadata"]["namespace"] = serde_json::json!("default");
    for material in wire["spec"]["requestPlugins"][0]["config"]["resolvedCaCertificates"]
        .as_array_mut()
        .unwrap()
    {
        material["metadata"]["namespace"] = serde_json::json!("default");
    }
    wire["spec"]["requestPlugins"][0]["config"]["resolvedClientCertificate"]["metadata"]["namespace"] =
        serde_json::json!("default");
    assert_eq!(local_proof_result(wire.clone()), Ok(()));
    let material = &mut wire["spec"]["requestPlugins"][0]["config"]["resolvedCaCertificates"][0];
    material["metadata"].as_object_mut().unwrap().remove("namespace");
    material["metadata"]["annotations"] = serde_json::json!({CONTROLLER_CA_SOURCE:"ConfigMap"});
    assert_eq!(
        local_proof_result(wire.clone()),
        Err(TlsMaterialProofError::MaterialIdentity)
    );
    wire["spec"]["requestPlugins"][0]["config"]["resolvedCaCertificates"][0]["metadata"]["namespace"] =
        serde_json::json!("default");
    assert_eq!(
        local_proof_result(wire.clone()),
        Err(TlsMaterialProofError::MaterialProvenance)
    );
    wire["spec"]["requestPlugins"][0]["config"]["tls"]["validation"]["caCertificateRefs"][0]["name"] =
        serde_json::json!("");
    assert!(matches!(
        local_proof_result(wire),
        Err(TlsMaterialProofError::Reference(_))
    ));
}
