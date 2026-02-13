//! EdgionAcme CRD definition
//!
//! Manages automatic TLS certificate issuance and renewal via the ACME protocol (RFC 8555).
//! Supports HTTP-01 and DNS-01 challenges with automatic EdgionTls/Secret creation.

use super::common::ParentReference;
use super::gateway::SecretObjectReference;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for EdgionAcme
pub const EDGION_ACME_GROUP: &str = "edgion.io";

/// Kind for EdgionAcme
pub const EDGION_ACME_KIND: &str = "EdgionAcme";

// ============================================================================
// ACME Challenge types
// ============================================================================

/// ACME challenge type
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum AcmeChallengeType {
    /// HTTP-01 challenge: ACME server validates domain ownership via HTTP request
    #[serde(rename = "http-01")]
    Http01,
    /// DNS-01 challenge: ACME server validates domain ownership via DNS TXT record
    #[serde(rename = "dns-01")]
    Dns01,
}

impl Default for AcmeChallengeType {
    fn default() -> Self {
        Self::Http01
    }
}

/// HTTP-01 challenge configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Http01ChallengeConfig {
    /// Gateway reference for serving challenge responses.
    /// The challenge handler is activated on this Gateway's HTTP listeners
    /// only when there are active challenges.
    pub gateway_ref: ParentReference,
}

/// DNS-01 challenge configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dns01ChallengeConfig {
    /// DNS provider name: "cloudflare" or "alidns"
    pub provider: String,

    /// Secret reference containing DNS provider API credentials.
    /// Expected Secret keys vary by provider:
    ///   - cloudflare: "api-token"
    ///   - alidns: "access-key-id", "access-key-secret"
    pub credential_ref: SecretObjectReference,

    /// DNS propagation timeout in seconds (default: 120)
    #[serde(default = "default_propagation_timeout")]
    pub propagation_timeout: u64,

    /// DNS propagation check interval in seconds (default: 5)
    #[serde(default = "default_propagation_check_interval")]
    pub propagation_check_interval: u64,
}

fn default_propagation_timeout() -> u64 {
    120
}

fn default_propagation_check_interval() -> u64 {
    5
}

/// ACME challenge configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcmeChallengeConfig {
    /// Challenge type (default: http-01)
    #[serde(default, rename = "type")]
    pub challenge_type: AcmeChallengeType,

    /// HTTP-01 challenge configuration (required when type is http-01)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http01: Option<Http01ChallengeConfig>,

    /// DNS-01 challenge configuration (required when type is dns-01)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns01: Option<Dns01ChallengeConfig>,
}

// ============================================================================
// Certificate key type
// ============================================================================

/// Certificate key algorithm
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum AcmeKeyType {
    /// ECDSA P-256 (recommended, smaller and faster)
    #[serde(rename = "ecdsa-p256")]
    EcdsaP256,
    /// ECDSA P-384
    #[serde(rename = "ecdsa-p384")]
    EcdsaP384,
}

impl Default for AcmeKeyType {
    fn default() -> Self {
        Self::EcdsaP256
    }
}

// ============================================================================
// Renewal configuration
// ============================================================================

/// Certificate renewal configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcmeRenewalConfig {
    /// Days before certificate expiry to trigger renewal (default: 30)
    #[serde(default = "default_renew_before_days")]
    pub renew_before_days: u32,

    /// Interval between renewal checks in seconds (default: 86400 = 24h)
    #[serde(default = "default_check_interval")]
    pub check_interval: u64,

    /// Backoff duration in seconds after a failed renewal attempt (default: 300 = 5min)
    #[serde(default = "default_fail_backoff")]
    pub fail_backoff: u64,
}

impl Default for AcmeRenewalConfig {
    fn default() -> Self {
        Self {
            renew_before_days: default_renew_before_days(),
            check_interval: default_check_interval(),
            fail_backoff: default_fail_backoff(),
        }
    }
}

fn default_renew_before_days() -> u32 {
    30
}

fn default_check_interval() -> u64 {
    86400
}

fn default_fail_backoff() -> u64 {
    300
}

// ============================================================================
// External Account Binding (EAB)
// ============================================================================

/// External Account Binding for ACME providers that require it (e.g., ZeroSSL)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcmeExternalAccountBinding {
    /// Key identifier
    pub key_id: String,
    /// HMAC key (base64url-encoded)
    pub hmac_key: String,
}

// ============================================================================
// Certificate storage configuration
// ============================================================================

/// Configuration for how ACME-issued certificates are stored
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcmeStorageConfig {
    /// Name of the K8s Secret to store the certificate and private key.
    /// The Secret will be created/updated automatically with keys: "tls.crt", "tls.key"
    pub secret_name: String,

    /// Namespace for the certificate Secret (defaults to the EdgionAcme namespace)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_namespace: Option<String>,
}

