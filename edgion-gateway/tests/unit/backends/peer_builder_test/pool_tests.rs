//! Pool isolation regressions for `peer_builder.rs` (see `super`).

use super::*;
use k8s_openapi::{api::core::v1::Secret, ByteString};
use pingora_core::upstreams::peer::Peer;
use std::collections::BTreeMap;

fn policy() -> BackendTLSPolicy {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "BackendTLSPolicy",
        "metadata": {"name": "pool", "namespace": "test"},
        "spec": {
            "targetRefs": [{"group": "", "kind": "Service", "name": "backend"}],
            "validation": {"hostname": "backend.example.com"},
            "useSystemCa": true
        }
    }))
    .unwrap()
}

fn certificate(key: &rcgen::KeyPair, name: &str) -> String {
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).unwrap();
    params.serial_number = Some(42u64.into());
    params.self_signed(key).unwrap().pem()
}

fn secret(entries: &[(&str, &str)]) -> Secret {
    Secret {
        data: Some(BTreeMap::from_iter(
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), ByteString(value.as_bytes().to_vec()))),
        )),
        ..Default::default()
    }
}

fn peer(policy: BackendTLSPolicy, gateway: Option<&str>) -> Box<HttpPeer> {
    create_tls_peer(
        SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
        &EffectiveBackendTlsPolicy::from_policy(Arc::new(policy)),
        gateway,
    )
    .unwrap()
}

fn assert_isolated(first: &HttpPeer, second: &HttpPeer) {
    assert_ne!(first.group_key, second.group_key);
    assert_ne!(first.reuse_hash(), second.reuse_hash());
    // Removing Edgion's discriminator reproduces the original collision:
    // addr, SNI and (for mTLS) leaf serial are identical in these fixtures.
    let mut first = first.clone();
    let mut second = second.clone();
    first.group_key = 0;
    second.group_key = 0;
    assert_eq!(first.reuse_hash(), second.reuse_hash());
}

#[test]
fn pool_isolation_covers_ca_mode_ca_content_and_san_composition() {
    let key = rcgen::KeyPair::generate().unwrap();
    let ca_a = certificate(&key, "ca-a.example.com");
    let ca_b = certificate(&key, "ca-b.example.com");
    let system = policy();
    let mut custom = system.clone();
    custom.spec.use_system_ca = false;
    custom.spec.resolved_ca_certificates = Some(vec![secret(&[(CA_CERT, &ca_a)])]);
    assert_isolated(&peer(system, None), &peer(custom.clone(), None));

    // Exercise CA changes both with and without the additional SAN hook.
    for sans in [
        None,
        Some(
            serde_json::from_value(serde_json::json!([
                {"type": "Hostname", "hostname": "backend.example.com"}
            ]))
            .unwrap(),
        ),
    ] {
        custom.spec.validation.subject_alt_names = sans;
        let mut rotated = custom.clone();
        rotated.spec.resolved_ca_certificates = Some(vec![secret(&[(CA_CERT, &ca_b)])]);
        assert_isolated(&peer(custom.clone(), None), &peer(rotated, None));
    }
    let with_sans = peer(custom.clone(), None);
    custom.spec.validation.subject_alt_names = None;
    assert_isolated(&with_sans, &peer(custom.clone(), None));
    assert_eq!(peer(custom.clone(), None).reuse_hash(), peer(custom, None).reuse_hash());
}

