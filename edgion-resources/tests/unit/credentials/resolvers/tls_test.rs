use super::*;

fn mk(data: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
    data.iter()
        .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
        .collect()
}

#[test]
fn parse_full_kubernetes_tls_secret() {
    let data = mk(&[("tls.crt", b"CERT"), ("tls.key", b"KEY"), ("ca.crt", b"CA")]);
    let m = TlsCertsResolver.parse(&data).unwrap();
    assert_eq!(m.ca_pem.as_deref(), Some(b"CA".as_ref()));
    assert_eq!(m.client_cert_pem.as_deref(), Some(b"CERT".as_ref()));
    assert_eq!(m.client_key_pem.as_ref().map(|z| z.as_slice()), Some(b"KEY".as_ref()));
}

#[test]
fn parse_ca_only() {
    let data = mk(&[("ca.crt", b"CA")]);
    let m = TlsCertsResolver.parse(&data).unwrap();
    assert!(m.ca_pem.is_some());
    assert!(m.client_cert_pem.is_none());
    assert!(m.client_key_pem.is_none());
}

#[test]
fn parse_mtls_without_ca() {
    let data = mk(&[("tls.crt", b"CERT"), ("tls.key", b"KEY")]);
    let m = TlsCertsResolver.parse(&data).unwrap();
    assert!(m.ca_pem.is_none());
    assert!(m.client_cert_pem.is_some());
    assert!(m.client_key_pem.is_some());
}

#[test]
fn parse_empty_reports_all_candidates() {
    let data = BTreeMap::new();
    match TlsCertsResolver.parse(&data) {
        Err(ResolveError::MissingKeys(keys)) => {
            assert_eq!(keys, vec!["tls.crt", "tls.key", "ca.crt"]);
        }
        other => panic!("expected MissingKeys, got {other:?}"),
    }
}

#[test]
fn parse_preserves_binary_content_verbatim() {
    // TLS material is bytes; non-UTF8 is legal (DER, etc. — even though we
    // advertise PEM, the resolver should not try to validate).
    let data = mk(&[("tls.key", &[0x00, 0xff, 0x7f][..])]);
    let m = TlsCertsResolver.parse(&data).unwrap();
    assert_eq!(m.client_key_pem.as_ref().unwrap().as_slice(), &[0x00, 0xff, 0x7f]);
}

#[test]
fn allows_k8s_tls_and_opaque_types() {
    let allowed = TlsCertsResolver.allowed_secret_types();
    assert!(allowed.contains(&"kubernetes.io/tls"));
    assert!(allowed.contains(&"Opaque"));
    assert!(allowed.contains(&""));
}

#[test]
fn required_keys_match_constants() {
    assert_eq!(
        TlsCertsResolver.required_keys(),
        &[TLS_CERT_KEY, TLS_KEY_KEY, TLS_CA_KEY]
    );
}
