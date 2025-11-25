use super::gateway::SecretObjectReference;
use super::http_route::ParentReference;
use k8s_openapi::api::core::v1::SecretReference;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for EdgionTls
pub const EDGION_TLS_GROUP: &str = "edgion.io";

/// Kind for EdgionTls
pub const EDGION_TLS_KIND: &str = "EdgionTls";

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionTls",
    plural = "edgiontls",
    shortname = "etls",
    namespaced,
    status = "EdgionTlsStatus"
)]
#[serde(rename_all = "snake_case")]
pub struct EdgionTlsSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hosts: Vec<String>,
    pub secret_ref: SecretReference,

    // todo, replace secret_refer
    /// CertificateRefs contains references to Kubernetes objects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_refs: Option<Vec<SecretObjectReference>>,
}

#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionTlsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition: Vec<Condition>,
}

impl EdgionTls {
    pub fn get_secret_namespace(&self) -> Option<String> {
        self.spec
            .secret_ref
            .namespace
            .clone()
            .or_else(|| self.metadata.namespace.clone())
    }

    pub fn matches_hostname(&self, hostname: &str) -> bool {
        let hostname_lower = hostname.to_lowercase();

        for host in &self.spec.hosts {
            let host_lower = host.to_lowercase();

            // Exact match_engine
            if host_lower == hostname_lower {
                return true;
            }

            // Wildcard match_engine: only allow * at the beginning in "*.*.*.domain" format
            if host_lower.starts_with('*') {
                if Self::wildcard_match(&host_lower, &hostname_lower) {
                    return true;
                }
            }
        }

        false
    }

