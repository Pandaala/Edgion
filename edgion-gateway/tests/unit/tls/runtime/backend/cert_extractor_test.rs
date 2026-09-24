use super::{encode_fingerprint, extract_client_cert_info_from_cert, extract_sans};
use pingora_core::tls::hash::MessageDigest;
use pingora_core::tls::x509::X509;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

fn canonical_client_cert() -> X509 {
    let key = KeyPair::generate().expect("test key generation must succeed");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SAN list must be valid");
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "client-cn");
    params.subject_alt_names = vec![
        SanType::DnsName("client.example".try_into().expect("test DNS SAN must be valid")),
        SanType::IpAddress("127.0.0.1".parse().expect("test IP SAN must be valid")),
    ];
    let pem = params
        .self_signed(&key)
        .expect("test certificate signing must succeed")
        .pem();
    X509::from_pem(pem.as_bytes()).expect("generated certificate must parse")
}

fn subject_only_cert() -> X509 {
    let key = KeyPair::generate().expect("test key generation must succeed");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SAN list must be valid");
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::OrganizationName, "subject-only-org");
    params.distinguished_name = distinguished_name;
    let pem = params
        .self_signed(&key)
        .expect("test certificate signing must succeed")
        .pem();
    X509::from_pem(pem.as_bytes()).expect("generated certificate must parse")
}

#[test]
fn extracts_dns_and_ip_subject_alt_names() {
    let cert = canonical_client_cert();

    let sans = extract_sans(&cert);
    assert!(sans.iter().any(|san| san == "client.example"));
    assert!(sans.iter().any(|san| san == "IP:127.0.0.1"));
}

#[test]
fn canonical_projection_extracts_cn_sans_and_sha256_fingerprint() {
    let cert = canonical_client_cert();
    let expected_fingerprint = encode_fingerprint(
        cert.digest(MessageDigest::sha256())
            .expect("generated certificate digest must succeed")
            .as_ref(),
    );

    let info = extract_client_cert_info_from_cert(&cert);

    assert_eq!(info.subject, "CN=client-cn");
    assert_eq!(info.cn.as_deref(), Some("client-cn"));
    assert_eq!(info.sans, ["client.example", "IP:127.0.0.1"]);
    assert_eq!(info.fingerprint, expected_fingerprint);
}

#[test]
fn canonical_projection_does_not_invent_cn_or_san_from_subject() {
    let info = extract_client_cert_info_from_cert(&subject_only_cert());

    assert!(!info.subject.is_empty());
    assert!(info.sans.is_empty());
    assert!(info.cn.is_none());
}

#[test]
fn fingerprint_encoding_preserves_colon_separated_lower_hex() {
    assert_eq!(encode_fingerprint(&[]), "");
    assert_eq!(encode_fingerprint(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "00:0f:10:ab:ff");
}

fn other_name_explicit(inner: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(inner.len() + 2);
    encoded.push(0xa0);
    encoded.push(u8::try_from(inner.len()).expect("test OtherName payload fits short-form length"));
    encoded.extend_from_slice(inner);
    encoded
}

#[test]
fn other_name_utf8_string_decodes_for_matching() {
    let utf8_alice = [0x0c, 0x05, b'a', b'l', b'i', b'c', b'e'];
    assert_eq!(
        super::decode_other_name_value(&utf8_alice),
        None,
        "inner type without [0] EXPLICIT is skipped"
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&utf8_alice)).as_deref(),
        Some("alice")
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x01, 0x01, 0xff])).as_deref(),
        Some("true")
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x05, 0x00])).as_deref(),
        Some("")
    );
    assert_eq!(
        super::decode_other_name_value(b"alice"),
        None,
        "raw UTF-8 without BER must not match"
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x04, 0x05, b'a', b'l', b'i', b'c', b'e'])),
        None,
        "OCTET STRING is not a string type"
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x03, 0x02, 0x00, b'a'])),
        None,
        "BIT STRING is not a string type"
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x0d, 0x03, 0x01, 0x02, 0x03])),
        None,
        "RELATIVE-OID is not OBJECT IDENTIFIER"
    );
    assert_eq!(
        super::decode_other_name_value(&[0xa1, 0x07, 0x0c, 0x05, b'a', b'l', b'i', b'c', b'e']),
        None,
        "non-[0] context tags are not unwrapped"
    );
    assert_eq!(
        super::decode_other_name_value(&[0x80, 0x07, 0x0c, 0x05, b'a', b'l', b'i', b'c', b'e']),
        None,
        "primitive [0] is not EXPLICIT"
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[
            0x02, 0x09, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
        ]))
        .as_deref(),
        Some("18446744073709551616")
    );
    let bmp_embedded_nul = [0x1e, 0x06, 0x00, b'a', 0x00, 0x00, 0x00, b'b'];
    let decoded = super::decode_other_name_value(&other_name_explicit(&bmp_embedded_nul)).expect("BMPString");
    assert_eq!(decoded, "a\u{0}b");
    assert_ne!(decoded, "a");
}
