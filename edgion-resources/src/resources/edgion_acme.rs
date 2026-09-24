//! EdgionAcme CRD definition
//!
//! Manages automatic TLS certificate issuance and renewal via the ACME protocol (RFC 8555).
//! Supports HTTP-01 challenges with automatic EdgionTls/Secret creation.

use super::common::Condition;

crate::redact::impl_redacted_debug!(ActiveHttpChallenge);
use super::common::{ParentReference, SecretObjectReference};
use super::edgion_tls::ResolvedListenerAttachment;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================================
// ACME Challenge types
// ============================================================================

/// Maximum number of Controller-resolved HTTP listener attachments for one
/// `EdgionAcme`. A single `gatewayRef` cannot expand beyond one Gateway's
/// bounded listener set.
pub const MAX_ACME_RESOLVED_ATTACHMENTS: usize = super::gateway::MAX_GATEWAY_LISTENERS;

/// ACME challenge configuration.
///
/// ```yaml
/// challenge:
///   type: http-01
///   gatewayRef:
///     name: my-gateway
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum AcmeChallenge {
    /// HTTP-01 challenge: ACME server validates domain ownership via HTTP request.
    #[serde(rename = "http-01", rename_all = "camelCase")]
    Http01 {
        /// Gateway reference for serving challenge responses.
        /// The challenge handler is activated on this Gateway's HTTP listeners
        /// only when there are active challenges.
        gateway_ref: ParentReference,
    },
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
    /// Lead time before certificate expiry to trigger renewal
    /// (GEP-2257 duration, default: "720h" = 30 days)
    #[serde(default = "default_renew_before")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub renew_before: String,

    /// Interval between renewal checks (GEP-2257 duration, default: "24h")
    #[serde(default = "default_check_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub check_interval: String,

    /// Backoff after a failed renewal attempt (GEP-2257 duration, default: "5m")
    #[serde(default = "default_fail_backoff")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub fail_backoff: String,
}

impl Default for AcmeRenewalConfig {
    fn default() -> Self {
        Self {
            renew_before: default_renew_before(),
            check_interval: default_check_interval(),
            fail_backoff: default_fail_backoff(),
        }
    }
}

fn default_renew_before() -> String {
    "720h".to_string()
}

fn default_check_interval() -> String {
    "24h".to_string()
}

fn default_fail_backoff() -> String {
    "5m".to_string()
}

// ============================================================================
// External Account Binding (EAB)
// ============================================================================

/// External Account Binding for ACME providers that require it (e.g., ZeroSSL)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcmeExternalAccountBinding {
    /// Key identifier (public; not a secret)
    pub key_id: String,
    /// Reference to a K8s Secret whose `hmacKey` data entry holds the
    /// base64url-encoded EAB HMAC key. The controller fetches it at
    /// account-registration time; it never enters the stored CR.
    pub key_secret_ref: SecretObjectReference,
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
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<EdgionAcmeStatus>,

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
    /// Wildcard domains are unsupported; use an externally issued TLS Secret.
    pub domains: Vec<String>,

    /// Certificate key algorithm (default: ecdsa-p256)
    #[serde(default)]
    pub key_type: AcmeKeyType,

    /// Challenge configuration
    pub challenge: AcmeChallenge,

    /// Controller-resolved logical Gateway listener identities authorized to
    /// serve this resource's HTTP-01 challenges. This internal transport field
    /// is recomputed on every reconcile and is never operator-authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listener_attachments: Option<Vec<ResolvedListenerAttachment>>,

    /// Per-reconcile Controller notification intent, consumed after cache publication.
    /// Never accepted from input or included in the CRD/conf-sync schema.
    #[serde(skip)]
    #[schemars(skip)]
    pub notify_after_publish: bool,

    /// Certificate renewal configuration
    #[serde(default)]
    pub renewal: AcmeRenewalConfig,

    /// External Account Binding (required by some ACME providers like ZeroSSL)
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    /// Durable checkpoint for the Controller's ACME scheduler.
    ///
    /// This is read-only operational state. It is persisted so a new leader can
    /// recover the retry budget without treating a restart as a fresh attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<AcmeSchedulerStatus>,

    /// Active HTTP-01 challenge tokens.
    ///
    /// Written by the Controller's AcmeService through the `/status` subresource
    /// (separate field manager `edgion-acme-challenges`) and observed by the
    /// Gateway to serve `/.well-known/acme-challenge/<token>` responses.
    /// Empty (or absent) when no challenge is in flight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_challenges: Vec<ActiveHttpChallenge>,
}

/// Durable, non-secret ACME scheduler checkpoint.
#[derive(Default, Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcmeSchedulerStatus {
    /// Resource generation for which this retry budget was recorded.
    pub observed_generation: i64,

    /// Identifier of the most recently reserved issuance attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,

    /// Number of issuance attempts reserved for this resource generation.
    #[serde(default)]
    pub attempts_used: u32,

    /// Time at which the current/last attempt was reserved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_started_at: Option<String>,

    /// Earliest time at which the scheduler may start another attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
}

// ============================================================================
// Helper methods
// ============================================================================

impl EdgionAcme {
    /// Check if any domain is a wildcard (unsupported by built-in ACME)
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

    /// Get this deployment's current challenges (empty when not published).
    pub fn active_challenges(&self) -> &[ActiveHttpChallenge] {
        self.spec
            .current_status
            .as_ref()
            .map(|s| s.active_challenges.as_slice())
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/edgion_acme_test.rs"]
mod tests;
