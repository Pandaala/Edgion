use super::*;
use std::collections::BTreeMap;
use std::sync::Arc;

fn fingerprint(secret: &Secret) -> [u8; 32] {
    let mut hasher = FingerprintHasher::new();
    hash_secret_data(&mut hasher, secret);
    hasher.finish()
}

#[test]
fn material_identity_changes_hash_but_uid_and_rv_do_not() {
    let mut material = Secret::default();
    material.metadata.name = Some("ca".into());
    material.metadata.namespace = Some("default".into());
    material.data = Some(BTreeMap::from([(
        "ca.crt".into(),
        k8s_openapi::ByteString(b"certificate".to_vec()),
    )]));
    let original = fingerprint(&material);
    for changed in ["name", "namespace", "marker", "data"] {
        let mut candidate = material.clone();
        match changed {
            "name" => candidate.metadata.name = Some("other".into()),
            "namespace" => candidate.metadata.namespace = Some("other".into()),
            "marker" => {
                candidate.metadata.annotations =
                    Some(BTreeMap::from([(CONTROLLER_CA_SOURCE.into(), "ConfigMap".into())]))
            }
            "data" => candidate.data.as_mut().unwrap().get_mut("ca.crt").unwrap().0.push(0),
            _ => unreachable!(),
        }
        assert_ne!(fingerprint(&candidate), original, "{changed}");
    }
    material.metadata.uid = Some("uid".into());
    material.metadata.resource_version = Some("42".into());
    material.metadata.annotations = Some(BTreeMap::from([("operator.example/annotation".into(), "value".into())]));
    assert_eq!(fingerprint(&material), original);
}

#[test]
fn exact_source_marker_values_have_distinct_hashes() {
    let mut material = Secret::default();
    let mut hashes = vec![fingerprint(&material)];
    for value in ["", "ConfigMap", "Secret", "unknown"] {
        material.metadata.annotations = Some(BTreeMap::from([(CONTROLLER_CA_SOURCE.into(), value.into())]));
        hashes.push(fingerprint(&material));
    }
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), 5);
}

fn inherited_hash(local: Option<&OutboundTlsLocal>, snapshot: &GlobalOutboundTlsSnapshot) -> [u8; 32] {
    let mut hash = FingerprintHasher::new();
    hash_tls_inheritance(&mut hash, local, snapshot);
    hash.finish()
}

#[test]
fn inherited_encoder_masks_only_the_overridden_axis_and_native_inactive_transport() {
    use super::super::outbound_tls::test_util;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let empty = test_util::snapshot(OutboundTlsConfig::default());
    let verify = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let ca = test_util::snapshot(
        serde_json::from_value(serde_json::json!({
            "validation":{"wellKnownCACertificates":"System"}
        }))
        .unwrap(),
    );
    let client = test_util::snapshot(
        serde_json::from_value(serde_json::json!({
            "clientCertificateRef":{"name":"client", "namespace":"apps"}
        }))
        .unwrap(),
    );
    let inherit = OutboundTlsLocal::default();
    for changed in [&verify, &ca, &client] {
        assert_ne!(
            inherited_hash(Some(&inherit), &empty),
            inherited_hash(Some(&inherit), changed)
        );
        assert_eq!(inherited_hash(None, &empty), inherited_hash(None, changed));
    }
    for (local, masked) in [
        (serde_json::json!({"verify":true}), &verify),
        (serde_json::json!({"validation":{}}), &ca),
        (serde_json::json!({"clientCertificateRef":{"name":"local"}}), &client),
    ] {
        let local: OutboundTlsLocal = serde_json::from_value(local).unwrap();
        assert_eq!(
            inherited_hash(Some(&local), &empty),
            inherited_hash(Some(&local), masked)
        );
        for unmasked in [&verify, &ca, &client] {
            if !Arc::ptr_eq(masked, unmasked) {
                assert_ne!(
                    inherited_hash(Some(&local), &empty),
                    inherited_hash(Some(&local), unmasked)
                );
            }
        }
    }
    assert_ne!(inherited_hash(None, &empty), inherited_hash(Some(&inherit), &empty));
}
