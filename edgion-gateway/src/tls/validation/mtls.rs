//! mTLS client certificate validation at application layer
//!
//! Validates client certificate SAN/CN against whitelist after TLS handshake

use crate::ctx::ClientCertInfo;

/// Validate client certificate against SAN whitelist
/// Returns true if certificate matches whitelist or no whitelist configured
pub fn validate_san_whitelist(cert_info: &ClientCertInfo, allowed_sans: &[String]) -> bool {
    if allowed_sans.is_empty() {
        return true;
    }

    for san in &cert_info.sans {
        for allowed in allowed_sans {
            if super::matches_host_pattern(san, allowed) {
                return true;
            }
        }
    }

    false
}

/// Validate client certificate against CN whitelist
/// Returns true if certificate matches whitelist or no whitelist configured
pub fn validate_cn_whitelist(cert_info: &ClientCertInfo, allowed_cns: &[String]) -> bool {
    if allowed_cns.is_empty() {
        return true;
    }

    let Some(ref cn) = cert_info.cn else {
        return false;
    };

    for allowed in allowed_cns {
        if super::matches_host_pattern(cn, allowed) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_pattern_exact() {
        use crate::tls::validation::matches_host_pattern;
        assert!(matches_host_pattern("example.com", "example.com"));
        assert!(!matches_host_pattern("example.com", "other.com"));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        use crate::tls::validation::matches_host_pattern;
        assert!(matches_host_pattern("sub.example.com", "*.example.com"));
        assert!(!matches_host_pattern("example.com", "*.example.com"));
        assert!(!matches_host_pattern("sub.sub.example.com", "*.example.com"));
    }

    #[test]
    fn test_validate_san_whitelist_empty() {
        let cert_info = ClientCertInfo {
            subject: "CN=test".to_string(),
            sans: vec!["test.example.com".to_string()],
            cn: Some("test".to_string()),
            fingerprint: "abc123".to_string(),
        };

        // Empty whitelist should allow all
        assert!(validate_san_whitelist(&cert_info, &[]));
    }

    #[test]
    fn test_validate_san_whitelist_match() {
        let cert_info = ClientCertInfo {
            subject: "CN=test".to_string(),
            sans: vec!["test.example.com".to_string(), "other.com".to_string()],
            cn: Some("test".to_string()),
            fingerprint: "abc123".to_string(),
        };

        assert!(validate_san_whitelist(&cert_info, &["test.example.com".to_string()]));
        assert!(validate_san_whitelist(&cert_info, &["other.com".to_string()]));
        assert!(validate_san_whitelist(&cert_info, &["*.example.com".to_string()]));
    }

    #[test]
    fn test_validate_san_whitelist_no_match() {
        let cert_info = ClientCertInfo {
            subject: "CN=test".to_string(),
            sans: vec!["test.example.com".to_string()],
            cn: Some("test".to_string()),
            fingerprint: "abc123".to_string(),
        };

        assert!(!validate_san_whitelist(&cert_info, &["other.com".to_string()]));
    }

    #[test]
    fn test_validate_cn_whitelist() {
        let cert_info = ClientCertInfo {
            subject: "CN=TestUser".to_string(),
            sans: vec![],
            cn: Some("TestUser".to_string()),
            fingerprint: "abc123".to_string(),
        };

        assert!(validate_cn_whitelist(&cert_info, &[])); // Empty whitelist
        assert!(validate_cn_whitelist(&cert_info, &["TestUser".to_string()]));
        assert!(!validate_cn_whitelist(&cert_info, &["OtherUser".to_string()]));
    }
}
