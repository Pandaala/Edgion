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

    let sans = extract_sans(&cert);

    let fingerprint = cert
        .digest(pingora_core::tls::hash::MessageDigest::sha256())
        .map(|digest| encode_fingerprint(digest.as_ref()))
        .unwrap_or_else(|_| "unknown".to_string());

    Some(ClientCertInfo {
        subject,
        sans,
        cn,
        fingerprint,
    })
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
    let mut sans = Vec::new();

    if let Some(san_ext) = cert.subject_alt_names() {
        for san in san_ext {
            // DNS names
            if let Some(dns_name) = san.dnsname() {
                sans.push(dns_name.to_string());
            }
            // IP addresses - MUST handle as binary data, not UTF-8
            if let Some(ip_bytes) = san.ipaddress() {
                let ip_str = match ip_bytes.len() {
                    4 => {
                        format!("{}.{}.{}.{}", ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3])
                    }
                    16 => {
                        // Use Ipv6Addr for RFC-compliant compressed IPv6 formatting.
                        let octets: [u8; 16] = match ip_bytes.try_into() {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        std::net::Ipv6Addr::from(octets).to_string()
                    }
                    _ => continue,
                };
                sans.push(format!("IP:{}", ip_str));
            }
            // Email addresses
            if let Some(email) = san.email() {
                sans.push(format!("email:{}", email));
            }
            // URI
            if let Some(uri) = san.uri() {
                sans.push(format!("uri:{}", uri));
            }
        }
    }

    sans
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
    use super::{encode_fingerprint, extract_sans};
    use pingora_core::tls::x509::X509;

    #[test]
    fn extracts_dns_and_ip_subject_alt_names() {
        let key = rcgen::KeyPair::generate().expect("test key generation must succeed");
        let params = rcgen::CertificateParams::new(vec!["client.example".to_string(), "127.0.0.1".to_string()])
            .expect("test SAN values must be valid");
        let pem = params
            .self_signed(&key)
            .expect("test certificate signing must succeed")
            .pem();
        let cert = X509::from_pem(pem.as_bytes()).expect("generated certificate must parse");

        let sans = extract_sans(&cert);
        assert!(sans.iter().any(|san| san == "client.example"));
        assert!(sans.iter().any(|san| san == "IP:127.0.0.1"));
    }

    #[test]
    fn fingerprint_encoding_preserves_colon_separated_lower_hex() {
        assert_eq!(encode_fingerprint(&[]), "");
        assert_eq!(encode_fingerprint(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "00:0f:10:ab:ff");
    }
}
