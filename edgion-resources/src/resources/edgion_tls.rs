use super::common::ParentReference;

crate::redact::impl_redacted_debug!(ClientAuthConfig, EdgionTlsSpec);
use super::gateway::SecretObjectReference;
use super::http_route::RouteParentStatus;
use crate::constants::annotations;
use crate::constants::secret_keys::tls::{CERT, KEY};
use crate::ListenerKey;
use k8s_openapi::api::core::v1::Secret;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Gateway API Route-compatible cardinality bounds. These keep the
/// attachment index bounded before the Controller expands parent/listener
/// attachments and the Gateway combines them with host patterns.
pub const MAX_EDGION_TLS_PARENT_REFS: usize = 32;
pub const MAX_EDGION_TLS_HOSTS: usize = 16;
pub const MAX_EDGION_TLS_RESOLVED_ATTACHMENTS: usize = 2_048;

/// Validate operator-controlled dimensions after wire decoding. This must not
/// be a serde hook: Kubernetes may already have admitted an object before a
/// newer controller adds a limit, and poisoning the typed watch would retain
/// the previous data-plane generation instead of publishing a fail-closed one.
pub fn validate_operator_attachment_dimensions(
    spec: &EdgionTlsSpec,
    owner_namespace: &str,
) -> Result<(), &'static str> {
    let parent_refs = spec.parent_refs.as_deref().unwrap_or_default();
    if parent_refs.len() > MAX_EDGION_TLS_PARENT_REFS {
        return Err("parentRefs exceeds the runtime limit");
    }
    if spec.hosts.len() > MAX_EDGION_TLS_HOSTS {
        return Err("hosts exceeds the runtime limit");
    }

    let mut parents = HashSet::with_capacity(parent_refs.len());
    for parent in parent_refs {
        let identity = (
            parent.group.as_deref().unwrap_or("gateway.networking.k8s.io"),
            parent.kind.as_deref().unwrap_or("Gateway"),
            parent.namespace.as_deref().unwrap_or(owner_namespace),
            parent.name.as_str(),
            parent.section_name.as_deref().unwrap_or(""),
            parent.port,
        );
        if !parents.insert(identity) {
            return Err("parentRefs contains a duplicate attachment");
        }
    }

    let mut hosts = HashSet::with_capacity(spec.hosts.len());
    if spec.hosts.iter().any(|host| !hosts.insert(host.to_ascii_lowercase())) {
        return Err("hosts contains a duplicate hostname");
    }
    Ok(())
}

pub use super::allowed_san::{AllowedSan, AllowedSanMatch, AllowedSanType};

/// TLS protocol version (similar to Cloudflare's Minimum TLS Version).
///
/// **Security note**: `Tls10` and `Tls11` are **formally deprecated by IETF
/// RFC 8996 (2021)** and are no longer supported by modern browsers
/// (Chrome 84+ / Firefox 78+ / Safari 14+ / Edge 84+). Choose them only
/// when you must interoperate with known legacy clients (e.g., embedded
/// IoT, older payment terminals) and understand the risk. For all other
/// deployments use `Tls12` or `Tls13`.
///
/// When `min_tls_version` is not set, BoringSSL's default (TLS 1.2+)
/// applies — which is the safe path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TlsVersion {
    /// TLS 1.0 — deprecated by RFC 8996; legacy-client use only.
    #[serde(rename = "TLS1_0")]
    Tls10,
    /// TLS 1.1 — deprecated by RFC 8996; legacy-client use only.
    #[serde(rename = "TLS1_1")]
    Tls11,
    /// TLS 1.2 — recommended baseline for all modern deployments.
    #[serde(rename = "TLS1_2")]
    Tls12,
    /// TLS 1.3 — strongest; recommended when all clients support it.
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
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Controller-resolved CA Secret. Populated at pre-parse time and carried to the
    /// Gateway via conf_sync (redacted in admin output). `#[schemars(skip)]` keeps it
    /// out of the CRD schema so the K8s API server prunes it from user-submitted CRs.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub ca_secret: Option<Secret>,

    /// Optional typed Subject Alternative Name allowlist.
    /// Omit or empty means no extra SAN constraint. A non-empty list is any-match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_sans: Option<Vec<AllowedSan>>,

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

