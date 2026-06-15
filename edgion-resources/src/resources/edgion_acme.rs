//! EdgionAcme CRD definition
//!
//! Manages automatic TLS certificate issuance and renewal via the ACME protocol (RFC 8555).
//! Supports HTTP-01 and DNS-01 challenges with automatic EdgionTls/Secret creation.

use super::common::Condition;
use super::common::{ParentReference, SecretObjectReference};
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

/// ACME challenge configuration.
///
/// Tagged enum with `type: http-01 | dns-01` discriminator. The fields specific
/// to each challenge are flat under the discriminator, so non-applicable fields
/// cannot be expressed in YAML at all (the type system prevents the
/// "type=http-01 + dns01 fields" misconfiguration).
///
/// ```yaml
/// challenge:
///   type: http-01
///   gatewayRef:
///     name: my-gateway
/// ```
///
/// ```yaml
/// challenge:
///   type: dns-01
///   provider: cloudflare
///   credentialRef:
///     name: cf-api-token
///   propagationTimeout: 180
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum AcmeChallenge {
    /// HTTP-01 challenge: ACME server validates domain ownership via HTTP request.
    #[serde(rename = "http-01", rename_all = "camelCase")]
    Http01 {
        /// Gateway reference for serving challenge responses.
        /// The challenge handler is activated on this Gateway's HTTP listeners
        /// only when there are active challenges.
        gateway_ref: ParentReference,
    },
    /// DNS-01 challenge: ACME server validates domain ownership via DNS TXT record.
    #[serde(rename = "dns-01", rename_all = "camelCase")]
    Dns01 {
        /// DNS provider name: "cloudflare" or "alidns"
        provider: String,

        /// Secret reference containing DNS provider API credentials.
        /// Expected Secret keys vary by provider:
        ///   - cloudflare: "api-token"
        ///   - alidns: "access-key-id", "access-key-secret"
        credential_ref: SecretObjectReference,

        /// DNS propagation timeout in seconds (default: 120)
        #[serde(default = "default_propagation_timeout")]
        propagation_timeout: u64,

        /// DNS propagation check interval in seconds (default: 5)
        #[serde(default = "default_propagation_check_interval")]
        propagation_check_interval: u64,
    },
}

fn default_propagation_timeout() -> u64 {
    120
}

fn default_propagation_check_interval() -> u64 {
    5
}

// ============================================================================
// Certificate key type
// ============================================================================

/// Certificate key algorithm
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum AcmeKeyType {
    /// ECDSA P-256 (recommended, smaller and faster)
    #[serde(rename = "ecdsa-p256")]
    #[default]
    EcdsaP256,
    /// ECDSA P-384
    #[serde(rename = "ecdsa-p384")]
    EcdsaP384,
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
// Active challenge (status-only, written by AcmeService, observed by Gateway)
// ============================================================================

/// An active ACME HTTP-01 challenge token, published in `EdgionAcmeStatus`
/// by the controller's AcmeService for the Gateway to serve.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHttpChallenge {
    /// The domain being validated
    pub domain: String,

    /// The challenge token (appears in the URL path)
    #[serde(serialize_with = "crate::redact::redact_required::serialize")]
    pub token: String,

    /// The key authorization (returned as the HTTP response body)
    #[serde(serialize_with = "crate::redact::redact_required::serialize")]
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

    /// K8s Secret to store the ACME account private key and credentials.
    ///
    /// **REQUIRED** - must be specified by the operator.
    ///
    /// Edgion will create this Secret if it doesn't exist (with ownerReferences pointing
    /// to the EdgionAcme resource). If the Secret already exists, Edgion will:
    /// - Use existing credentials if `accountCredentials` key is present (backup restore scenario)
    /// - Patch the `accountCredentials` key if the Secret exists but key is missing (self-managed scenario)
    ///
    /// See cert-manager's Issuer.spec.acme.privateKeySecretRef for the same pattern.
    pub private_key_secret_ref: SecretObjectReference,

    /// Domain names to obtain certificates for.
    /// Wildcard domains (e.g., "*.example.com") require DNS-01 challenge.
    pub domains: Vec<String>,

    /// Certificate key algorithm (default: ecdsa-p256)
    #[serde(default)]
    pub key_type: AcmeKeyType,

    /// Challenge configuration
    pub challenge: AcmeChallenge,

    /// Certificate renewal configuration
    #[serde(default)]
    pub renewal: AcmeRenewalConfig,

    /// External Account Binding (required by some ACME providers like ZeroSSL)
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    pub external_account_binding: Option<AcmeExternalAccountBinding>,

    /// Certificate storage configuration
    pub storage: AcmeStorageConfig,

    /// Automatically create/update EdgionTls resource for the issued certificate
    #[serde(default)]
    pub auto_edgion_tls: AcmeAutoEdgionTlsConfig,
}