    /// Match hostname against a wildcard pattern using dual-pointer approach
    /// Wildcard rules:
    /// - * can only appear at the beginning of the pattern
    /// - Must be in the form of consecutive "*." (e.g., "*.example.com", "*.*.example.com")
    /// - Each * matches exactly one domain level
    fn wildcard_match(pattern: &str, hostname: &str) -> bool {
        let pattern_bytes = pattern.as_bytes();
        let hostname_bytes = hostname.as_bytes();
        let pattern_len = pattern_bytes.len();
        let hostname_len = hostname_bytes.len();

        let mut p_idx = 0; // Pattern pointer
        let mut h_idx = 0; // Hostname pointer
        let mut has_exact_match = false; // Track if we've seen any exact match_engine segment

        // Process pattern segment by segment
        while p_idx < pattern_len {
            // Find next dot or end of pattern
            let segment_start = p_idx;
            let mut segment_end = p_idx;
            while segment_end < pattern_len && pattern_bytes[segment_end] != b'.' {
                segment_end += 1;
            }

            let segment_len = segment_end - segment_start;

            // Check if this segment is a wildcard
            if segment_len == 1 && pattern_bytes[segment_start] == b'*' {
                // This is a wildcard segment

                // Rule: wildcard cannot appear after exact match_engine
                if has_exact_match {
                    return false;
                }

                // Find the next dot in hostname (or end)
                if h_idx >= hostname_len {
                    return false; // No more hostname to match_engine
                }

                let h_segment_start = h_idx;
                let mut h_segment_end = h_idx;
                while h_segment_end < hostname_len && hostname_bytes[h_segment_end] != b'.' {
                    h_segment_end += 1;
                }

                // Wildcard must match_engine at least one character
                if h_segment_end == h_segment_start {
                    return false;
                }

                // Move hostname pointer past this segment
                h_idx = h_segment_end;
            } else {
                // This is an exact match_engine segment
                has_exact_match = true;

                // Check if hostname has enough bytes left
                if h_idx + segment_len > hostname_len {
                    return false;
                }

                // Compare bytes
                for i in 0..segment_len {
                    if pattern_bytes[segment_start + i] != hostname_bytes[h_idx + i] {
                        return false;
                    }
                }

                // Move hostname pointer
                h_idx += segment_len;
            }

            // Move pattern pointer past the segment
            p_idx = segment_end;

            // Handle the dot separator
            if p_idx < pattern_len {
                // Pattern has a dot
                if h_idx >= hostname_len || hostname_bytes[h_idx] != b'.' {
                    return false;
                }
                p_idx += 1; // Skip dot in pattern
                h_idx += 1; // Skip dot in hostname
            }
        }

        // Both pointers should be at the end
        h_idx == hostname_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_tls(hosts: Vec<&str>) -> EdgionTls {
        EdgionTls {
            metadata: Default::default(),
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: hosts.iter().map(|s| s.to_string()).collect(),
                secret_ref: SecretReference {
                    name: Some("test-secret".to_string()),
                    namespace: Some("default".to_string()),
                },
                certificate_refs: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_exact_match() {
        let tls = create_tls(vec!["example.com", "test.com"]);

        assert!(tls.matches_hostname("example.com"));
        assert!(tls.matches_hostname("test.com"));
        assert!(!tls.matches_hostname("other.com"));
    }

    #[test]
    fn test_single_wildcard_one_level() {
        let tls = create_tls(vec!["*.aaa.com"]);

        // Should match_engine one level
        assert!(tls.matches_hostname("test.aaa.com"));
        assert!(tls.matches_hostname("foo.aaa.com"));
        assert!(tls.matches_hostname("bar.aaa.com"));

        // Should NOT match_engine multiple levels
        assert!(!tls.matches_hostname("my.test.aaa.com"));
        assert!(!tls.matches_hostname("a.b.aaa.com"));

        // Should NOT match_engine base domain
        assert!(!tls.matches_hostname("aaa.com"));

        // Should NOT match_engine different domain
        assert!(!tls.matches_hostname("test.bbb.com"));
    }

    #[test]
    fn test_double_wildcard() {
        let tls = create_tls(vec!["*.*.aaa.com"]);

        // Should match_engine two levels
        assert!(tls.matches_hostname("my.test.aaa.com"));
        assert!(tls.matches_hostname("a.b.aaa.com"));

        // Should NOT match_engine one level
        assert!(!tls.matches_hostname("test.aaa.com"));

        // Should NOT match_engine three levels
        assert!(!tls.matches_hostname("x.y.z.aaa.com"));
    }

    #[test]
    fn test_invalid_wildcard_with_prefix() {
        // *-api.example.com is INVALID (wildcard not followed by dot)
        let tls = create_tls(vec!["*-api.example.com"]);

        assert!(!tls.matches_hostname("foo-api.example.com"));
        assert!(!tls.matches_hostname("bar-api.example.com"));
    }

    #[test]
    fn test_invalid_wildcard_with_suffix() {
        // api-*.example.com is INVALID (wildcard not at beginning)
        let tls = create_tls(vec!["api-*.example.com"]);

        assert!(!tls.matches_hostname("api-v1.example.com"));
        assert!(!tls.matches_hostname("api-v2.example.com"));
    }

    #[test]
    fn test_case_insensitive() {
        let tls = create_tls(vec!["*.Example.COM"]);

        assert!(tls.matches_hostname("test.example.com"));
        assert!(tls.matches_hostname("TEST.EXAMPLE.COM"));
        assert!(tls.matches_hostname("Test.Example.Com"));
    }

    #[test]
    fn test_multiple_hosts() {
        let tls = create_tls(vec!["*.aaa.com", "*.bbb.com", "exact.ccc.com"]);

        assert!(tls.matches_hostname("test.aaa.com"));
        assert!(tls.matches_hostname("test.bbb.com"));
        assert!(tls.matches_hostname("exact.ccc.com"));

        assert!(!tls.matches_hostname("test.ccc.com"));
        assert!(!tls.matches_hostname("my.test.aaa.com"));
    }

    #[test]
    fn test_invalid_wildcard_in_middle() {
        // foo.*.example.com is INVALID (wildcard not at beginning)
        let tls = create_tls(vec!["foo.*.example.com"]);

        assert!(!tls.matches_hostname("foo.bar.example.com"));
        assert!(!tls.matches_hostname("foo.test.example.com"));
    }

    #[test]
    fn test_invalid_wildcard_mixed() {
        // *.aaa.*.com is INVALID (wildcard in the middle)
        let tls = create_tls(vec!["*.aaa.*.com"]);

        assert!(!tls.matches_hostname("test.aaa.example.com"));
        assert!(!tls.matches_hostname("foo.aaa.bar.com"));
    }

    #[test]
    fn test_triple_wildcard() {
        let tls = create_tls(vec!["*.*.*.example.com"]);

        // Should match_engine three levels
        assert!(tls.matches_hostname("a.b.c.example.com"));
        assert!(tls.matches_hostname("foo.bar.baz.example.com"));

        // Should NOT match_engine two levels
        assert!(!tls.matches_hostname("a.b.example.com"));

        // Should NOT match_engine four levels
        assert!(!tls.matches_hostname("a.b.c.d.example.com"));
    }

    #[test]
    fn test_empty_hostname() {
        let tls = create_tls(vec!["*.example.com"]);

        assert!(!tls.matches_hostname(""));
    }

    #[test]
    fn test_no_hosts() {
        let tls = create_tls(vec![]);

        assert!(!tls.matches_hostname("test.example.com"));
    }

    #[test]
    fn test_hostname_longer_than_pattern() {
        let tls = create_tls(vec!["*.example.com"]);

        // Hostname has extra characters at the end (not a valid domain)
        assert!(!tls.matches_hostname("aaa.example.coma"));

        // Hostname has extra domain level at the end
        assert!(!tls.matches_hostname("aaa.example.com.us"));

        // Hostname has extra domain level at the beginning
        assert!(!tls.matches_hostname("sub.aaa.example.com"));

        // Valid match_engine - exactly one level before example.com
        assert!(tls.matches_hostname("aaa.example.com"));
    }

    #[test]
    fn test_hostname_longer_with_double_wildcard() {
        let tls = create_tls(vec!["*.*.example.com"]);

        // Hostname has extra characters at the end
        assert!(!tls.matches_hostname("a.b.example.coma"));

        // Hostname has extra domain level at the end
        assert!(!tls.matches_hostname("a.b.example.com.us"));

        // Hostname has extra domain level at the beginning
        assert!(!tls.matches_hostname("c.a.b.example.com"));

        // Valid match_engine - exactly two levels before example.com
        assert!(tls.matches_hostname("a.b.example.com"));
    }
}
