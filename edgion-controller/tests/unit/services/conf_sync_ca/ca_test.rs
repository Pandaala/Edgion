use super::*;
use x509_parser::prelude::*;

/// Parse a PEM certificate, leaking the DER buffer so the returned
/// `X509Certificate` can carry a `'static` lifetime (test-only helper).
fn parse(pem: &str) -> X509Certificate<'static> {
    let der = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap().1;
    let boxed: &'static [u8] = Box::leak(der.contents.into_boxed_slice());
    X509Certificate::from_der(boxed).unwrap().1
}

#[test]
fn root_and_leaves_have_expected_shape() {
    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();

    // -- server leaf --
    let san_name = "edgion-controller.ns1.svc".to_string();
    let server = ca
        .sign_server(std::slice::from_ref(&san_name), ::time::Duration::days(365))
        .unwrap();
    let sc = parse(&server.cert_pem);

    // Real EKU assertion: server cert must carry serverAuth.
    let server_eku = sc
        .extended_key_usage()
        .expect("server EKU parse ok")
        .expect("server cert must have an EKU extension");
    assert!(server_eku.value.server_auth, "server cert EKU must include serverAuth");

    // SAN assertion: the requested DNS name must appear in the SAN extension.
    let san_ext = sc
        .subject_alternative_name()
        .expect("server SAN parse ok")
        .expect("server cert must have a SAN extension");
    let has_san = san_ext
        .value
        .general_names
        .iter()
        .any(|n| matches!(n, GeneralName::DNSName(name) if *name == san_name.as_str()));
    assert!(has_san, "server cert SAN must contain '{san_name}'");

    // -- client leaf --
    let client = ca.sign_client(::time::Duration::days(365)).unwrap();
    let cc = parse(&client.cert_pem);

    // Real EKU assertion: client cert must carry clientAuth.
    let client_eku = cc
        .extended_key_usage()
        .expect("client EKU parse ok")
        .expect("client cert must have an EKU extension");
    assert!(client_eku.value.client_auth, "client cert EKU must include clientAuth");

    // CN assertion.
    assert!(cc.subject().to_string().contains("edgion-gateway"));

    // Both leaf private keys must be non-empty PEM.
    assert!(server.key_pem.starts_with("-----BEGIN"));
    assert!(client.key_pem.starts_with("-----BEGIN"));
}

#[test]
fn load_from_pem_reissues_against_same_root() {
    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
    let reloaded = Ca::load_from_pem(ca.ca_cert_pem(), ca.ca_key_pem()).unwrap();
    // A leaf signed by the reloaded CA must chain to the original root.
    let leaf = reloaded.sign_client(::time::Duration::days(30)).unwrap();
    // Chain check: the leaf's issuer DN must equal the root's subject DN.
    let root = parse(ca.ca_cert_pem());
    let l = parse(&leaf.cert_pem);
    assert_eq!(l.issuer().to_string(), root.subject().to_string());
}

#[test]
fn leaves_are_backdated_five_minutes() {
    use crate::services::conf_sync_ca::cert_util::{parse_cert_window, BACKDATE_SLACK};
    let ca = Ca::generate_root(::time::Duration::days(365)).unwrap();
    let leaf = ca.sign_client(::time::Duration::days(30)).unwrap();
    let (nb, _) = parse_cert_window(&leaf.cert_pem).unwrap();
    let now = ::time::OffsetDateTime::now_utc();
    assert!(nb <= now - BACKDATE_SLACK + ::time::Duration::seconds(30));
    assert!(nb >= now - BACKDATE_SLACK - ::time::Duration::seconds(30));
}

#[test]
fn renew_root_same_key_keeps_trust() {
    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
    let renewed = ca.renew_root_same_key(::time::Duration::days(3650)).unwrap();
    // Same public key: a leaf signed by the RENEWED root verifies against the OLD root cert's key.
    let leaf = renewed.sign_client(::time::Duration::days(30)).unwrap();
    assert!(edgion_common::grpc_tls::validate_identity(
        leaf.cert_pem.as_bytes(),
        leaf.key_pem.as_bytes(),
        ca.ca_cert_pem().as_bytes(), // OLD root cert as trust anchor
    )
    .is_ok());
    // And the window actually moved forward.
    use crate::services::conf_sync_ca::cert_util::parse_cert_window;
    let (_, old_na) = parse_cert_window(ca.ca_cert_pem()).unwrap();
    let (_, new_na) = parse_cert_window(renewed.ca_cert_pem()).unwrap();
    assert!(new_na >= old_na);
}

#[test]
fn unrepresentable_configured_lifetimes_return_errors() {
    use edgion_common::conf_sync::{cert_duration::CertDuration, config::ConfSyncRotationConfig};

    // A valid duration need not fit in an absolute certificate timestamp.
    let config = ConfSyncRotationConfig {
        ca_ttl: CertDuration::parse("10000000y").unwrap(),
        ..Default::default()
    };
    config.validate_numeric().unwrap();
    let ttl = config.ca_ttl.as_time();
    assert!(Ca::generate_root(ttl).is_err());

    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
    assert!(ca.renew_root_same_key(ttl).is_err());
    assert!(ca.sign_server(&["localhost".to_string()], ttl).is_err());
    assert!(ca.sign_client(ttl).is_err());
}
