//! ForwardAuth plugin configuration.
//!
//! Sends execution-time request metadata to an external authentication service.
//! If the auth service responds with 2xx, the request is forwarded to upstream.
//! Otherwise, the auth service's response is returned to the client.

use crate::gwapi_types::is_valid_header_token;
use crate::gwapi_types::HEADER_NAME_MAX_LEN;
use crate::response_header_policy::validate_response_header_mutation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{ExternalHttpConnection, OutboundRequestSpec, ResolvedSecrets};

// ============================================================
// ForwardAuthDecision
// ============================================================

/// Decision-phase configuration for the ForwardAuth plugin.
///
/// Controls what happens after the auth service responds: which headers to
/// propagate upstream or back to the client, and timing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub struct ForwardAuthDecision {
    /// Headers to copy from the auth service response to the upstream request
    /// (forwarded to upstream on success).
    ///
    /// Example: ["X-User-ID", "X-User-Role"]
    #[serde(default)]
    pub upstream_headers: Vec<String>,

    /// Headers to copy from the auth service response to the client response
    /// (only when auth fails, i.e., non-2xx response).
    ///
    /// Example: ["WWW-Authenticate", "X-Auth-Error"]
    #[serde(default)]
    pub client_headers: Vec<String>,

    /// Remove the Authorization header from the upstream request after
    /// successful authentication.
    ///
    /// Default: false.
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    ///
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,
}

/// Serde default for `request`: forward_auth forwards all request headers by
/// default (unlike the webhook plugin which defaults to an empty allow-list).
fn default_forward_auth_request() -> OutboundRequestSpec {
    let mut r = OutboundRequestSpec::default();
    r.headers.forward_all = true;
    r
}

// ============================================================
// ForwardAuthConfig
// ============================================================

/// ForwardAuth plugin configuration.
///
/// Sends execution-time request metadata to an external authentication service.
/// Earlier rewrites are visible in the automatic X-Forwarded-* headers, which
/// override configured same-name headers regardless of forwardAll.
/// If the auth service responds with 2xx, the request is forwarded to upstream.
/// Otherwise, the auth service's response is returned to the client.
///
/// ## Examples
///
/// ### Basic: forward all request headers to auth service
/// ```yaml
/// target:
///   name: auth-service
///   port: 8080
/// decision:
///   upstreamHeaders:
///     - X-User-ID
///     - X-User-Role
/// ```
///
/// ### Selective: forward only specific headers
/// ```yaml
/// target:
///   name: auth-service
///   port: 8443
/// tls:
///   verify: false
/// timeoutMs: 5000
/// statusOnError: 503
/// request:
///   headers:
///     origin:
///       - name: Authorization
///       - name: Cookie
/// decision:
///   upstreamHeaders:
///     - X-User-ID
///   clientHeaders:
///     - WWW-Authenticate
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForwardAuthConfig {
    /// Connection-level configuration: target, TLS, timeout, retry,
    /// rate-limit, health-check, and response-handling.
    #[serde(flatten)]
    pub conn: ExternalHttpConnection,

    /// Outbound request specification: which path, method, args, headers,
    /// and cookies are forwarded to the auth service.
    ///
    /// Default: forward all request headers (forward_auth default behavior).
    /// Omitted method/path use entry values; explicit templates may read current
    /// values. Ordinary headers are read at execution time, before automatic metadata.
    #[serde(default = "default_forward_auth_request")]
    pub request: OutboundRequestSpec,

    /// Forward the complete current committed request body to the authentication
    /// service. The plugin entry must also declare its body policy and limit.
    #[serde(default)]
    pub forward_body: bool,

    /// Decision-phase configuration: header propagation and timing.
    pub decision: ForwardAuthDecision,

    /// Controller-resolved `${secretRef:...}` values carried to the gateway
    /// over conf_sync. Never exposed in the CRD (schemars skip) and shown as
    /// `[redacted]` in admin API output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_secrets: Option<ResolvedSecrets>,
}

impl Default for ForwardAuthConfig {
    fn default() -> Self {
        let mut request = OutboundRequestSpec::default();
        // forward_auth default: forward all request headers to the auth service
        request.headers.forward_all = true;
        Self {
            conn: ExternalHttpConnection::default(),
            request,
            forward_body: false,
            decision: ForwardAuthDecision::default(),
            resolved_secrets: None,
        }
    }
}

impl ForwardAuthConfig {
    /// Validate the configuration and return the first error found.
    pub fn validation_error(&self) -> Option<String> {
        // Validate the target (port required, kind must be supported)
        if let Some(e) = crate::resources::common::validate_connection_target(&self.conn) {
            return Some(format!("target invalid: {e}"));
        }
        // Validate status_on_error range
        if !crate::http_status::is_valid_final_http_status(self.conn.status_on_error) {
            return Some(format!(
                "statusOnError must be 200-599, got {}",
                self.conn.status_on_error
            ));
        }
        if self.forward_body && self.request.body.is_some() {
            return Some(
                "forwardBody and request.body are mutually exclusive; configure only one body source".to_string(),
            );
        }
        // Delegate request-level validation (rejects forward_all on args/cookies)
        for name in &self.decision.client_headers {
            if name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name) {
                return Some(
                    "decision.clientHeaders must contain valid HTTP header names of at most 256 bytes".to_string(),
                );
            }
            if let Err(error) = validate_response_header_mutation(name) {
                return Some(error.to_string());
            }
        }
        if let Some(e) = self.request.validation_error() {
            return Some(e);
        }
        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for ForwardAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.validation_error()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/forward_auth_test.rs"]
mod tests;
