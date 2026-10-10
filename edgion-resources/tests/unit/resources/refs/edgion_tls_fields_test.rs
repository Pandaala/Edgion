use super::*;
use kube::api::ObjectMeta;

fn owner() -> EdgionTls {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "default"},
        "spec": {"hosts": ["edge.example"], "secretRef": {"name": "server"},
            "clientAuth": {"mode": "Mutual", "caSecretRef": {"name": "ca"}}}
    }))
    .unwrap()
}
fn secret(name: &str) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn actual_server_and_ca_proofs_are_independent_and_cannot_freeze() {
    let mut tls = owner();
    tls.spec.secret = Some(secret("server"));
    tls.spec.client_auth.as_mut().unwrap().ca_secret = Some(secret("ca"));
    let refs = EdgionTlsRefs::from_tls(&tls);
    assert!(refs.server().material().is_ok());
    assert!(refs.client_ca().unwrap().material().is_ok());
    assert!(refs.server().token().freeze().is_err());
    assert!(refs.client_ca().unwrap().token().freeze().is_err());
    tls.spec
        .client_auth
        .as_mut()
        .unwrap()
        .ca_secret_ref
        .as_mut()
        .unwrap()
        .group = Some("core".into());
    let refs = EdgionTlsRefs::from_tls(&tls);
    assert!(refs.server().material().is_ok());
    assert!(refs.client_ca().unwrap().material().is_err());
    tls.spec
        .client_auth
        .as_mut()
        .unwrap()
        .ca_secret_ref
        .as_mut()
        .unwrap()
        .group = None;
    tls.spec.secret = Some(secret("ca"));
    let refs = EdgionTlsRefs::from_tls(&tls);
    assert!(refs.server().material().is_err());
    assert!(refs.client_ca().unwrap().material().is_ok());
}
#[test]
fn strict_edgion_tls_fields_reject_alias_invalid_source_and_material_provenance() {
    for case in 0..10 {
        let mut tls = owner();
        tls.spec.secret = Some(secret("server"));
        match case {
            0 => tls.spec.secret_ref.group = Some("core".into()),
            1 => tls.spec.secret_ref.kind = Some("ConfigMap".into()),
            2 => tls.spec.secret_ref.name = "bad/name".into(),
            3 => tls.spec.secret_ref.namespace = Some(String::new()),
            4 => tls.metadata.name = None,
            5 => tls.metadata.namespace = Some(String::new()),
            6 => tls.spec.secret.as_mut().unwrap().metadata.name = Some("decoy".into()),
            7 => tls.spec.secret.as_mut().unwrap().metadata.namespace = Some(String::new()),
            8 => tls.spec.secret.as_mut().unwrap().metadata.namespace = Some("other".into()),
            _ => {
                use crate::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
                tls.spec.secret.as_mut().unwrap().metadata.annotations = Some(std::collections::BTreeMap::from([(
                    CONTROLLER_CA_SOURCE.into(),
                    CONTROLLER_CA_SOURCE_CONFIG_MAP.into(),
                )]));
            }
        }
        assert!(
            EdgionTlsRefs::from_tls(&tls).server().material().is_err(),
            "case {case}"
        );
    }
}
#[test]
fn strict_edgion_tls_default_namespace_projection_preserves_actual_metadata() {
    for source in [None, Some("default"), Some("apps")] {
        for material in [None, Some("default"), Some("apps"), Some("")] {
            let mut tls = owner();
            tls.metadata.namespace = source.map(str::to_owned);
            tls.spec.secret = Some(secret("server"));
            tls.spec.secret.as_mut().unwrap().metadata.namespace = material.map(str::to_owned);
            tls.spec.secret_ref.group = Some(String::new());
            tls.spec.secret_ref.kind = Some("Secret".into());
            assert_eq!(
                EdgionTlsRefs::from_tls(&tls).server().material().is_ok(),
                source.unwrap_or("default") == material.unwrap_or("default")
            );
            assert_eq!(
                tls.spec.secret.as_ref().unwrap().metadata.namespace.as_deref(),
                material
            );
        }
    }
}