#[test]
fn pool_isolation_covers_same_serial_leaf_chain_and_private_key() {
    let key = rcgen::KeyPair::generate().unwrap();
    let other_key = rcgen::KeyPair::generate().unwrap();
    let leaf = certificate(&key, "client-a.example.com");
    let other_leaf = certificate(&key, "client-b.example.com");
    let intermediate = certificate(&other_key, "intermediate.example.com");
    let chain = format!("{leaf}{intermediate}");
    let identity = secret(&[(CERT, &chain), (KEY, &key.serialize_pem())]);
    let mut base = policy();
    base.spec.resolved_client_certificate = Some(identity.clone());
    for (label, cert, private_key) in [
        (
            "same serial leaf",
            format!("{other_leaf}{intermediate}"),
            key.serialize_pem(),
        ),
        (
            "full chain",
            format!("{leaf}{}", certificate(&other_key, "other-intermediate.example.com")),
            key.serialize_pem(),
        ),
        // Deliberately isolate the private-key input. This is a fingerprint
        // boundary test, not a claim that a mismatched identity can handshake.
        ("private key", chain, other_key.serialize_pem()),
    ] {
        let mut rotated = base.clone();
        rotated.spec.resolved_client_certificate = Some(secret(&[(CERT, &cert), (KEY, &private_key)]));
        let first = peer(base.clone(), None);
        let second = peer(rotated, None);
        assert_eq!(
            first
                .client_cert_key
                .as_ref()
                .unwrap()
                .leaf()
                .serial_number()
                .to_bn()
                .unwrap(),
            second
                .client_cert_key
                .as_ref()
                .unwrap()
                .leaf()
                .serial_number()
                .to_bn()
                .unwrap(),
            "{label} must retain the same serial"
        );
        assert_isolated(&first, &second);
    }
    assert_eq!(peer(base.clone(), None).reuse_hash(), peer(base, None).reuse_hash());
}

struct GatewayIdentityGuard(&'static str);

impl Drop for GatewayIdentityGuard {
    fn drop(&mut self) {
        get_global_gateway_backend_tls_store().remove(self.0);
    }
}

#[test]
fn pool_isolation_composes_gateway_rotation_and_policy_precedence() {
    let gateway = GatewayIdentityGuard("test/pool-isolation-gateway");
    let store = get_global_gateway_backend_tls_store();
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = certificate(&key, "client.example.com");
    let extra = certificate(&key, "intermediate.example.com");
    let chain = format!("{leaf}{extra}");
    let identity = secret(&[(CERT, &chain), (KEY, &key.serialize_pem())]);
    let base = policy();
    let anonymous = peer(base.clone(), Some(gateway.0));
    store.update_from_gateway(
        gateway.0,
        &super::tests::gateway_identity_owner("test", "pool-isolation-gateway", identity.clone()),
    );
    let fallback = peer(base.clone(), Some(gateway.0));
    assert_ne!(anonymous.reuse_hash(), fallback.reuse_hash());
    assert_eq!(fallback.reuse_hash(), peer(base.clone(), Some(gateway.0)).reuse_hash());
    let mut policy_identity = base.clone();
    policy_identity.spec.resolved_client_certificate = Some(identity);
    let explicit = peer(policy_identity.clone(), Some(gateway.0));
    // Sources may be separated even when material is identical; they must
    // never collide when their effective identity differs.
    assert_isolated(&fallback, &explicit);
    for rotated in [
        secret(&[
            (CERT, &format!("{}{extra}", certificate(&key, "rotated.example.com"))),
            (KEY, &key.serialize_pem()),
        ]),
        secret(&[
            (
                CERT,
                &format!("{leaf}{}", certificate(&key, "other-intermediate.example.com")),
            ),
            (KEY, &key.serialize_pem()),
        ]),
        secret(&[
            (CERT, &chain),
            (KEY, &rcgen::KeyPair::generate().unwrap().serialize_pem()),
        ]),
    ] {
        store.update_from_gateway(
            gateway.0,
            &super::tests::gateway_identity_owner("test", "pool-isolation-gateway", rotated),
        );
        assert_isolated(&fallback, &peer(base.clone(), Some(gateway.0)));
        assert_eq!(
            explicit.reuse_hash(),
            peer(policy_identity.clone(), Some(gateway.0)).reuse_hash()
        );
    }
    store.remove(gateway.0);
    assert_eq!(anonymous.reuse_hash(), peer(base, Some(gateway.0)).reuse_hash());
}
