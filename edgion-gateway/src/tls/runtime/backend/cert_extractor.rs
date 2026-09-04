//! Client certificate extraction utilities.
//!
//! Extracts client certificate information from SSL connections after TLS handshake.
//! Requires BoringSSL for X.509 certificate access.

use crate::ctx::ClientCertInfo;
use pingora_core::tls::ssl::SslRef;
use pingora_core::tls::x509::X509Ref;

/// Extract client certificate information from SSL connection
/// Returns None if no client certificate is present
pub fn extract_client_cert_info(ssl: &SslRef) -> Option<ClientCertInfo> {
    let cert = ssl.peer_certificate()?;

    Some(extract_client_cert_info_from_cert(&cert))
}

/// Extract client certificate information from an X.509 certificate.
pub(crate) fn extract_client_cert_info_from_cert(cert: &X509Ref) -> ClientCertInfo {
    // Extract subject DN.
    let mut subject = String::with_capacity(128);
    let mut first = true;
    for entry in cert.subject_name().entries() {
        if !first {
            subject.push_str(", ");
        }
        first = false;

        let key = entry.object().nid().short_name().unwrap_or("?");
        subject.push_str(key);
        subject.push('=');

        match entry.data().as_utf8() {
            Ok(s) => subject.push_str(&s),
            Err(_) => subject.push('?'),
        }
    }

    let cn = cert
        .subject_name()
        .entries()
        .find(|entry| entry.object().nid().short_name().map(|s| s == "CN").unwrap_or(false))
        .and_then(|entry| entry.data().as_utf8().map(|s| s.to_string()).ok());

    let sans = extract_sans(cert);

    let fingerprint = cert
        .digest(pingora_core::tls::hash::MessageDigest::sha256())
        .map(|digest| encode_fingerprint(digest.as_ref()))
        .unwrap_or_else(|_| "unknown".to_string());

    ClientCertInfo {
        subject,
        sans,
        cn,
        fingerprint,
    }
}

