//! mTLS client certificate validation at application layer
//! 
//! Validates client certificate SAN/CN against whitelist after TLS handshake

use crate::types::ctx::ClientCertInfo;

/// Validate client certificate against SAN whitelist
/// Returns true if certificate matches whitelist or no whitelist configured
pub fn validate_san_whitelist(
    cert_info: &ClientCertInfo,
    allowed_sans: &Vec<String>,
) -> bool {
    if allowed_sans.is_empty() {
        // No whitelist configured, allow all
        return true;
    }
    
    // Check if any SAN matches the whitelist
    for san in &cert_info.sans {
        for allowed in allowed_sans {
            if matches_pattern(san, allowed) {
                tracing::debug!(
                    san = %san,
                    allowed = %allowed,
                    "Client certificate SAN matches whitelist"
                );
                return true;
            }
        }
    }
    
    tracing::warn!(
        sans = ?cert_info.sans,
        allowed_sans = ?allowed_sans,
        "Client certificate SAN does not match whitelist"
    );
    false
}

/// Validate client certificate against CN whitelist
/// Returns true if certificate matches whitelist or no whitelist configured
pub fn validate_cn_whitelist(
    cert_info: &ClientCertInfo,
    allowed_cns: &Vec<String>,
) -> bool {
    if allowed_cns.is_empty() {
        // No whitelist configured, allow all
        return true;
    }
    
    let Some(ref cn) = cert_info.cn else {
        tracing::warn!("Client certificate has no CN, but CN whitelist is configured");
        return false;
    };
    
    // Check if CN matches the whitelist
    for allowed in allowed_cns {
        if matches_pattern(cn, allowed) {
            tracing::debug!(
                cn = %cn,
                allowed = %allowed,
                "Client certificate CN matches whitelist"
            );
            return true;
        }
    }
    
    tracing::warn!(
        cn = %cn,
        allowed_cns = ?allowed_cns,
        "Client certificate CN does not match whitelist"
    );
    false
}

/// Match string against pattern (supports exact match and wildcard)
/// Wildcard rules:
/// - * can only appear at the beginning
/// - *.example.com matches any subdomain of example.com
fn matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern == value {
        // Exact match
        return true;
    }
    
    if pattern.starts_with("*.") {
        // Wildcard match: *.example.com
        // SAFETY: Check pattern length before slicing to prevent panic
        if pattern.len() < 3 {
            // Pattern is just "*." with no suffix - invalid
            return false;
        }
        let suffix = &pattern[2..]; // Remove "*."
        
        if value.ends_with(suffix) {
            // Check that there's exactly one subdomain level
            // SAFETY: value.len() >= suffix.len() because ends_with() succeeded
            let prefix_len = value.len() - suffix.len();
            if prefix_len == 0 {
                // value equals suffix, no subdomain prefix
                return false;
            }
            let prefix = &value[..prefix_len];
            // SAFETY: Check prefix is not empty before further slicing
            if prefix.ends_with('.') {
                if prefix.len() > 1 {
                    // Check subdomain part doesn't contain dots
                    return !prefix[..prefix.len()-1].contains('.');
                }
                // prefix is just "." - invalid
                return false;
            }
        }
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("example.com", "example.com"));
        assert!(!matches_pattern("example.com", "other.com"));
    }
    
    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("sub.example.com", "*.example.com"));
        assert!(!matches_pattern("example.com", "*.example.com"));
        assert!(!matches_pattern("sub.sub.example.com", "*.example.com"));
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
        assert!(validate_san_whitelist(&cert_info, &vec![]));
    }
    
    #[test]
    fn test_validate_san_whitelist_match() {
        let cert_info = ClientCertInfo {
            subject: "CN=test".to_string(),
            sans: vec!["test.example.com".to_string(), "other.com".to_string()],
            cn: Some("test".to_string()),
            fingerprint: "abc123".to_string(),
        };
        
        assert!(validate_san_whitelist(&cert_info, &vec!["test.example.com".to_string()]));
        assert!(validate_san_whitelist(&cert_info, &vec!["other.com".to_string()]));
        assert!(validate_san_whitelist(&cert_info, &vec!["*.example.com".to_string()]));
    }
    
    #[test]
    fn test_validate_san_whitelist_no_match() {
        let cert_info = ClientCertInfo {
            subject: "CN=test".to_string(),
            sans: vec!["test.example.com".to_string()],
            cn: Some("test".to_string()),
            fingerprint: "abc123".to_string(),
        };
        
        assert!(!validate_san_whitelist(&cert_info, &vec!["other.com".to_string()]));
    }
    
    #[test]
    fn test_validate_cn_whitelist() {
        let cert_info = ClientCertInfo {
            subject: "CN=TestUser".to_string(),
            sans: vec![],
            cn: Some("TestUser".to_string()),
            fingerprint: "abc123".to_string(),
        };
        
        assert!(validate_cn_whitelist(&cert_info, &vec![])); // Empty whitelist
        assert!(validate_cn_whitelist(&cert_info, &vec!["TestUser".to_string()]));
        assert!(!validate_cn_whitelist(&cert_info, &vec!["OtherUser".to_string()]));
    }
}

