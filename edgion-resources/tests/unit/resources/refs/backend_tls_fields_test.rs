use super::*;
use serde_json::json;

fn policy() -> BackendTLSPolicy {
    serde_json::from_value(json!({"metadata":{"name":"policy","namespace":"apps"},"spec":{
        "targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example","caCertificateRefs":[
            {"group":"","kind":"Secret","name":"shared"},{"group":"","kind":"ConfigMap","name":"shared"}]},
        "options":{"edgion.io/client-certificate-ref":"  client  "},
        "resolvedCaCertificates":[{"metadata":{"name":"shared","namespace":"apps"},"data":{"ca.crt":"Y2E="}},
          {"metadata":{"name":"shared","namespace":"apps","annotations":{"edgion.io/controller-ca-source":"ConfigMap"}},"data":{"ca.crt":"Y2E="}}],
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"},"data":{"tls.crt":"Y2VydA==","tls.key":"a2V5"}}
    }})).unwrap()
}

#[test]
fn backend_tls_actual_fields_preserve_ordinal_kind_option_and_owner() {
    use crate::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
    let mut owner = policy();
    owner.spec.resolved_ca_certificates.as_mut().unwrap()[1]
        .metadata
        .annotations = Some([(CONTROLLER_CA_SOURCE.into(), CONTROLLER_CA_SOURCE_CONFIG_MAP.into())].into());
    let refs = BackendTlsRefs::from_policy(&owner);
    assert!(refs.all_materials_resolved());
    for (index, kind) in [(0, "Secret"), (1, "ConfigMap")] {
        let field = refs.ca(index).unwrap();
        let token = field.token();
        let bound = token.validated().unwrap();
        assert_eq!(bound.owner().name(), "policy");
        assert_eq!(bound.source().group_kind().kind(), "BackendTLSPolicy");
        assert_eq!(bound.target().group_kind().kind(), kind);
        assert_eq!(bound.target().scope().namespace(), Some("apps"));
        assert!(bound.is_backend_tls_material());
        assert!(!bound.is_gateway_tls());
        assert!(token.freeze().is_err());
        assert!(field.resolved());
    }
    let client = refs.client().unwrap().unwrap();
    assert_eq!(client.token().validated().unwrap().target().name(), "client");
    assert!(client.resolved());
    assert!(refs.ca(2).is_none());
    owner.spec.resolved_ca_certificates.as_mut().unwrap().swap(0, 1);
    assert!(BackendTlsRefs::from_policy(&owner).all_materials_resolved());
    owner.spec.validation.ca_certificate_refs.as_mut().unwrap().swap(0, 1);
    assert_eq!(
        BackendTlsRefs::from_policy(&owner)
            .ca(0)
            .unwrap()
            .token()
            .validated()
            .unwrap()
            .target()
            .group_kind()
            .kind(),
        "ConfigMap"
    );
}

#[test]
fn backend_tls_pairing_rejects_kind_decoys_missing_conflicts_and_client_substitution() {
    use crate::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
    let mut base = policy();
    base.spec.resolved_ca_certificates.as_mut().unwrap()[1]
        .metadata
        .annotations = Some([(CONTROLLER_CA_SOURCE.into(), CONTROLLER_CA_SOURCE_CONFIG_MAP.into())].into());
    assert!(BackendTlsRefs::from_policy(&base).all_materials_resolved());
    for case in [
        "missing-cm",
        "secret-decoy",
        "cm-marker",
        "ca-name",
        "ca-ns",
        "client-name",
        "client-ns",
        "client-marker",
        "client-empty",
        "surplus",
        "duplicate-declaration",
    ] {
        let mut owner = base.clone();
        match case {
            "missing-cm" => {
                owner.spec.resolved_ca_certificates.as_mut().unwrap().pop();
            }
            "secret-decoy" => {
                let s = owner.spec.resolved_ca_certificates.as_ref().unwrap()[0].clone();
                owner.spec.resolved_ca_certificates.as_mut().unwrap()[1] = s;
            }
            "cm-marker" => {
                owner.spec.resolved_ca_certificates.as_mut().unwrap()[1]
                    .metadata
                    .annotations = None
            }
            "ca-name" => owner.spec.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("decoy".into()),
            "ca-ns" => {
                owner.spec.resolved_ca_certificates.as_mut().unwrap()[0]
                    .metadata
                    .namespace = Some("elsewhere".into())
            }
            "client-name" => {
                owner.spec.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("decoy".into())
            }
            "client-ns" => {
                owner
                    .spec
                    .resolved_client_certificate
                    .as_mut()
                    .unwrap()
                    .metadata
                    .namespace = Some("elsewhere".into())
            }
            "client-marker" => {
                owner
                    .spec
                    .resolved_client_certificate
                    .as_mut()
                    .unwrap()
                    .metadata
                    .annotations = Some([(CONTROLLER_CA_SOURCE.into(), CONTROLLER_CA_SOURCE_CONFIG_MAP.into())].into())
            }
            "client-empty" => owner
                .spec
                .resolved_client_certificate
                .as_mut()
                .unwrap()
                .data
                .as_mut()
                .unwrap()
                .clear(),
            "surplus" => owner
                .spec
                .resolved_ca_certificates
                .as_mut()
                .unwrap()
                .push(Secret::default()),
            "duplicate-declaration" => {
                let r = owner.spec.validation.ca_certificate_refs.as_ref().unwrap()[0].clone();
                owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[1] = r;
            }
            _ => unreachable!(),
        }
        let refs = BackendTlsRefs::from_policy(&owner);
        assert!(!refs.all_materials_resolved(), "{case}");
        if case == "missing-cm" {
            assert!(refs.ca(0).unwrap().resolved());
            assert!(!refs.ca(1).unwrap().resolved());
        }
    }
}

