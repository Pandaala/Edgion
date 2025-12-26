//! Client certificate extraction utilities
//! 
//! Extracts client certificate information from SSL connections after TLS handshake

use crate::types::ctx::ClientCertInfo;
use pingora_core::tls::ssl::SslRef;
use pingora_core::tls::x509::X509Ref;

/// Extract client certificate information from SSL connection
/// Returns None if no client certificate is present
pub fn extract_client_cert_info(ssl: &SslRef) -> Option<ClientCertInfo> {
    // Get peer certificate (client certificate in mTLS)
    let cert = ssl.peer_certificate()?;
    
    // Extract subject DN
    let subject = cert.subject_name()
        .entries()
        .map(|entry| {
            let key = entry.object().nid().short_name()
                .unwrap_or("?");
            let value = entry.data().as_utf8()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "?".to_string());
            format!("{}={}", key, value)
        })
        .collect::<Vec<_>>()
        .join(", ");
    
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
            digest.as_ref()
                .iter()
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
            // IP addresses
            if let Some(ip) = san.ipaddress() {
                if let Ok(ip_str) = std::str::from_utf8(ip) {
                    sans.push(ip_str.to_string());
                }
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
    use super::*;
    
    #[test]
    fn test_extract_sans_empty() {
        // This is a placeholder test - actual testing requires SSL connection
        // Real tests should be in integration tests with actual certificates
    }
}

