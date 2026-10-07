use super::*;

fn owner(name: &str, policy: Option<OutboundTlsConfig>) -> Arc<EdgionGatewayConfig> {
    let mut config: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":name}, "spec":{}
    }))
    .unwrap();
    config.spec.outbound_tls = policy;
    Arc::new(config)
}

#[test]
fn empty_defaults_and_actual_owner_retention() {
    let empty = GlobalOutboundTlsSnapshot::from_configs([]);
    assert!(empty.verify());
    assert!(empty.validation().is_none());
    assert_eq!(empty.effective_proofs(&OutboundTlsLocal::default()), [Ok(()); 3]);
    let config = owner(
        "actual",
        Some(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        }),
    );
    let snapshot = GlobalOutboundTlsSnapshot::from_configs([Arc::clone(&config)]);
    assert!(Arc::ptr_eq(&config, &snapshot.candidates[0].owner));
    assert!(!snapshot.verify());
}

#[test]
fn malformed_verify_source_survives_absent_tls_and_zero_references() {
    for policy in [
        None,
        Some(OutboundTlsConfig {
            verify: false,
            ..Default::default()
        }),
    ] {
        let mut config = (*owner("invalid", policy)).clone();
        config.metadata.namespace = Some("default".into());
        let snapshot = GlobalOutboundTlsSnapshot::from_configs([Arc::new(config)]);
        let proofs = snapshot.effective_proofs(&OutboundTlsLocal::default());
        assert!(proofs[0].is_err());
        assert!(proofs[1].is_ok() && proofs[2].is_ok());
        assert_eq!(
            snapshot.effective_proofs(&OutboundTlsLocal {
                verify: Some(true),
                ..Default::default()
            }),
            [Ok(()); 3]
        );
    }
}

fn paired_wire() -> serde_json::Value {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    serde_json::json!({"metadata":{"name":"actual","uid":"owner-a","generation":1},"spec":{"outboundTls":{
        "verify":false,"validation":{"caCertificateRefs":[
            {"group":"core","kind":"Secret","name":"bundle","namespace":"apps"},
            {"kind":"ConfigMap","name":"bundle","namespace":"apps"}
        ]},"clientCertificateRef":{"name":"client","namespace":"apps"},
        "resolvedCaCertificates":[
            {"metadata":{"name":"bundle","namespace":"apps"},"data":{"ca.crt":"Y2EtYQ=="}},
            {"metadata":{"name":"bundle","namespace":"apps","annotations":{CONTROLLER_CA_SOURCE:"ConfigMap"}},"data":{"ca.crt":"Y2EtYg=="}}
        ],"resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"},"data":{"tls.crt":"Y2VydA==","tls.key":"cHJpdmF0ZS1rZXk="}}
    }}})
}
fn from_wire(wire: serde_json::Value) -> GlobalOutboundTlsSnapshot {
    let owner: EdgionGatewayConfig = serde_json::from_value(wire).unwrap();
    GlobalOutboundTlsSnapshot::from_configs([Arc::new(owner)])
}