/// Configuration for automatically creating an EdgionTls resource
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcmeAutoEdgionTlsConfig {
    /// Whether to automatically create/update an EdgionTls resource (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Name for the auto-created EdgionTls resource.
    /// If not specified, defaults to "acme-{EdgionAcme.name}"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Parent references for the auto-created EdgionTls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,
}

impl Default for AcmeAutoEdgionTlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            name: None,
            parent_refs: None,
        }
    }
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Active challenge (runtime, filled by Controller)
// ============================================================================

/// An active ACME HTTP-01 challenge token (filled by Controller at runtime)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHttpChallenge {
    /// The domain being validated
    pub domain: String,

    /// The challenge token (appears in the URL path)
    pub token: String,

    /// The key authorization (returned as the HTTP response body)
    pub key_authorization: String,

    /// Expiry timestamp (Unix seconds). Gateway should ignore expired challenges.
    pub expire_at: u64,
}

// ============================================================================
// EdgionAcme CRD
// ============================================================================

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionAcme",
    plural = "edgionacmes",
    shortname = "eacme",
    namespaced,
    status = "EdgionAcmeStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionAcmeSpec {
    /// ACME server directory URL (default: Let's Encrypt production)
    #[serde(default = "default_acme_server")]
    pub server: String,

    /// Contact email for ACME account registration (required)
    pub email: String,

    /// Domain names to obtain certificates for.
    /// Wildcard domains (e.g., "*.example.com") require DNS-01 challenge.
    pub domains: Vec<String>,

    /// Certificate key algorithm (default: ecdsa-p256)
    #[serde(default)]
    pub key_type: AcmeKeyType,

    /// Challenge configuration
    pub challenge: AcmeChallengeConfig,

    /// Certificate renewal configuration
    #[serde(default)]
    pub renewal: AcmeRenewalConfig,

    /// External Account Binding (required by some ACME providers like ZeroSSL)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_account_binding: Option<AcmeExternalAccountBinding>,

    /// Certificate storage configuration
    pub storage: AcmeStorageConfig,

    /// Automatically create/update EdgionTls resource for the issued certificate
    #[serde(default)]
    pub auto_edgion_tls: AcmeAutoEdgionTlsConfig,

    // =========================================================================
    // Runtime fields (filled by Controller, not from YAML)
    // =========================================================================
    /// Active HTTP-01 challenges (filled by Controller for Gateway to serve).
    /// Gateway checks this field to respond to ACME validation requests.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_challenges: Option<Vec<ActiveHttpChallenge>>,

    /// DNS provider credential Secret (resolved by Controller, not from YAML)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dns_credential_secret: Option<k8s_openapi::api::core::v1::Secret>,
}

fn default_acme_server() -> String {
    "https://acme-v02.api.letsencrypt.org/directory".to_string()
}

// ============================================================================
// Status
// ============================================================================

/// ACME certificate status phase
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum AcmeCertPhase {
    /// No certificate yet, waiting for initial issuance
    Pending,
    /// ACME order in progress (challenge validation, certificate issuance)
    Issuing,
    /// Certificate is valid and active
    Ready,
    /// Certificate renewal in progress
    Renewing,
    /// Last operation failed
    Failed,
}

impl Default for AcmeCertPhase {
    fn default() -> Self {
        Self::Pending
    }
}

/// EdgionAcme resource status
#[derive(Default, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionAcmeStatus {
    /// Current phase of the ACME certificate lifecycle
    #[serde(default)]
    pub phase: AcmeCertPhase,

    /// Certificate serial number (if issued)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_serial: Option<String>,

    /// Certificate expiry time (RFC 3339 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_not_after: Option<String>,

    /// Last successful renewal time (RFC 3339 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_renewal_time: Option<String>,

    /// Last failure reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_reason: Option<String>,

    /// Last failure time (RFC 3339 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_time: Option<String>,

    /// Name of the K8s Secret containing the certificate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,

    /// Name of the auto-created EdgionTls resource
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edgion_tls_name: Option<String>,

    /// ACME account URI (after registration)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_uri: Option<String>,
}

// ============================================================================
// Helper methods
// ============================================================================

impl EdgionAcme {
    /// Check if this ACME config uses HTTP-01 challenge
    pub fn is_http01(&self) -> bool {
        self.spec.challenge.challenge_type == AcmeChallengeType::Http01
    }

    /// Check if this ACME config uses DNS-01 challenge
    pub fn is_dns01(&self) -> bool {
        self.spec.challenge.challenge_type == AcmeChallengeType::Dns01
    }

    /// Check if any domain requires DNS-01 (wildcard domains)
    pub fn has_wildcard_domains(&self) -> bool {
        self.spec.domains.iter().any(|d| d.starts_with("*."))
    }