fn default_acme_server() -> String {
    "https://acme-v02.api.letsencrypt.org/directory".to_string()
}

// ============================================================================
// Status
// ============================================================================

/// ACME certificate status phase
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum AcmeCertPhase {
    /// No certificate yet, waiting for initial issuance
    #[default]
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

/// EdgionAcme resource status
#[derive(Default, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionAcmeStatus {
    /// Gateway-API-style conditions. `Accepted` reflects spec validation.
    /// The `phase` field and timestamps below track the asynchronous ACME lifecycle
    /// (issuance / renewal) and are updated by the background AcmeService, not here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,

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

    /// Active HTTP-01 challenge tokens.
    ///
    /// Written by the Controller's AcmeService through the `/status` subresource
    /// (separate field manager `edgion-acme-challenges`) and observed by the
    /// Gateway to serve `/.well-known/acme-challenge/<token>` responses.
    /// Empty (or absent) when no challenge is in flight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_challenges: Vec<ActiveHttpChallenge>,
}

// ============================================================================
// Helper methods
// ============================================================================

impl EdgionAcme {
    /// Check if this ACME config uses HTTP-01 challenge
    pub fn is_http01(&self) -> bool {
        matches!(self.spec.challenge, AcmeChallenge::Http01 { .. })
    }

    /// Check if this ACME config uses DNS-01 challenge
    pub fn is_dns01(&self) -> bool {
        matches!(self.spec.challenge, AcmeChallenge::Dns01 { .. })
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

    /// Get active challenges from status (empty slice when none in flight).
    pub fn active_challenges(&self) -> &[ActiveHttpChallenge] {
        self.status
            .as_ref()
            .map(|s| s.active_challenges.as_slice())
            .unwrap_or_default()
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
  privateKeySecretRef:
    name: my-acme-account
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
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
        assert_eq!(acme.spec.private_key_secret_ref.name, "my-acme-account");
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
  privateKeySecretRef:
    name: wildcard-acme-account
  domains:
    - "*.example.com"
    - example.com
  keyType: ecdsa-p384
  challenge:
    type: dns-01
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

        match &acme.spec.challenge {
            AcmeChallenge::Dns01 {
                provider,
                propagation_timeout,
                ..
            } => {
                assert_eq!(provider, "cloudflare");
                assert_eq!(*propagation_timeout, 180);
            }
            other => panic!("expected Dns01 variant, got {:?}", other),
        }
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
  privateKeySecretRef:
    name: test-acme-account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: my-gateway
  storage:
    secretName: test-cert
status:
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
  privateKeySecretRef:
    name: minimal-account
  domains:
    - a.com
  challenge:
    type: http-01
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
        assert!(acme.status.is_none());
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
  privateKeySecretRef:
    name: my-cert-account
  domains: [a.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;

        let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(acme.get_edgion_tls_name(), "acme-my-cert");
        assert_eq!(acme.get_secret_namespace(), "default");
    }

    #[test]
    fn test_private_key_secret_ref_required() {
        // Missing privateKeySecretRef should be rejected
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test
  namespace: default
spec:
  email: admin@example.com
  domains: [example.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;
        let result: Result<EdgionAcme, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "privateKeySecretRef should be required");
    }

    #[test]
    fn test_private_key_secret_ref_with_namespace() {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: test
  namespace: default
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: my-acme-account
    namespace: secrets-ns
  domains: [example.com]
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: cert
"#;
        let acme: EdgionAcme = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(acme.spec.private_key_secret_ref.name, "my-acme-account");
        assert_eq!(
            acme.spec.private_key_secret_ref.namespace.as_deref(),
            Some("secrets-ns")
        );
    }
}
