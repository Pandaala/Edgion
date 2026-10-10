use super::*;
use crate::resources::refs::outbound_tls::TlsMaterialIdentityError;
use kube::api::ObjectMeta;

fn owner() -> EdgionBackendTrafficPolicy {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": "health", "namespace": "default"},
        "spec": {"targetRefs": [{"kind": "Service", "name": "svc"}],
            "healthCheck": {"active": {"type": "https", "tls": {
                "validation": {"hostname": "probe.example", "caCertificateRefs": [
                    {"kind": "Secret", "name": "ca"}, {"group": "core", "kind": "ConfigMap", "name": "ca"}]},
                "clientCertificateRef": {"name": "client"}}}}}
    }))
    .unwrap()
}
fn active(p: &mut EdgionBackendTrafficPolicy) -> &mut ActiveHealthCheckConfig {
    p.spec.health_check.as_mut().unwrap().active.as_mut().unwrap()
}
fn material(name: &str, configmap: bool) -> Secret {
    let mut value = Secret {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    if configmap {
        use crate::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
        value.metadata.annotations = Some(std::collections::BTreeMap::from([(
            CONTROLLER_CA_SOURCE.into(),
            CONTROLLER_CA_SOURCE_CONFIG_MAP.into(),
        )]));
    }
    value
}
#[test]
fn health_fields_bind_actual_ordinals_custom_defaults_and_material_provenance() {
    let mut p = owner();
    active(&mut p).resolved_ca_certificates = Some(vec![material("ca", false), material("ca", true)]);
    active(&mut p).resolved_client_certificate = Some(material("client", false));
    let check = |p: &EdgionBackendTrafficPolicy| {
        let refs = HealthTlsRefs::from_policy(p);
        let field = refs.field().unwrap().unwrap().outbound();
        let mut results = Vec::new();
        field.visit_ca(|pair| results.push(pair.validate_identity()));
        assert!(field.client_certificate().unwrap().validate_identity().is_ok());
        results
    };
    assert!(check(&p).iter().all(Result::is_ok));
    active(&mut p).resolved_ca_certificates.as_mut().unwrap().swap(0, 1);
    assert!(check(&p)
        .iter()
        .all(|r| matches!(r, Err(TlsMaterialIdentityError::Provenance))));
    active(&mut p).resolved_ca_certificates.as_mut().unwrap().swap(0, 1);
    active(&mut p).resolved_ca_certificates.as_mut().unwrap()[0]
        .metadata
        .name = Some("decoy".into());
    assert!(matches!(check(&p)[0], Err(TlsMaterialIdentityError::Identity)));
}
#[test]
fn health_selection_rejects_invalid_source_or_fields_and_never_activates_plaintext() {
    for protocol in [HealthCheckType::Http, HealthCheckType::Grpc, HealthCheckType::Tcp] {
        let mut p = owner();
        active(&mut p).r#type = protocol;
        active(&mut p).tls.as_mut().unwrap().verify = Some(false);
        assert!(HealthTlsRefs::from_policy(&p).field().unwrap().is_none());
    }
    for case in 0..6 {
        let mut p = owner();
        match case {
            0 => p.metadata.name = None,
            1 => p.metadata.namespace = Some(String::new()),
            2 => {
                active(&mut p)
                    .tls
                    .as_mut()
                    .unwrap()
                    .client_certificate_ref
                    .as_mut()
                    .unwrap()
                    .namespace = Some("other".into())
            }
            3 => {
                active(&mut p)
                    .tls
                    .as_mut()
                    .unwrap()
                    .client_certificate_ref
                    .as_mut()
                    .unwrap()
                    .kind = Some("ConfigMap".into())
            }
            4 => active(&mut p)
                .tls
                .as_mut()
                .unwrap()
                .validation
                .as_mut()
                .unwrap()
                .ca_certificate_refs
                .as_mut()
                .unwrap()[0]
                .name
                .clear(),
            _ => active(&mut p).tls = None,
        }
        assert!(HealthTlsRefs::from_policy(&p).field().is_err());
    }
}
#[test]
fn health_default_projection_is_finite_and_field_cannot_freeze() {
    let mut p = owner();
    p.metadata.namespace = None;
    let mut ca = material("ca", false);
    ca.metadata.namespace = None;
    active(&mut p).resolved_ca_certificates = Some(vec![ca, material("ca", true)]);
    let refs = HealthTlsRefs::from_policy(&p);
    let field = refs.field().unwrap().unwrap();
    field.visit_ca(|token| {
        let reference = token.validated().unwrap();
        assert!(reference.is_health_tls_material());
        assert!(!reference.is_gateway_tls());
        assert_eq!(reference.target().scope().namespace(), Some("default"));
        assert!(token.freeze().is_err());
    });
    field
        .outbound()
        .visit_ca(|pair| assert!(pair.validate_identity().is_ok()));
    p.metadata.namespace = Some("other".into());
    let refs = HealthTlsRefs::from_policy(&p);
    refs.field()
        .unwrap()
        .unwrap()
        .outbound()
        .visit_ca(|pair| assert!(pair.validate_identity().is_err()));
}
