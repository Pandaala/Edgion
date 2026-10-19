//! Client certificate extraction utilities.
//!
//! Extracts client certificate information from SSL connections after TLS handshake.
//! Requires BoringSSL for X.509 certificate access.

use crate::ctx::ClientCertInfo;
use edgion_common::allowed_san::PresentedSan;
use pingora_core::tls::ssl::SslRef;
use pingora_core::tls::x509::X509Ref;
use std::borrow::Cow;
use std::net::{IpAddr, Ipv6Addr};

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
    let mut cn = None;
    let mut cn_seen = false;
    for entry in cert.subject_name().entries() {
        if !first {
            subject.push_str(", ");
        }
        first = false;

        let key = entry.object().nid().short_name().unwrap_or("?");
        subject.push_str(key);
        subject.push('=');

        let is_first_cn = key == "CN" && !cn_seen;
        cn_seen |= is_first_cn;
        match entry.data().as_utf8() {
            Ok(s) => {
                subject.push_str(&s);
                if is_first_cn {
                    cn = Some(s.to_string());
                }
            }
            Err(_) => subject.push('?'),
        }
    }

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
    extract_san_projection(cert, |san| san.into_display())
}

pub(crate) fn extract_presented_sans(cert: &X509Ref) -> Vec<PresentedSan> {
    extract_san_projection(cert, |san| san.into_presented())
}

/// Borrow ordinary text until projection; move shared decoding temporaries.
enum ClassifiedSan<'a> {
    Dns(&'a str),
    Ip(IpAddr),
    Email(&'a str),
    Uri(&'a str),
    OtherName { oid: String, value: Cow<'a, str> },
}

impl ClassifiedSan<'_> {
    fn into_display(self) -> String {
        match self {
            Self::Dns(value) => value.to_string(),
            Self::Ip(value) => format!("IP:{value}"),
            Self::Email(value) => format!("email:{value}"),
            Self::Uri(value) => format!("uri:{value}"),
            Self::OtherName { oid, value } => format!("otherName:{oid}={value}"),
        }
    }

    fn into_presented(self) -> PresentedSan {
        match self {
            Self::Dns(value) => PresentedSan::dns(value),
            Self::Ip(value) => PresentedSan::ip(value.to_string()),
            Self::Email(value) => PresentedSan::email(value),
            Self::Uri(value) => PresentedSan::uri(value),
            Self::OtherName { oid, value } => PresentedSan::other_name(oid, value.into_owned()),
        }
    }
}

fn extract_san_projection<T>(cert: &X509Ref, mut project: impl FnMut(ClassifiedSan<'_>) -> T) -> Vec<T> {
    let mut output = Vec::new();

    if let Some(san_ext) = cert.subject_alt_names() {
        for san in san_ext {
            if let Some(dns_name) = san.dnsname() {
                output.push(project(ClassifiedSan::Dns(dns_name)));
            }
            if let Some(ip_bytes) = san.ipaddress() {
                let ip = match ip_bytes.len() {
                    4 => IpAddr::from([ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]]),
                    16 => {
                        let octets: [u8; 16] = match ip_bytes.try_into() {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        IpAddr::V6(Ipv6Addr::from(octets))
                    }
                    _ => continue,
                };
                output.push(project(ClassifiedSan::Ip(ip)));
            }
            if let Some(email) = san.email() {
                output.push(project(ClassifiedSan::Email(email)));
            }
            if let Some(uri) = san.uri() {
                output.push(project(ClassifiedSan::Uri(uri)));
            }
        }
    }

    extract_other_names(cert, &mut output, &mut project);
    output
}

fn extract_other_names<T>(cert: &X509Ref, output: &mut Vec<T>, project: &mut impl FnMut(ClassifiedSan<'_>) -> T) {
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
            output.push(project(ClassifiedSan::OtherName {
                oid: oid.to_string(),
                value: decoded,
            }));
        }
    }
}

/// Decode an OtherName payload. Incomplete or non-BER encodings are skipped.
fn decode_other_name_value(bytes: &[u8]) -> Option<Cow<'_, str>> {
    let (rest, object) = x509_parser::der_parser::ber::parse_ber(bytes).ok()?;
    if !rest.is_empty() {
        return None;
    }
    stringify_other_name_object(&object)
}

/// `x509-parser` leaves OtherName.value as `[0] EXPLICIT ANY`. Generic `parse_ber`
/// stores that wrapper as a context-specific Unknown; unwrap it once.
fn stringify_other_name_object<'a>(object: &x509_parser::der_parser::ber::BerObject<'a>) -> Option<Cow<'a, str>> {
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