#[test]
fn backend_tls_native_bounds_names_and_local_default_are_finite() {
    let mut owner = policy();
    owner.spec.options = None;
    owner.spec.resolved_client_certificate = None;
    owner.spec.validation.ca_certificate_refs.as_mut().unwrap().truncate(1);
    owner.spec.resolved_ca_certificates.as_mut().unwrap().truncate(1);
    owner.metadata.namespace = None;
    owner.spec.resolved_ca_certificates.as_mut().unwrap()[0]
        .metadata
        .namespace = None;
    assert!(BackendTlsRefs::from_policy(&owner).all_materials_resolved());
    for namespace in [Some(""), Some("other")] {
        let mut wrong = owner.clone();
        wrong.spec.resolved_ca_certificates.as_mut().unwrap()[0]
            .metadata
            .namespace = namespace.map(str::to_owned);
        assert!(!BackendTlsRefs::from_policy(&wrong).all_materials_resolved());
    }
    for name in ["Capital_Name".to_owned(), "é".repeat(253)] {
        owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].name = name.clone();
        owner.spec.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some(name);
        assert!(BackendTlsRefs::from_policy(&owner).all_materials_resolved());
    }
    owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].name = "é".repeat(254);
    assert!(BackendTlsRefs::from_policy(&owner)
        .ca(0)
        .unwrap()
        .token()
        .validated()
        .is_err());
    for count in [8, 9] {
        let mut bounded = policy();
        bounded.spec.options = None;
        bounded.spec.resolved_client_certificate = None;
        let reference = bounded.spec.validation.ca_certificate_refs.as_ref().unwrap()[0].clone();
        let material = bounded.spec.resolved_ca_certificates.as_ref().unwrap()[0].clone();
        bounded.spec.validation.ca_certificate_refs = Some(
            (0..count)
                .map(|i| {
                    let mut r = reference.clone();
                    r.name = format!("ca-{i}");
                    r
                })
                .collect(),
        );
        bounded.spec.resolved_ca_certificates = Some(
            (0..count)
                .map(|i| {
                    let mut s = material.clone();
                    s.metadata.name = Some(format!("ca-{i}"));
                    s
                })
                .collect(),
        );
        let refs = BackendTlsRefs::from_policy(&bounded);
        assert_eq!(refs.all_materials_resolved(), count == 8);
        assert_eq!(refs.ca(0).is_some(), count == 8);
    }
    let mut oversized = policy();
    oversized.spec.resolved_ca_certificates = Some(vec![Secret::default(); 4096]);
    let refs = BackendTlsRefs::from_policy(&oversized);
    assert!(!refs.bounded_materials());
    assert!(refs.ca(0).is_none());
    assert!(refs.client().is_err());
    assert!(!refs.all_materials_resolved());
}

#[test]
fn backend_tls_native_references_and_custom_option_keep_distinct_grammar() {
    for case in ["core", "kind", "name", "owner-name", "owner-ns"] {
        let mut owner = policy();
        match case {
            "core" => owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].group = "core".into(),
            "kind" => owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].kind = "Service".into(),
            "name" => owner.spec.validation.ca_certificate_refs.as_mut().unwrap()[0]
                .name
                .clear(),
            "owner-name" => owner.metadata.name = None,
            "owner-ns" => owner.metadata.namespace = Some(String::new()),
            _ => unreachable!(),
        }
        assert!(
            BackendTlsRefs::from_policy(&owner)
                .ca(0)
                .unwrap()
                .token()
                .validated()
                .is_err(),
            "{case}"
        );
    }
    for option in ["other/client", "Capital_Name", "", "bad..name"] {
        let mut owner = policy();
        owner
            .spec
            .options
            .as_mut()
            .unwrap()
            .insert("edgion.io/client-certificate-ref".into(), option.into());
        assert!(BackendTlsRefs::from_policy(&owner).client().is_err(), "{option}");
    }
    for field in ["group", "kind", "name"] {
        let mut wire = json!({"group":"","kind":"Secret","name":"ca"});
        wire.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<crate::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef>(wire)
                .is_err()
        );
    }
}
