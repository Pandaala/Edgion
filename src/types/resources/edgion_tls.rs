use super::common::ParentReference;
use super::gateway::SecretObjectReference;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for EdgionTls
pub const EDGION_TLS_GROUP: &str = "edgion.io";

/// Kind for EdgionTls
pub const EDGION_TLS_KIND: &str = "EdgionTls";

/// TLS protocol version (similar to Cloudflare's Minimum TLS Version)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TlsVersion {
    #[serde(rename = "TLS1_0")]
    Tls10,
    #[serde(rename = "TLS1_1")]
    Tls11,
    #[serde(rename = "TLS1_2")]
    Tls12,
    #[serde(rename = "TLS1_3")]
    Tls13,
}

/// Client authentication mode for mTLS
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum ClientAuthMode {
    /// Single-way TLS: only verify server certificate (default)
    #[default]
    Terminate,
    /// Mutual TLS: require valid client certificate
    Mutual,
    /// Optional mutual TLS: client certificate is optional
    OptionalMutual,
}

/// Client authentication configuration for mTLS
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientAuthConfig {
    /// TLS mode (default: Terminate)
    #[serde(default)]
    pub mode: ClientAuthMode,

    /// CA certificate Secret reference (required when mode=Mutual/OptionalMutual)
    /// Secret must contain ca.crt field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_secret_ref: Option<SecretObjectReference>,

    /// Certificate chain verification depth (1-9, default: 1)
    #[serde(default = "default_verify_depth", skip_serializing_if = "is_default_verify_depth")]
    pub verify_depth: u8,

    /// CA Secret data (filled by controller, not from YAML)
    /// Note: This field is serialized for controller->gateway communication,
    /// but should be skipped when deserializing from YAML files
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ca_secret: Option<Secret>,

    /// Optional Subject Alternative Names whitelist
    /// If configured, client certificate SAN must match one of these
    /// Validation happens at application layer after TLS handshake
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_sans: Option<Vec<String>>,

    /// Optional Common Name whitelist
    /// If configured, client certificate CN must match one of these
    /// Validation happens at application layer after TLS handshake
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_cns: Option<Vec<String>>,
}

fn default_verify_depth() -> u8 {
    1
}

fn is_default_verify_depth(depth: &u8) -> bool {
    *depth == 1
}

/// OCSP stapling configuration (planned; data model reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcspStaplingConfig {
    /// Enable OCSP stapling (no-op until handshake support is added)
    #[serde(default)]
    pub enabled: bool,

    /// Refresh interval seconds for OCSP responses
    /// Recommended: 900-3600. When None, implementation default will be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_interval_seconds: Option<u64>,

    /// Fail-open behavior when OCSP response is unavailable/expired
    /// true  -> allow handshake without stapled OCSP
    /// false -> fail handshake
    #[serde(default)]
    pub fail_open: bool,
}

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
#[serde(rename_all = "camelCase")]
pub struct EdgionTlsSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hosts: Vec<String>,
    pub secret_ref: SecretObjectReference,
    /// mTLS client authentication configuration (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_auth: Option<ClientAuthConfig>,
    /// Minimum TLS version (optional, similar to Cloudflare's Minimum TLS Version)
    /// Options: TLS1_0, TLS1_1, TLS1_2, TLS1_3
    /// If not configured, uses BoringSSL default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_tls_version: Option<TlsVersion>,
    /// Cipher list in OpenSSL format (optional, similar to Nginx ssl_ciphers)
    /// Example: ["ECDHE-RSA-AES256-GCM-SHA384", "ECDHE-RSA-AES128-GCM-SHA256"]
    /// If not configured, uses BoringSSL default ciphers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ciphers: Option<Vec<String>>,
    /// Extended/experimental TLS knobs (reserved; requires handshake support)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extend: Option<EdgionTlsExtend>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub secret: Option<Secret>,
}

/// Extended/experimental TLS features (reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionTlsExtend {
    /// Prefer server cipher order
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prefer_server_ciphers: bool,

    /// OCSP stapling configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocsp_stapling: Option<OcspStaplingConfig>,

    /// Session ticket configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ticket: Option<SessionTicketConfig>,

    /// Session cache configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_cache: Option<SessionCacheConfig>,

    /// Revocation check configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_check: Option<RevocationCheckConfig>,

    /// Early data (0-RTT) configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub early_data: Option<EarlyDataConfig>,
}

