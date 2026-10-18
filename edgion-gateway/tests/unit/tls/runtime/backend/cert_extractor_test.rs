use super::{encode_fingerprint, extract_client_cert_info_from_cert, extract_presented_sans, extract_sans};
use edgion_common::allowed_san::PresentedSan;
use pingora_core::tls::hash::MessageDigest;
use pingora_core::tls::x509::X509;
use rcgen::{CertificateParams, CustomExtension, DistinguishedName, DnType, KeyPair, SanType};

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

fn cert_with_subject(name: &pingora_core::tls::x509::X509NameRef) -> X509 {
    let template = canonical_client_cert();
    let key = KeyPair::generate().unwrap();
    let key = pingora_core::tls::pkey::PKey::private_key_from_der(&key.serialize_der()).unwrap();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_serial_number(template.serial_number()).unwrap();
    builder.set_not_before(template.not_before()).unwrap();
    builder.set_not_after(template.not_after()).unwrap();
    builder.set_subject_name(name).unwrap();
    builder.set_issuer_name(name).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();
    builder.build()
}

#[test]
fn subject_order_and_first_cn_survive_duplicate_common_names() {
    use pingora_core::tls::nid::Nid;
    let mut name = pingora_core::tls::x509::X509Name::builder().unwrap();
    name.append_entry_by_nid(Nid::ORGANIZATIONNAME, "example").unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "first\0cn").unwrap();
    name.append_entry_by_nid(Nid::ORGANIZATIONALUNITNAME, "clients")
        .unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "second").unwrap();
    let info = extract_client_cert_info_from_cert(&cert_with_subject(&name.build()));
    // BoringSSL's OpensslString exposes a C string, preserving the existing
    // subject/CN truncation at NUL (unlike the OtherName BMP projection).
    assert_eq!(info.subject, "O=example, CN=first, OU=clients, CN=second");
    assert_eq!(info.cn.as_deref(), Some("first"));
}

#[test]
fn unsupported_first_cn_does_not_promote_later_common_name() {
    use boring::asn1::Asn1Type;
    use pingora_core::tls::nid::Nid;
    let mut name = pingora_core::tls::x509::X509Name::builder().unwrap();
    name.append_entry_by_nid_with_type(Nid::COMMONNAME, "unsupported", Asn1Type::GENERALSTRING)
        .unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "second").unwrap();
    let name = name.build();
    assert!(name.entries().next().unwrap().data().as_utf8().is_err());
    let info = extract_client_cert_info_from_cert(&cert_with_subject(&name));
    assert_eq!(info.subject, "CN=?, CN=second");
    assert!(info.cn.is_none());
}

#[test]
fn bmp_decoding_preserves_utf16_boundaries() {
    assert_eq!(super::bmp_string(&[]).as_deref(), Some(""));
    assert_eq!(super::bmp_string(&[0, b'a', 0, 0, 0, 0xe9]).as_deref(), Some("a\0é"));
    assert_eq!(super::bmp_string(&[0xd8, 0x34, 0xdd, 0x1e]).as_deref(), Some("𝄞"));
    for invalid in [&[0][..], &[0xd8, 0x34], &[0xdd, 0x1e], &[0xdd, 0x1e, 0xd8, 0x34]] {
        assert_eq!(super::bmp_string(invalid), None);
    }
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

fn san_der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut encoded = vec![tag];
    if content.len() < 128 {
        encoded.push(content.len() as u8);
    } else {
        encoded.extend_from_slice(&[
            0x81,
            u8::try_from(content.len()).expect("fixture fits one-byte DER length"),
        ]);
    }
    encoded.extend_from_slice(content);
    encoded
}

fn other_name_der(oid: &[u8], inner: &[u8]) -> Vec<u8> {
    let mut content = san_der(0x06, oid);
    content.extend(other_name_explicit(inner));
    san_der(0xa0, &content)
}

fn cert_with_raw_sans(names: &[Vec<u8>]) -> X509 {
    let key = KeyPair::generate().expect("test key generation must succeed");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("empty SAN list must be valid");
    params.custom_extensions.push(CustomExtension::from_oid_content(
        &[2, 5, 29, 17],
        san_der(0x30, &names.concat()),
    ));
    let certificate = params.self_signed(&key).expect("test certificate signing must succeed");
    X509::from_der(certificate.der()).expect("generated certificate must parse")
}