fn encode_fingerprint(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(3).saturating_sub(1));
    for (index, byte) in bytes.iter().copied().enumerate() {
        if index != 0 {
            encoded.push(':');
        }
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// Extract Subject Alternative Names from certificate
fn extract_sans(cert: &X509Ref) -> Vec<String> {
    extract_san_sets(cert).0
}

pub(crate) fn extract_presented_sans(cert: &X509Ref) -> Vec<edgion_common::allowed_san::PresentedSan> {
    extract_san_sets(cert).1
}

fn extract_san_sets(cert: &X509Ref) -> (Vec<String>, Vec<edgion_common::allowed_san::PresentedSan>) {
    use edgion_common::allowed_san::PresentedSan;

    let mut display = Vec::new();
    let mut presented = Vec::new();

    if let Some(san_ext) = cert.subject_alt_names() {
        for san in san_ext {
            if let Some(dns_name) = san.dnsname() {
                display.push(dns_name.to_string());
                presented.push(PresentedSan::dns(dns_name.to_string()));
            }
            if let Some(ip_bytes) = san.ipaddress() {
                let ip_str = match ip_bytes.len() {
                    4 => {
                        format!("{}.{}.{}.{}", ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3])
                    }
                    16 => {
                        let octets: [u8; 16] = match ip_bytes.try_into() {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        std::net::Ipv6Addr::from(octets).to_string()
                    }
                    _ => continue,
                };
                display.push(format!("IP:{}", ip_str));
                presented.push(PresentedSan::ip(ip_str));
            }
            if let Some(email) = san.email() {
                display.push(format!("email:{}", email));
                presented.push(PresentedSan::email(email.to_string()));
            }
            if let Some(uri) = san.uri() {
                display.push(format!("uri:{}", uri));
                presented.push(PresentedSan::uri(uri.to_string()));
            }
        }
    }

    extract_other_names(cert, &mut display, &mut presented);
    (display, presented)
}

fn extract_other_names(
    cert: &X509Ref,
    display: &mut Vec<String>,
    presented: &mut Vec<edgion_common::allowed_san::PresentedSan>,
) {
    use edgion_common::allowed_san::PresentedSan;
    use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

    let Ok(der) = cert.to_der() else {
        return;
    };
    let Ok((_, parsed)) = X509Certificate::from_der(&der) else {
        return;
    };
    let Ok(Some(san_ext)) = parsed.subject_alternative_name() else {
        return;
    };
    for general_name in &san_ext.value.general_names {
        if let GeneralName::OtherName(oid, value) = general_name {
            let Some(decoded) = decode_other_name_value(value) else {
                continue;
            };
            let oid = oid.to_string();
            display.push(format!("otherName:{oid}={decoded}"));
            presented.push(PresentedSan::other_name(oid, decoded));
        }
    }
}

/// Decode an OtherName payload. Incomplete or non-BER encodings are skipped.
fn decode_other_name_value(bytes: &[u8]) -> Option<String> {
    let (rest, object) = x509_parser::der_parser::ber::parse_ber(bytes).ok()?;
    if !rest.is_empty() {
        return None;
    }
    stringify_other_name_object(&object)
}

/// `x509-parser` leaves OtherName.value as `[0] EXPLICIT ANY`. Generic `parse_ber`
/// stores that wrapper as a context-specific Unknown; unwrap it once.
fn stringify_other_name_object(object: &x509_parser::der_parser::ber::BerObject<'_>) -> Option<String> {
    use x509_parser::der_parser::ber::{BerObjectContent, Class};

    if object.header.class() != Class::ContextSpecific || object.header.tag().0 != 0 || !object.header.constructed() {
        return None;
    }
    match &object.content {
        BerObjectContent::Tagged(_, _, inner) => {
            if inner.header.class() != Class::Universal {
                return None;
            }
            ber_object_to_san_string(inner)
        }
        BerObjectContent::Unknown(any) => {
            let (rest, inner) = x509_parser::der_parser::ber::parse_ber(any.data).ok()?;
            if !rest.is_empty() || inner.header.class() != Class::Universal {
                return None;
            }
            ber_object_to_san_string(&inner)
        }
        _ => None,
    }
}

fn ber_object_to_san_string(object: &x509_parser::der_parser::ber::BerObject<'_>) -> Option<String> {
    use x509_parser::der_parser::ber::BerObjectContent;

    match &object.content {
        BerObjectContent::Boolean(value) => Some(if *value { "true" } else { "false" }.to_string()),
        BerObjectContent::Integer(raw) => integer_to_decimal(raw),
        BerObjectContent::Enum(value) => Some(value.to_string()),
        BerObjectContent::OID(oid) => Some(oid.to_string()),
        BerObjectContent::Null => Some(String::new()),
        BerObjectContent::UTF8String(value)
        | BerObjectContent::IA5String(value)
        | BerObjectContent::PrintableString(value)
        | BerObjectContent::T61String(value)
        | BerObjectContent::NumericString(value)
        | BerObjectContent::VisibleString(value)
        | BerObjectContent::VideotexString(value)
        | BerObjectContent::GraphicString(value)
        | BerObjectContent::GeneralString(value)
        | BerObjectContent::ObjectDescriptor(value) => Some((*value).to_string()),
        BerObjectContent::BmpString(value) => bmp_string(value),
        BerObjectContent::UniversalString(value) => universal_string(value),
        _ => None,
    }
}

/// Bound OtherName INTEGER payloads so a huge BER integer cannot burn handshake CPU.
const OTHER_NAME_INTEGER_MAX_BYTES: usize = 64;

fn integer_to_decimal(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > OTHER_NAME_INTEGER_MAX_BYTES {
        return None;
    }
    Some(signed_be_integer_to_decimal(bytes))
}

fn signed_be_integer_to_decimal(bytes: &[u8]) -> String {
    if bytes.iter().all(|&b| b == 0) {
        return "0".to_string();
    }
    if bytes[0] & 0x80 == 0 {
        return unsigned_be_to_decimal(bytes);
    }
    let mut magnitude = bytes.to_vec();
    for byte in &mut magnitude {
        *byte = !*byte;
    }
    let mut carry = 1u8;
    for byte in magnitude.iter_mut().rev() {
        let (sum, overflow) = byte.overflowing_add(carry);
        *byte = sum;
        carry = u8::from(overflow);
        if carry == 0 {
            break;
        }
    }
    format!("-{}", unsigned_be_to_decimal(&magnitude))
}

fn unsigned_be_to_decimal(bytes: &[u8]) -> String {
    let mut current = bytes.to_vec();
    let mut digits = Vec::new();
    while current.iter().any(|&b| b != 0) {
        let mut remainder = 0u16;
        for byte in &mut current {
            let value = remainder * 256 + u16::from(*byte);
            *byte = (value / 10) as u8;
            remainder = value % 10;
        }
        digits.push(b'0' + remainder as u8);
    }
    if digits.is_empty() {
        return "0".to_string();
    }
    digits.reverse();
    String::from_utf8(digits).expect("decimal digits are ASCII")
}

fn bmp_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

fn universal_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| char::from_u32(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
        .collect()
}

/// Extract Subject Alternative Names from SSL peer certificate.
/// Returns empty vec if no peer certificate or no SANs.
pub fn extract_sans_from_ssl(ssl: &SslRef) -> Vec<String> {
    match ssl.peer_certificate() {
        Some(cert) => extract_sans(&cert),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
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
}