impl ClientAuthConfig {
    /// Compile RegularExpression SAN allowlist entries. Idempotent for other match kinds.
    pub fn compile_allowed_sans(&mut self) -> Result<(), String> {
        if let Some(entries) = &mut self.allowed_sans {
            for (index, entry) in entries.iter_mut().enumerate() {
                entry.compile().map_err(|err| format!("allowedSans[{index}]: {err}"))?;
            }
        }
        Ok(())
    }
}

#[derive(CustomResource, Serialize, Deserialize, JsonSchema)]
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
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<EdgionTlsStatus>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_EDGION_TLS_PARENT_REFS))]
    pub parent_refs: Option<Vec<ParentReference>>,
    #[schemars(
        length(max = MAX_EDGION_TLS_HOSTS),
        inner(
            length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
            pattern(crate::gwapi_types::HOSTNAME_PATTERN)
        )
    )]
    pub hosts: Vec<String>,
    pub secret_ref: SecretObjectReference,
    /// mTLS client authentication configuration (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_auth: Option<ClientAuthConfig>,
    /// Minimum TLS version accepted on this listener (optional, similar
    /// to Cloudflare's Minimum TLS Version).
    ///
    /// Options: `TLS1_0`, `TLS1_1`, `TLS1_2`, `TLS1_3`.
    ///
    /// **Recommended: leave unset (BoringSSL default = TLS 1.2+) or set
    /// explicitly to `TLS1_2` / `TLS1_3`.** `TLS1_0` and `TLS1_1` are
    /// deprecated by IETF RFC 8996 and are accepted only for explicit
    /// legacy-client compatibility — the operator takes responsibility
    /// for the downgrade. See `TlsVersion` docs for details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_tls_version: Option<TlsVersion>,
    /// Cipher list in OpenSSL format (optional, similar to Nginx ssl_ciphers)
    /// Example: ["ECDHE-RSA-AES256-GCM-SHA384", "ECDHE-RSA-AES128-GCM-SHA256"]
    /// If not configured, uses BoringSSL default ciphers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ciphers: Option<Vec<String>>,
    /// Resolved cert Secret (filled by Controller from `secret_ref`, never from user YAML).
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub secret: Option<Secret>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[schemars(skip)]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Controller-captured logical attachment identities for each resolved
    /// Gateway listener. The physical `ListenerKey` alone is insufficient when
    /// several Gateways share an address: the data plane uses this complete
    /// identity to prevent an EdgionTls from overlaying another Gateway's
    /// frontend client-auth policy after a collision or config generation swap.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[schemars(skip)]
    pub resolved_listener_attachments: Option<Vec<ResolvedListenerAttachment>>,

    /// Gateway-local exact attachment lookup compiled before matcher publication.
    /// It is never serialized; conf-sync transports the ordered attachment vector
    /// and the data plane warms this index on its cold configuration path.
    #[serde(skip)]
    #[schemars(skip)]
    pub resolved_listener_attachment_index: std::sync::OnceLock<std::collections::HashSet<ResolvedListenerAttachment>>,

    /// Operator log labels resolved from `metadata` (annotations / labels) at
    /// `pre_parse` time, attached to `rsc.labels` in `ssl.log`. Pure in-process
    /// cache derived from metadata the gateway already holds: never part of the
    /// CRD (`schemars(skip)`) nor the conf_sync payload (`serde(skip)`), and
    /// re-derived on every gateway ingest. Caching here keeps the TLS handshake
    /// callback off the metadata-parsing path.
    #[serde(skip)]
    #[schemars(skip)]
    pub resolved_log_labels: Option<std::sync::Arc<std::collections::BTreeMap<String, String>>>,
}

