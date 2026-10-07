use super::*;
use serde_json::{json, Value};

fn config(ca: Value, client: Value) -> EdgionGatewayConfig {
    serde_json::from_value(json!({"metadata":{"name":"global","uid":"uid","generation":2},
        "spec":{"outboundTls":{"validation":{"caCertificateRefs":ca},"clientCertificateRef":client}}}))
    .unwrap()
}
fn valid() -> EdgionGatewayConfig {
    config(
        json!([{"kind":"ConfigMap","name":"ca","namespace":"certs"}]),
        json!({"name":"client","namespace":"certs"}),
    )
}

#[test]
fn selections_keep_true_cluster_owner_explicit_targets_and_private_roles() {
    let owner = valid();
    let prepared = PreparedEgcTlsReferences::from_config(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let references = binding.references().unwrap();
    let ca = references.ca_certificates().next().unwrap().unwrap();
    let client = references.client_certificate().unwrap().unwrap();
    for selection in [&ca, &client] {
        let reference = selection.reference().validated().unwrap();
        assert_eq!(reference.owner().scope(), super::super::ResourceScope::Cluster);
        assert_eq!(reference.source().scope(), super::super::ResourceScope::Cluster);
        assert_eq!(reference.source().group_kind().group(), "edgion.io");
        assert_eq!(reference.source().group_kind().kind(), "EdgionGatewayConfig");
        assert_eq!(reference.target().scope().namespace(), Some("certs"));
        assert_eq!(reference.target().group_kind().group(), "");
        assert_eq!(
            selection.reference().freeze().err(),
            Some(ReferenceError::UnsupportedTarget)
        );
    }
    assert!(!ca.is_client_certificate());
    assert!(client.is_client_certificate());
    assert_eq!(
        ca.reference().validated().unwrap().target().group_kind().kind(),
        "ConfigMap"
    );
    assert_eq!(
        client.reference().validated().unwrap().target().group_kind().kind(),
        "Secret"
    );
}

#[test]
fn cluster_owner_is_validated_even_without_outbound_fields() {
    for metadata in [
        json!({}),
        json!({"name":""}),
        json!({"name":"bad/name"}),
        json!({"name":"global","namespace":""}),
        json!({"name":"global","namespace":"default"}),
    ] {
        for spec in [json!({}), json!({"outboundTls":{}})] {
            let mut owner: EdgionGatewayConfig =
                serde_json::from_value(json!({"metadata":metadata,"spec":spec})).unwrap();
            let prepared = PreparedEgcTlsReferences::from_config(&owner);
            assert!(prepared.bind(&owner).is_err());
            assert!(prepared.bind_mut(&mut owner).is_err());
        }
    }
    let owner: EdgionGatewayConfig = serde_json::from_value(json!({"metadata":{"name":"global"},"spec":{}})).unwrap();
    let prepared = PreparedEgcTlsReferences::from_config(&owner);
    assert!(prepared.bind(&owner).unwrap().outbound_tls().unwrap().is_none());
}

#[test]
fn target_namespace_and_group_kind_defaults_are_field_specific() {
    for (namespace, valid_namespace) in [
        (None, false),
        (Some(""), false),
        (Some("Bad"), false),
        (Some("certs"), true),
    ] {
        for (group, valid_group) in [
            (None, true),
            (Some(""), true),
            (Some("core"), true),
            (Some("other.io"), false),
        ] {
            for (kind, valid_kind) in [
                (None, true),
                (Some("Secret"), true),
                (Some(""), false),
                (Some("ConfigMap"), false),
            ] {
                let owner = config(
                    json!([]),
                    json!({"namespace":namespace,"group":group,"kind":kind,"name":"client"}),
                );
                let prepared = PreparedEgcTlsReferences::from_config(&owner);
                let binding = prepared.bind(&owner).unwrap();
                assert_eq!(
                    binding.references().unwrap().client_certificate().unwrap().is_ok(),
                    valid_namespace && valid_group && valid_kind
                );
            }
        }
    }
    for kind in ["Secret", "ConfigMap", "Service", ""] {
        let owner = config(
            json!([{"group":"core","kind":kind,"name":"ca","namespace":"certs"}]),
            Value::Null,
        );
        let prepared = PreparedEgcTlsReferences::from_config(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert_eq!(
            binding.references().unwrap().ca_certificates().next().unwrap().is_ok(),
            matches!(kind, "Secret" | "ConfigMap")
        );
    }
}

#[test]
fn duplicate_and_invalid_ca_slots_keep_order_without_suppressing_client() {
    let owner = config(
        json!([
            {"kind":"Secret","name":"one","namespace":"certs"},
            {"kind":"Secret","name":"bad"},
            {"kind":"Secret","name":"one","namespace":"certs"},
            {"kind":"ConfigMap","name":"two","namespace":"certs"},
        ]),
        json!({"name":"client","namespace":"certs"}),
    );
    let prepared = PreparedEgcTlsReferences::from_config(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let references = binding.references().unwrap();
    let ca: Vec<_> = references.ca_certificates().collect();
    assert_eq!(ca.len(), 4);
    assert!(matches!(ca[1], Err(ReferenceError::MissingNamespace)));
    for (index, name) in [(0, "one"), (2, "one"), (3, "two")] {
        assert_eq!(
            ca[index]
                .as_ref()
                .unwrap()
                .reference()
                .validated()
                .unwrap()
                .target()
                .name(),
            name
        );
    }
    assert!(references.client_certificate().unwrap().is_ok());
}

#[test]
fn reference_drift_rejects_but_actual_policy_and_material_writeback_remain_borrowed() {
    let mut owner = valid();
    let prepared = PreparedEgcTlsReferences::from_config(&owner);
    for mutation in 0..6 {
        let mut changed = owner.clone();
        match mutation {
            0 => changed.metadata.uid = Some("replacement".into()),
            1 => changed.metadata.generation = Some(3),
            2 => changed.metadata.namespace = Some("fake".into()),
            3 => changed.spec.outbound_tls = None,
            4 => changed.spec.outbound_tls.as_mut().unwrap().validation = None,
            _ => {
                changed
                    .spec
                    .outbound_tls
                    .as_mut()
                    .unwrap()
                    .client_certificate_ref
                    .as_mut()
                    .unwrap()
                    .name = "other".into()
            }
        }
        assert!(prepared.bind(&changed).is_err());
    }
    let mut binding = prepared.bind_mut(&mut owner).unwrap();
    let actual = binding.outbound_tls_mut().as_mut().unwrap();
    actual.verify = false;
    actual.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret::default()]);
    let field = binding.outbound_tls().unwrap().unwrap();
    assert!(!field.policy().verify);
    assert_eq!(field.ca_reference_count(), 1);
    let mut pairs = 0;
    field.visit_ca(|pair| {
        pairs += 1;
        assert!(std::ptr::eq(
            pair.material().unwrap(),
            &field.policy().resolved_ca_certificates.as_ref().unwrap()[0]
        ));
        assert_eq!(pair.reference().validated().unwrap().target().name(), "ca");
    });
    assert_eq!(pairs, 1);
    assert!(field.client_certificate().unwrap().material().is_none());
    binding
        .outbound_tls_mut()
        .as_mut()
        .unwrap()
        .client_certificate_ref
        .as_mut()
        .unwrap()
        .name = "other".into();
    assert!(matches!(binding.references(), Err(ReferenceError::StaleBinding)));
}