/// Session ticket configuration (reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionTicketConfig {
    /// Enable session tickets
    #[serde(default)]
    pub enabled: bool,
    /// Ticket lifetime in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime_seconds: Option<u64>,
    /// Optional key rotation interval seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_interval_seconds: Option<u64>,
    /// Optional secret ref for ticket keys (reserved; format TBD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_secret_ref: Option<SecretObjectReference>,
}

/// Session cache configuration (reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionCacheConfig {
    /// Enable session cache
    #[serde(default)]
    pub enabled: bool,
    /// Maximum entries in cache
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<u64>,
    /// Entry TTL seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
}

/// Revocation check configuration (reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevocationCheckConfig {
    /// Mode: off / ocsp / crl (future extension)
    #[serde(default)]
    pub mode: RevocationMode,
    /// Fail open if revocation data unavailable/expired
    #[serde(default)]
    pub fail_open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum RevocationMode {
    #[serde(rename = "off")]
    #[default]
    Off,
    #[serde(rename = "ocsp")]
    Ocsp,
    #[serde(rename = "crl")]
    Crl,
}

/// Early data (0-RTT) configuration (reserved)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EarlyDataConfig {
    /// Enable early data (0-RTT)
    #[serde(default)]
    pub enabled: bool,
    /// Reject on replay risk (if supported by TLS stack)
    #[serde(default)]
    pub reject_on_replay: bool,
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

    /// Extract certificate PEM from the secret
    pub fn cert_pem(&self) -> anyhow::Result<String> {
        let secret = self
            .spec
            .secret
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret not found in EdgionTls"))?;

        let data = secret
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret data not found"))?;

        let cert_pem = data
            .get("tls.crt")
            .ok_or_else(|| anyhow::anyhow!("Secret data tls.crt not found"))?;

        String::from_utf8(cert_pem.0.clone()).map_err(|e| anyhow::anyhow!("Failed to decode cert PEM: {}", e))
    }

    /// Extract private key PEM from the secret
    pub fn key_pem(&self) -> anyhow::Result<String> {
        let secret = self
            .spec
            .secret
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret not found in EdgionTls"))?;

        let data = secret
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret data not found"))?;

        let key_pem = data
            .get("tls.key")
            .ok_or_else(|| anyhow::anyhow!("Secret data tls.key not found"))?;

        String::from_utf8(key_pem.0.clone()).map_err(|e| anyhow::anyhow!("Failed to decode key PEM: {}", e))
    }

    /// Extract CA certificate PEM from the CA secret (for mTLS)
    pub fn ca_cert_pem(&self) -> anyhow::Result<String> {
        let client_auth = self
            .spec
            .client_auth
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("clientAuth not configured"))?;

        let ca_secret = client_auth
            .ca_secret
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("CA secret not loaded by controller"))?;

        let data = ca_secret
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("CA secret data not found"))?;

        let ca_cert_pem = data
            .get("ca.crt")
            .ok_or_else(|| anyhow::anyhow!("CA secret data ca.crt not found"))?;

        String::from_utf8(ca_cert_pem.0.clone()).map_err(|e| anyhow::anyhow!("Failed to decode CA cert PEM: {}", e))
    }

    /// Get client authentication mode
    pub fn client_auth_mode(&self) -> ClientAuthMode {
        self.spec
            .client_auth
            .as_ref()
            .map(|ca| ca.mode.clone())
            .unwrap_or_default()
    }

    /// Check if mTLS is enabled (Mutual or OptionalMutual)
    pub fn is_mtls_enabled(&self) -> bool {
        matches!(
            self.client_auth_mode(),
            ClientAuthMode::Mutual | ClientAuthMode::OptionalMutual
        )
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
            if host_lower.starts_with('*') && Self::wildcard_match(&host_lower, &hostname_lower) {
                return true;
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
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "test-secret".to_string(),
                    namespace: Some("default".to_string()),
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                extend: None,
                secret: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_yaml_deserialization_with_camel_case() {
        // Test that YAML with camelCase field names can be correctly deserialized
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parentRefs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
    - "*.example.com"
  secretRef:
    name: test-secret
    namespace: default
"#;

        let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
        assert!(tls.is_ok(), "Failed to deserialize YAML: {:?}", tls.err());

        let tls = tls.unwrap();
        assert_eq!(tls.metadata.name, Some("test-tls".to_string()));
        assert_eq!(tls.metadata.namespace, Some("default".to_string()));

        // Verify parentRefs
        assert!(tls.spec.parent_refs.is_some());
        let parent_refs = tls.spec.parent_refs.as_ref().unwrap();
        assert_eq!(parent_refs.len(), 1);
        assert_eq!(parent_refs[0].name, "example-gateway");
        assert_eq!(parent_refs[0].namespace, Some("default".to_string()));

        // Verify hosts
        assert_eq!(tls.spec.hosts.len(), 2);
        assert_eq!(tls.spec.hosts[0], "example.com");
        assert_eq!(tls.spec.hosts[1], "*.example.com");

        // Verify secretRef
        assert_eq!(tls.spec.secret_ref.name, "test-secret");
        assert_eq!(tls.spec.secret_ref.namespace, Some("default".to_string()));
    }

    #[test]
    fn test_yaml_serialization_produces_camel_case() {
        // Test that serialization produces camelCase field names
        let tls = EdgionTls {
            metadata: kube::api::ObjectMeta {
                name: Some("test-tls".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: Some(vec![ParentReference {
                    group: None,
                    kind: None,
                    name: "example-gateway".to_string(),
                    namespace: Some("default".to_string()),
                    section_name: None,
                    port: None,
                }]),
                hosts: vec!["example.com".to_string(), "*.example.com".to_string()],
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "test-secret".to_string(),
                    namespace: Some("default".to_string()),
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                extend: None,
                secret: None,
            },
            status: None,
        };

        let yaml = serde_yaml::to_string(&tls).expect("Failed to serialize to YAML");

        // Verify camelCase field names in serialized YAML
        assert!(
            yaml.contains("parentRefs:"),
            "Serialized YAML should contain 'parentRefs'"
        );
        assert!(
            yaml.contains("secretRef:"),
            "Serialized YAML should contain 'secretRef'"
        );

        // Verify snake_case is NOT present
        assert!(
            !yaml.contains("parent_refs:"),
            "Serialized YAML should NOT contain 'parent_refs'"
        );
        assert!(
            !yaml.contains("secret_ref:"),
            "Serialized YAML should NOT contain 'secret_ref'"
        );
    }

    #[test]
    fn test_yaml_round_trip() {
        // Test that we can deserialize and then serialize without losing data
        let original_yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parentRefs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
    - test.com
  secretRef:
    name: test-secret
    namespace: default
"#;

        let tls: EdgionTls = serde_yaml::from_str(original_yaml).expect("Failed to deserialize original YAML");

        let serialized = serde_yaml::to_string(&tls).expect("Failed to serialize back to YAML");

        let tls2: EdgionTls = serde_yaml::from_str(&serialized).expect("Failed to deserialize serialized YAML");

        // Verify data integrity
        assert_eq!(tls.metadata.name, tls2.metadata.name);
        assert_eq!(tls.metadata.namespace, tls2.metadata.namespace);
        assert_eq!(tls.spec.hosts, tls2.spec.hosts);
        assert_eq!(tls.spec.secret_ref.name, tls2.spec.secret_ref.name);

        if let (Some(pr1), Some(pr2)) = (&tls.spec.parent_refs, &tls2.spec.parent_refs) {
            assert_eq!(pr1.len(), pr2.len());
            assert_eq!(pr1[0].name, pr2[0].name);
        }
    }

    #[test]
    fn test_snake_case_fields_should_fail() {
        // Test that YAML with snake_case field names (wrong naming) will fail or lose data
        // This ensures we catch naming convention errors early
        let yaml_with_snake_case = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  parent_refs:
    - name: example-gateway
      namespace: default
  hosts:
    - example.com
  secret_ref:
    name: test-secret
    namespace: default
"#;

        let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml_with_snake_case);

        // Should succeed in parsing (because parent_refs and secret_ref have defaults)
        // but the fields should be None/default because they don't match_engine the expected camelCase names
        if let Ok(tls) = tls {
            // With camelCase enforcement, snake_case fields are ignored
            // parent_refs should be None (not parsed from parent_refs)
            assert!(
                tls.spec.parent_refs.is_none(),
                "parent_refs (snake_case) should be ignored, expected None"
            );

            // secret_ref is required, so this test would actually fail at deserialization
            // But if it doesn't fail, the secretRef field would have a default/empty value
        } else {
            // Expected: deserialization should fail because secretRef is required
            // and secret_ref (snake_case) is not recognized
            println!("Expected failure: {:?}", tls.err());
        }
    }

    #[test]
    fn test_kubernetes_api_conventions_compliance() {
        // This test documents that our API follows Kubernetes conventions:
        // 1. All field names should be camelCase in YAML
        // 2. First character should be lowercase
        // 3. Acronyms should follow camelCase (e.g., secretRef not secretREF)

        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: convention-test
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hosts:
    - "*.example.com"
  secretRef:
    name: my-secret
"#;

        let result: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_ok(),
            "Kubernetes API convention compliant YAML should deserialize successfully"
        );

        let tls = result.unwrap();
        let serialized = serde_yaml::to_string(&tls).unwrap();

        // Verify all conventions are maintained in serialized output
        assert!(serialized.contains("parentRefs:"), "Should use parentRefs (camelCase)");
        assert!(serialized.contains("secretRef:"), "Should use secretRef (camelCase)");
        assert!(
            !serialized.contains("ParentRefs:"),
            "Should NOT use ParentRefs (PascalCase)"
        );
        assert!(
            !serialized.contains("SecretRef:"),
            "Should NOT use SecretRef (PascalCase)"
        );
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

    #[test]
    fn test_client_auth_deserialization() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-test
  namespace: default
spec:
  hosts:
    - api.example.com
  secretRef:
    name: server-tls
    namespace: default
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: default
    verifyDepth: 2
    allowedSans:
      - "client1.example.com"
      - "*.internal.example.com"
    allowedCns:
      - "AdminClient"
"#;

        let tls: Result<EdgionTls, _> = serde_yaml::from_str(yaml);
        assert!(tls.is_ok(), "Failed to deserialize mTLS YAML: {:?}", tls.err());

        let tls = tls.unwrap();
        let client_auth = tls.spec.client_auth.as_ref().unwrap();

        assert_eq!(client_auth.mode, ClientAuthMode::Mutual);
        assert_eq!(client_auth.ca_secret_ref.as_ref().unwrap().name, "client-ca");
        assert_eq!(client_auth.verify_depth, 2);
        // TODO: Re-enable when SAN/CN whitelist is implemented
        // assert_eq!(client_auth.allowed_sans.as_ref().unwrap().len(), 2);
        // assert_eq!(client_auth.allowed_cns.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_client_auth_mode_default() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    caSecretRef:
      name: client-ca
"#;

        let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
        let client_auth = tls.spec.client_auth.as_ref().unwrap();

        // Default mode should be Terminate
        assert_eq!(client_auth.mode, ClientAuthMode::Terminate);
    }

    #[test]
    fn test_client_auth_verify_depth_default() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
"#;

        let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
        let client_auth = tls.spec.client_auth.as_ref().unwrap();

        // Default verify_depth should be 1
        assert_eq!(client_auth.verify_depth, 1);

        // Test serialization - default verify_depth should be omitted
        let serialized = serde_yaml::to_string(&tls).unwrap();
        assert!(
            !serialized.contains("verifyDepth"),
            "Default verifyDepth should be omitted in serialization"
        );
    }

    #[test]
    fn test_client_auth_optional_fields() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
"#;

        let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();
        let client_auth = tls.spec.client_auth.as_ref().unwrap();

        assert_eq!(client_auth.mode, ClientAuthMode::OptionalMutual);
        // TODO: Re-enable when SAN/CN whitelist is implemented
        // assert!(client_auth.allowed_sans.is_none());
        // assert!(client_auth.allowed_cns.is_none());
    }

    #[test]
    fn test_client_auth_helper_methods() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
"#;

        let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(tls.client_auth_mode(), ClientAuthMode::Mutual);
        assert!(tls.is_mtls_enabled());
    }

    #[test]
    fn test_no_client_auth() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: test-tls
  namespace: default
spec:
  hosts:
    - example.com
  secretRef:
    name: test-secret
"#;

        let tls: EdgionTls = serde_yaml::from_str(yaml).unwrap();

        assert!(tls.spec.client_auth.is_none());
        assert_eq!(tls.client_auth_mode(), ClientAuthMode::Terminate);
        assert!(!tls.is_mtls_enabled());
    }
}