#[test]
fn shared_global_pairing_rejects_order_identity_cardinality_and_provenance() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let valid = from_wire(paired_wire());
    assert_eq!(valid.effective_proofs(&OutboundTlsLocal::default()), [Ok(()); 3]);
    assert!(!format!("{valid:?}").contains("private-key"));
    for case in [
        "order",
        "name",
        "namespace",
        "marker",
        "missing",
        "extra",
        "invalid-ref",
        "client-marker",
    ] {
        let mut wire = paired_wire();
        let field = &mut wire["spec"]["outboundTls"];
        match case {
            "order" => field["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "name" => field["resolvedCaCertificates"][0]["metadata"]["name"] = "other".into(),
            "namespace" => field["resolvedCaCertificates"][0]["metadata"]["namespace"] = "other".into(),
            "marker" => {
                field["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"ConfigMap"})
            }
            "missing" => {
                field["resolvedCaCertificates"].as_array_mut().unwrap().pop();
            }
            "extra" => {
                let extra = field["resolvedCaCertificates"][0].clone();
                field["resolvedCaCertificates"].as_array_mut().unwrap().push(extra);
            }
            "invalid-ref" => field["validation"]["caCertificateRefs"][0]["name"] = "Bad_Name".into(),
            "client-marker" => {
                field["resolvedClientCertificate"]["metadata"]["annotations"] =
                    serde_json::json!({CONTROLLER_CA_SOURCE:"ConfigMap"})
            }
            _ => unreachable!(),
        }
        let snapshot = from_wire(wire);
        let proofs = snapshot.effective_proofs(&OutboundTlsLocal::default());
        assert!(proofs[0].is_ok());
        if case == "client-marker" {
            assert!(proofs[1].is_ok() && proofs[2].is_err());
        } else {
            assert!(proofs[1].is_err() && proofs[2].is_ok(), "{case}");
        }
        assert!(super::super::resolve_outbound_tls_with_snapshot(
            &OutboundTlsLocal::default(),
            None,
            None,
            true,
            &snapshot
        )
        .is_err());
    }
}

#[test]
fn declarations_count_before_proof_and_san_only_is_not_a_ca_candidate() {
    let valid: EdgionGatewayConfig = serde_json::from_value(paired_wire()).unwrap();
    let mut invalid = valid.clone();
    invalid.metadata.name = Some("invalid".into());
    invalid
        .spec
        .outbound_tls
        .as_mut()
        .unwrap()
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()[0]
        .name = "Bad_Name".into();
    let snapshot = GlobalOutboundTlsSnapshot::from_configs([Arc::new(valid), Arc::new(invalid)]);
    assert!(snapshot.ca_conflict() && snapshot.client_conflict());
    assert!(snapshot.validation().is_none() && snapshot.ca_material().is_none());
    assert!(snapshot.client_reference().is_some() && snapshot.client_material().is_none());
    let proofs = snapshot.effective_proofs(&OutboundTlsLocal::default());
    assert!(proofs[1].is_ok() && proofs[2].is_err());
    let san = from_wire(serde_json::json!({"metadata":{"name":"san"},"spec":{"outboundTls":{
        "validation":{"hostname":"server.example","subjectAltNames":[{"type":"Hostname","hostname":"server.example"}]}
    }}}));
    assert!(san.validation().is_none() && !san.ca_conflict());
}

#[test]
fn local_overrides_mask_only_their_own_source_axes() {
    let mut wire = paired_wire();
    wire["metadata"]["namespace"] = "apps".into();
    let snapshot = from_wire(wire);
    for mask in 0..8 {
        let local = OutboundTlsLocal {
            verify: (mask & 1 != 0).then_some(true),
            validation: (mask & 2 != 0).then(OutboundTlsValidation::default),
            client_certificate_ref: (mask & 4 != 0).then(|| SecretObjectReference {
                name: "local".into(),
                group: None,
                kind: None,
                namespace: None,
            }),
        };
        let proofs = snapshot.effective_proofs(&local);
        for (index, proof) in proofs.into_iter().enumerate() {
            assert_eq!(proof.is_ok(), mask & (1 << index) != 0);
        }
    }
}

#[test]
fn equality_preserves_raw_options_and_full_material_metadata_but_fingerprint_does_not() {
    use crate::link_sys_core::fingerprint::{hash_secret, FingerprintHasher};
    let a = from_wire(paired_wire());
    let mut wire = paired_wire();
    wire["spec"]["outboundTls"]["resolvedClientCertificate"]["metadata"]["resourceVersion"] = "new".into();
    let b = from_wire(wire);
    assert!(!a.effective_eq(&b));
    let mut first = FingerprintHasher::new();
    let mut second = FingerprintHasher::new();
    hash_secret(&mut first, a.client_material());
    hash_secret(&mut second, b.client_material());
    assert_eq!(first.finish(), second.finish());
    for (field, value) in [("group", ""), ("kind", "Secret")] {
        let mut wire = paired_wire();
        wire["spec"]["outboundTls"]["clientCertificateRef"][field] = value.into();
        let changed = from_wire(wire);
        assert!(!a.effective_eq(&changed));
        assert_eq!(changed.effective_proofs(&OutboundTlsLocal::default()), [Ok(()); 3]);
    }
}

#[test]
fn publication_retains_new_provenance_even_when_effective_values_are_equal() {
    let _guard = super::super::outbound_tls_test_guard();
    let a = Arc::new(from_wire(paired_wire()));
    assert!(super::super::set_outbound_global_default(Arc::clone(&a)));
    let mut wire = paired_wire();
    wire["metadata"]["uid"] = "owner-b".into();
    let b = Arc::new(from_wire(wire));
    assert!(!super::super::set_outbound_global_default(Arc::clone(&b)));
    assert!(Arc::ptr_eq(&b, &super::super::outbound_global_default()));
    assert_eq!(a.candidates[0].owner.metadata.uid.as_deref(), Some("owner-a"));
    assert_eq!(b.candidates[0].owner.metadata.uid.as_deref(), Some("owner-b"));
}