impl Clone for EdgionTlsSpec {
    fn clone(&self) -> Self {
        Self {
            current_status: self.current_status.clone(),
            parent_refs: self.parent_refs.clone(),
            hosts: self.hosts.clone(),
            secret_ref: self.secret_ref.clone(),
            client_auth: self.client_auth.clone(),
            min_tls_version: self.min_tls_version,
            ciphers: self.ciphers.clone(),
            secret: self.secret.clone(),
            resolved_listeners: self.resolved_listeners.clone(),
            resolved_listener_attachments: self.resolved_listener_attachments.clone(),
            // A clone may be mutated into another conf-sync generation. Never
            // carry a compiled index across that generation boundary.
            resolved_listener_attachment_index: Default::default(),
            resolved_log_labels: self.resolved_log_labels.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedListenerAttachment {
    pub gateway_namespace: String,
    pub gateway_name: String,
    pub listener_name: String,
    pub listener_key: ListenerKey,
    pub gateway_proof: GatewayAttachmentProof,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAttachmentProof {
    pub instance: String,
    pub generation: i64,
    pub epoch: String,
    pub attachment_digest: String,
}

impl EdgionTlsSpec {
    pub fn warm_listener_attachment_index(&self) {
        let _ = self.resolved_listener_attachment_index.get_or_init(|| {
            self.resolved_listener_attachments
                .as_deref()
                .unwrap_or_default()
                .iter()
                .cloned()
                .collect()
        });
    }

    pub fn has_resolved_listener_attachment(&self, attachment: &ResolvedListenerAttachment) -> bool {
        self.resolved_listener_attachment_index
            .get()
            .is_some_and(|index| index.contains(attachment))
    }
}

impl GatewayAttachmentProof {
    pub fn is_resolved(&self) -> bool {
        fn valid_token(value: &str) -> bool {
            !value.is_empty() && value != "unresolved"
        }
        valid_token(&self.instance)
            && valid_token(&self.epoch)
            && self.generation >= 0
            && self.attachment_digest.len() == 64
            && self
                .attachment_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// EdgionTlsStatus describes the status of the EdgionTls resource
/// Following Gateway API RouteParentStatus pattern for parent-based status
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionTlsStatus {
    /// Parents describe the status of the TLS config with respect to each parent Gateway.
    ///
    /// Always serialized, even when empty: K8s status persistence uses JSON
    /// Merge Patch, and merge-patching an empty status object `{}` cannot
    /// clear a previously-persisted non-empty list — only an explicit
    /// `"parents": []` replaces it (reachable when all parentRefs are removed
    /// from a live EdgionTls). Do not add skip_serializing_if here.
    #[serde(default)]
    pub parents: Vec<RouteParentStatus>,
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
            .get(CERT)
            .ok_or_else(|| anyhow::anyhow!("Secret data {} not found", CERT))?;

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
            .get(KEY)
            .ok_or_else(|| anyhow::anyhow!("Secret data {} not found", KEY))?;

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

    /// Whether mTLS client certificate info should be exposed to plugins.
    ///
    /// Controlled by annotation `edgion.io/expose-client-cert: "true"`.
    pub fn should_expose_client_cert(&self) -> bool {
        self.metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(annotations::edgion::EXPOSE_CLIENT_CERT))
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    pub fn matches_hostname(&self, hostname: &str) -> bool {
        let hostname_lower = hostname.to_lowercase();

        for host in &self.spec.hosts {
            let host_lower = host.to_lowercase();

            // Exact match
            if host_lower == hostname_lower {
                return true;
            }

            // Wildcard match: only allow * at the beginning in "*.*.*.domain" format
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
        let mut has_exact_match = false; // Track if we've seen any exact match segment

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

                // Rule: wildcard cannot appear after exact match
                if has_exact_match {
                    return false;
                }

                // Find the next dot in hostname (or end)
                if h_idx >= hostname_len {
                    return false; // No more hostname to match
                }

                let h_segment_start = h_idx;
                let mut h_segment_end = h_idx;
                while h_segment_end < hostname_len && hostname_bytes[h_segment_end] != b'.' {
                    h_segment_end += 1;
                }

                // Wildcard must match at least one character
                if h_segment_end == h_segment_start {
                    return false;
                }

                // Move hostname pointer past this segment
                h_idx = h_segment_end;
            } else {
                // This is an exact match segment
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

// Unit tests for this module live in `tests/edgion_tls_test.rs`.
#[cfg(test)]
#[path = "../../tests/unit/resources/edgion_tls_test.rs"]
mod tests;
