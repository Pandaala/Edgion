//! Client certificate extraction utilities
//! 
//! Extracts client certificate information from SSL connections after TLS handshake
//!
//! **Note**: This module requires BoringSSL or OpenSSL for X.509 certificate access.

use crate::types::ctx::ClientCertInfo;
use pingora_core::tls::ssl::SslRef;
use pingora_core::tls::x509::X509Ref;

/// Extract client certificate information from SSL connection
/// Returns None if no client certificate is present
pub fn extract_client_cert_info(ssl: &SslRef) -> Option<ClientCertInfo> {
    // Get peer certificate (client certificate in mTLS)
    let cert = ssl.peer_certificate()?;
    
    // Extract subject DN with efficient string building
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
    
    // Extract Common Name (CN) from subject
    let cn = cert.subject_name()
        .entries()
        .find(|entry| {
            entry.object().nid().short_name()
                .map(|s| s == "CN")
                .unwrap_or(false)
        })
        .and_then(|entry| {
            entry.data().as_utf8()
                .map(|s| s.to_string())
                .ok()
        });
    
    // Extract Subject Alternative Names (SANs)
    let sans = extract_sans(&cert);
    
    // Calculate certificate fingerprint (SHA256)
    let fingerprint = cert.digest(pingora_core::tls::hash::MessageDigest::sha256())
        .map(|digest| {
            let bytes = digest.as_ref();
            bytes.iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_else(|_| "unknown".to_string());
    
    Some(ClientCertInfo {
        subject,
        sans,
        cn,
        fingerprint,
    })
}

/// Extract Subject Alternative Names from certificate
fn extract_sans(cert: &X509Ref) -> Vec<String> {
    let mut sans = Vec::new();
    
    // Get Subject Alternative Names extension
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
                        // IPv4: 4 bytes
                        // SAFETY: We've verified length is 4, so indexing [0..3] is safe
                        format!(
                            "{}.{}.{}.{}",
                            ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
                        )
                    }
                    16 => {
                        // IPv6: 16 bytes - use std::net::Ipv6Addr for RFC-compliant formatting
                        // SAFETY: We've verified length is 16 before converting to array
                        let octets: [u8; 16] = match ip_bytes.try_into() {
                            Ok(arr) => arr,
                            Err(_) => {
                                tracing::error!(
                                    "Failed to convert IPv6 bytes to array, length: {}",
                                    ip_bytes.len()
                                );
                                continue;
                            }
                        };
                        std::net::Ipv6Addr::from(octets).to_string()
                    }
                    _ => {
                        tracing::warn!("Invalid IP address length in SAN: {}", ip_bytes.len());
                        continue;
                    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_extract_sans_empty() {
        // This is a placeholder test - actual testing requires SSL connection
        // Real tests should be in integration tests with actual certificates
    }
}