#[test]
fn mixed_certificate_preserves_both_san_projections_and_two_pass_order() {
    // OtherNames precede ordinary names in DER, but extraction appends them.
    let cert = cert_with_raw_sans(&[
        other_name_der(&[0x2a, 0x03, 0x04], &[0x0c, 0x03, b'a', 0, b'b']),
        san_der(0x82, b"client.example"),
        san_der(0x87, &[127, 0, 0, 1]),
        other_name_der(&[0x2a, 0x03, 0x05], &[0x1e, 0x06, 0, b'x', 0, 0, 0, b'y']),
        san_der(0x81, b"client@example.test"),
        san_der(0x86, b"spiffe://example/client"),
        san_der(0x87, &std::net::Ipv6Addr::LOCALHOST.octets()),
        san_der(0x82, b"client.example"),
        other_name_der(&[0x2a, 0x03, 0x04], &[0x0c, 0x03, b'a', 0, b'b']),
        other_name_der(&[0x2a, 0x03, 0x06], &[0x04, 0x01, b'z']),
    ]);

    assert_eq!(
        extract_sans(&cert),
        [
            "client.example",
            "IP:127.0.0.1",
            "email:client@example.test",
            "uri:spiffe://example/client",
            "IP:::1",
            "client.example",
            "otherName:1.2.3.4=a\0b",
            "otherName:1.2.3.5=x\0y",
            "otherName:1.2.3.4=a\0b",
        ]
    );
    assert_eq!(
        extract_presented_sans(&cert),
        vec![
            PresentedSan::dns("client.example"),
            PresentedSan::ip("127.0.0.1"),
            PresentedSan::email("client@example.test"),
            PresentedSan::uri("spiffe://example/client"),
            PresentedSan::ip("::1"),
            PresentedSan::dns("client.example"),
            PresentedSan::other_name("1.2.3.4", "a\0b"),
            PresentedSan::other_name("1.2.3.5", "x\0y"),
            PresentedSan::other_name("1.2.3.4", "a\0b"),
        ]
    );
}

#[test]
fn invalid_ip_and_absent_sans_produce_no_invented_identities() {
    let cert = cert_with_raw_sans(&[san_der(0x87, &[127, 0, 1]), san_der(0x82, b"valid.example")]);
    assert_eq!(extract_sans(&cert), ["valid.example"]);
    assert_eq!(extract_presented_sans(&cert), [PresentedSan::dns("valid.example")]);

    let cert = subject_only_cert();
    assert!(extract_sans(&cert).is_empty());
    assert!(extract_presented_sans(&cert).is_empty());
}

#[test]
fn other_name_decode_rejects_trailing_incomplete_and_oversized_integer_values() {
    let mut trailing = other_name_explicit(&[0x0c, 0x01, b'a']);
    trailing.push(0);
    assert_eq!(super::decode_other_name_value(&trailing), None);
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x0c, 0x02, b'a'])),
        None
    );
    assert_eq!(
        super::decode_other_name_value(&other_name_explicit(&[0x0c, 0x01, b'a', 0x05, 0x00])),
        None
    );
    let mut integer = vec![0x02, 65];
    integer.extend([1; 65]);
    assert_eq!(super::decode_other_name_value(&other_name_explicit(&integer)), None);
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

#[test]
fn ip_san_projections_preserve_canonical_address_text() {
    let addresses = [
        ("0.0.0.0", "IP:0.0.0.0"),
        ("255.255.255.255", "IP:255.255.255.255"),
        ("::", "IP:::"),
        ("::1", "IP:::1"),
        ("2001:db8::1", "IP:2001:db8::1"),
        ("::ffff:192.0.2.1", "IP:::ffff:192.0.2.1"),
        (
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            "IP:ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        ),
    ];
    let names: Vec<_> = addresses
        .iter()
        .map(|(text, _)| match text.parse::<std::net::IpAddr>().unwrap() {
            std::net::IpAddr::V4(ip) => san_der(0x87, &ip.octets()),
            std::net::IpAddr::V6(ip) => san_der(0x87, &ip.octets()),
        })
        .collect();
    let cert = cert_with_raw_sans(&names);
    assert_eq!(extract_sans(&cert), addresses.map(|(_, display)| display));
    assert_eq!(
        extract_presented_sans(&cert),
        addresses.map(|(text, _)| PresentedSan::ip(text))
    );
}

#[test]
fn ip_display_projection_avoids_intermediate_address_string() {
    for text in ["255.255.255.255", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"] {
        let address = text.parse::<std::net::IpAddr>().unwrap();
        let (old, old_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            let value = match address {
                std::net::IpAddr::V4(ip) => {
                    let bytes = ip.octets();
                    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
                }
                std::net::IpAddr::V6(ip) => ip.to_string(),
            };
            format!("IP:{value}")
        });
        let (new, new_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| super::ClassifiedSan::Ip(address).into_display());
        assert_eq!(new, old);
        assert!(
            new_stats.calls < old_stats.calls,
            "old={old_stats:?}, new={new_stats:?}"
        );
        assert!(
            new_stats.requested_bytes < old_stats.requested_bytes,
            "old={old_stats:?}, new={new_stats:?}"
        );
    }
}