    /// Get the effective secret namespace
    pub fn get_secret_namespace(&self) -> String {
        self.spec
            .storage
            .secret_namespace
            .clone()
            .or_else(|| self.metadata.namespace.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Get the effective EdgionTls name
    pub fn get_edgion_tls_name(&self) -> String {
        self.spec
            .auto_edgion_tls
            .name
            .clone()
            .unwrap_or_else(|| format!("acme-{}", self.metadata.name.as_deref().unwrap_or("unknown")))
    }

    /// Get active challenges (returns empty slice if none)
    pub fn active_challenges(&self) -> &[ActiveHttpChallenge] {
        self.spec.active_challenges.as_deref().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http01_yaml_deserialization() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-acme
  namespace: default
spec:
  email: admin@example.com
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    http01:
      gatewayRef:
        name: my-gateway
        namespace: default
  storage:
    secretName: acme-cert-example
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: my-gateway
        namespace: default
"#;

        let acme: Result<EdgionAcme, _> = serde_yaml::from_str(yaml);
        assert!(acme.is_ok(), "Failed to deserialize YAML: {:?}", acme.err());

        let acme = acme.unwrap();
        assert_eq!(acme.metadata.name, Some("my-acme".to_string()));
        assert_eq!(acme.spec.email, "admin@example.com");
        assert_eq!(acme.spec.domains.len(), 2);
        assert!(acme.is_http01());
        assert!(!acme.has_wildcard_domains());
        assert_eq!(acme.spec.server, "https://acme-v02.api.letsencrypt.org/directory");
        assert_eq!(acme.spec.renewal.renew_before_days, 30);
    }

    #[test]
    fn test_dns01_yaml_deserialization() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: wildcard-acme
  namespace: default
spec:
  email: admin@example.com
  domains:
    - "*.example.com"
    - example.com
  keyType: ecdsa-p384
  challenge:
    type: dns-01
    dns01:
      provider: cloudflare
      credentialRef:
        name: cf-api-token
        namespace: default
      propagationTimeout: 180
  renewal:
    renewBeforeDays: 14
    checkInterval: 43200
  storage:
    secretName: wildcard-cert
    secretNamespace: cert-store
"#;

        let acme: Result<EdgionAcme, _> = serde_yaml::from_str(yaml);
        assert!(acme.is_ok(), "Failed to deserialize YAML: {:?}", acme.err());

        let acme = acme.unwrap();
        assert!(acme.is_dns01());
        assert!(acme.has_wildcard_domains());
        assert_eq!(acme.spec.key_type, AcmeKeyType::EcdsaP384);
        assert_eq!(acme.spec.renewal.renew_before_days, 14);
        assert_eq!(acme.spec.renewal.check_interval, 43200);

        let dns01 = acme.spec.challenge.dns01.as_ref().unwrap();
        assert_eq!(dns01.provider, "cloudflare");
        assert_eq!(dns01.propagation_timeout, 180);
    }

    #[test]
    fn test_active_challenges() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test-acme
  namespace: default
spec:
  email: admin@example.com
  domains:
    - example.com
  challenge:
    type: http-01
    http01:
      gatewayRef:
        name: my-gateway
  storage:
    secretName: test-cert
  activeChallenges:
    - domain: example.com
      token: "abc123token"
      keyAuthorization: "abc123token.thumbprint"
      expireAt: 1700000000
"#;

        let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
        let challenges = acme.active_challenges();
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].domain, "example.com");
        assert_eq!(challenges[0].token, "abc123token");
        assert_eq!(challenges[0].key_authorization, "abc123token.thumbprint");
    }

    #[test]
    fn test_default_values() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: minimal
  namespace: default
spec:
  email: a@b.com
  domains:
    - a.com
  challenge:
    type: http-01
    http01:
      gatewayRef:
        name: gw
  storage:
    secretName: cert
"#;

        let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(acme.spec.server, "https://acme-v02.api.letsencrypt.org/directory");
        assert_eq!(acme.spec.key_type, AcmeKeyType::EcdsaP256);
        assert_eq!(acme.spec.renewal.renew_before_days, 30);
        assert_eq!(acme.spec.renewal.check_interval, 86400);
        assert_eq!(acme.spec.renewal.fail_backoff, 300);
        assert!(acme.spec.auto_edgion_tls.enabled);
        assert!(acme.spec.active_challenges.is_none());
        assert!(acme.spec.external_account_binding.is_none());
    }

    #[test]
    fn test_get_edgion_tls_name() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-cert
  namespace: default
spec:
  email: a@b.com
  domains: [a.com]
  challenge:
    type: http-01
    http01:
      gatewayRef:
        name: gw
  storage:
    secretName: cert
"#;

        let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(acme.get_edgion_tls_name(), "acme-my-cert");
        assert_eq!(acme.get_secret_namespace(), "default");
    }
}