fn ber_object_to_san_string<'a>(object: &x509_parser::der_parser::ber::BerObject<'a>) -> Option<Cow<'a, str>> {
    use x509_parser::der_parser::ber::BerObjectContent;

    match &object.content {
        BerObjectContent::Boolean(value) => Some(Cow::Borrowed(if *value { "true" } else { "false" })),
        BerObjectContent::Integer(raw) => integer_to_decimal(raw).map(Cow::Owned),
        BerObjectContent::Enum(value) => Some(Cow::Owned(value.to_string())),
        BerObjectContent::OID(oid) => Some(Cow::Owned(oid.to_string())),
        BerObjectContent::Null => Some(Cow::Borrowed("")),
        BerObjectContent::UTF8String(value)
        | BerObjectContent::IA5String(value)
        | BerObjectContent::PrintableString(value)
        | BerObjectContent::T61String(value)
        | BerObjectContent::NumericString(value)
        | BerObjectContent::VisibleString(value)
        | BerObjectContent::VideotexString(value)
        | BerObjectContent::GraphicString(value)
        | BerObjectContent::GeneralString(value)
        | BerObjectContent::ObjectDescriptor(value) => Some(Cow::Borrowed(value)),
        BerObjectContent::BmpString(value) => bmp_string(value).map(Cow::Owned),
        BerObjectContent::UniversalString(value) => universal_string(value).map(Cow::Owned),
        _ => None,
    }
}

/// Bound OtherName INTEGER payloads so a huge BER integer cannot burn handshake CPU.
const OTHER_NAME_INTEGER_MAX_BYTES: usize = 64;

fn integer_to_decimal(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > OTHER_NAME_INTEGER_MAX_BYTES {
        return None;
    }
    let mut scratch = [0u8; OTHER_NAME_INTEGER_MAX_BYTES];
    let current = &mut scratch[..bytes.len()];
    current.copy_from_slice(bytes);
    Some(signed_be_integer_to_decimal(current))
}

fn signed_be_integer_to_decimal(bytes: &mut [u8]) -> String {
    if bytes.iter().all(|&b| b == 0) {
        return "0".to_string();
    }
    if bytes[0] & 0x80 == 0 {
        return magnitude_to_decimal(bytes, false);
    }
    for byte in bytes.iter_mut() {
        *byte = !*byte;
    }
    let mut carry = 1u8;
    for byte in bytes.iter_mut().rev() {
        let (sum, overflow) = byte.overflowing_add(carry);
        *byte = sum;
        carry = u8::from(overflow);
        if carry == 0 {
            break;
        }
    }
    magnitude_to_decimal(bytes, true)
}

fn magnitude_to_decimal(current: &mut [u8], negative: bool) -> String {
    // The guarded entry admits at most 64 bytes. Since 256 < 1000, three
    // decimal digits per byte plus one sign byte conservatively bounds output.
    let mut digits = [0u8; OTHER_NAME_INTEGER_MAX_BYTES * 3 + 1];
    let mut start = digits.len();
    while current.iter().any(|&b| b != 0) {
        let mut remainder = 0u16;
        for byte in current.iter_mut() {
            let value = remainder * 256 + u16::from(*byte);
            *byte = (value / 10) as u8;
            remainder = value % 10;
        }
        start -= 1;
        digits[start] = b'0' + remainder as u8;
    }
    if start == digits.len() {
        start -= 1;
        digits[start] = b'0';
    }
    if negative {
        start -= 1;
        digits[start] = b'-';
    }
    std::str::from_utf8(&digits[start..])
        .expect("decimal digits and minus sign are ASCII")
        .to_owned()
}

fn bmp_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
    char::decode_utf16(units).collect::<Result<String, _>>().ok()
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
#[path = "../../../../tests/unit/tls/runtime/backend/cert_extractor_test.rs"]
mod tests;
