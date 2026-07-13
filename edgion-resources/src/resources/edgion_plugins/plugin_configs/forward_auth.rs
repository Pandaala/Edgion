//! ForwardAuth plugin configuration.
//!
//! Sends the original request metadata to an external authentication service.
//! If the auth service responds with 2xx, the request is forwarded to upstream.
//! Otherwise, the auth service's response is returned to the client.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{ExternalHttpConnection, OutboundRequestSpec, ResolvedSecrets};

// ============================================================
// ForwardAuthDecision
// ============================================================

/// Decision-phase configuration for the ForwardAuth plugin.
///
/// Controls what happens after the auth service responds: which headers to
/// propagate upstream or back to the client, degradation behavior, and timing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

impl Default for ForwardAuthDecision {
    fn default() -> Self {
        Self {
            upstream_headers: Vec::new(),
            client_headers: Vec::new(),
            hide_credentials: false,
            auth_failure_delay_ms: 0,
        }
    }
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
/// Sends the original request metadata to an external authentication service.
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
/// ### Selective: forward only specific headers, with degradation
/// ```yaml
/// target:
///   name: auth-service
///   port: 8443
/// tls:
///   verify: false
/// timeoutMs: 5000
/// allowDegradation: true
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
    #[serde(default = "default_forward_auth_request")]
    pub request: OutboundRequestSpec,

    /// Decision-phase configuration: header propagation, degradation behavior,
    /// error status, and timing.
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
        if !(200..=599).contains(&self.conn.status_on_error) {
            return Some(format!(
                "statusOnError must be 200-599, got {}",
                self.conn.status_on_error
            ));
        }
        // Delegate request-level validation (rejects forward_all on args/cookies)
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
mod tests {
    use super::*;

    /// New schema parses correctly: conn.target resolves port, request defaults
    /// to forward_all=true, and validation passes.
    #[test]
    fn forward_auth_new_schema_parses() {
        let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 503\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        assert_eq!(cfg.conn.target.service.port, Some(8080));
        assert!(cfg.request.headers.forward_all); // forward_auth Default sets true
        assert!(cfg.validation_error().is_none());
    }

    /// Missing port must produce a validation error.
    #[test]
    fn forward_auth_rejects_missing_port() {
        let y = "target:\n  name: auth\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        assert!(cfg.validation_error().is_some());
    }

    /// statusOnError / allowDegradation now live on the flattened conn, at the top level.
    #[test]
    fn forward_auth_failure_policy_is_top_level() {
        let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 502\nallowDegradation: true\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        assert_eq!(cfg.conn.status_on_error, 502);
        assert!(cfg.conn.allow_degradation);
        assert!(cfg.validation_error().is_none());
    }

    /// A stale `decision.allowDegradation` (the pre-refactor nested location)
    /// must be rejected at deserialization time, not silently dropped.
    #[test]
    fn forward_auth_rejects_stale_decision_allow_degradation() {
        let y = "target:\n  name: auth\n  port: 8080\ndecision:\n  allowDegradation: true\n";
        let err = serde_yaml::from_str::<ForwardAuthConfig>(y)
            .expect_err("stale decision.allowDegradation must fail to deserialize");
        assert!(
            err.to_string().contains("allowDegradation") || err.to_string().contains("unknown field"),
            "error should call out the unknown field; got: {err}"
        );
    }

    /// A stale `decision.statusOnError` (the pre-refactor nested location)
    /// must be rejected at deserialization time, not silently dropped.
    #[test]
    fn forward_auth_rejects_stale_decision_status_on_error() {
        let y = "target:\n  name: auth\n  port: 8080\ndecision:\n  statusOnError: 500\n";
        let err = serde_yaml::from_str::<ForwardAuthConfig>(y)
            .expect_err("stale decision.statusOnError must fail to deserialize");
        assert!(
            err.to_string().contains("statusOnError") || err.to_string().contains("unknown field"),
            "error should call out the unknown field; got: {err}"
        );
    }

    /// statusOnError = 600 is out of range (200-599) and must be rejected.
    #[test]
    fn forward_auth_rejects_status_on_error_600() {
        let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 600\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        let err = cfg.validation_error().expect("600 should fail validation");
        assert!(
            err.contains("statusOnError"),
            "Error should mention statusOnError; got: {err}"
        );
    }

    /// Default::default() must set request.headers.forward_all = true (the
    /// forward_auth default behavior contract).
    #[test]
    fn forward_auth_default_headers_forward_all_is_true() {
        let cfg = ForwardAuthConfig::default();
        assert!(
            cfg.request.headers.forward_all,
            "Default ForwardAuthConfig must have request.headers.forward_all = true"
        );
    }

    #[test]
    fn forward_auth_default_passes_own_validation() {
        let cfg = ForwardAuthConfig::default();
        assert!(
            (200..=599).contains(&cfg.conn.status_on_error),
            "Default ForwardAuthConfig must pass status_on_error validation, got: {}",
            cfg.conn.status_on_error
        );
    }

    /// IMPORTANT 3 regression: `timeoutMs: 0` used to be admitted, and
    /// `tokio::time::timeout(ZERO, ..)` fires instantly on every request — with
    /// `allowDegradation: true` that is a 100% silent auth bypass. The check now
    /// lives in the shared `validate_connection_target` (exercised here via
    /// forward_auth's own validator, not just webhook's).
    #[test]
    fn forward_auth_rejects_zero_timeout_ms() {
        let y = "target:\n  name: auth\n  port: 8080\ntimeoutMs: 0\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        let err = cfg.validation_error().expect("timeoutMs: 0 must be rejected");
        assert!(err.contains("timeoutMs"), "got: {err}");
    }

    /// IMPORTANT 3 regression: `maxResponseBytes: 0` used to be admitted, and every
    /// response then trips `ResponseTooLarge` -> Err -> degrade, which is also a
    /// silent 100% bypass under `allowDegradation: true`.
    #[test]
    fn forward_auth_rejects_zero_max_response_bytes() {
        let y = "target:\n  name: auth\n  port: 8080\nmaxResponseBytes: 0\ndecision: {}\n";
        let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        let err = cfg.validation_error().expect("maxResponseBytes: 0 must be rejected");
        assert!(err.contains("maxResponseBytes"), "got: {err}");
    }

    #[test]
    fn forward_auth_inherits_body_content_type_conflict() {
        use crate::resources::common::outbound_request::{BodySpec, CustomField, CustomSource};
        let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 503\ndecision: {}\n";
        let mut cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
        cfg.request.body = Some(BodySpec::Json {
            template: r#"{"user": "${ctx:user}"}"#.to_string(),
        });
        cfg.request.headers.custom.push(CustomField {
            name: "Content-Type".to_string(),
            source: CustomSource::Value("text/plain".to_string()),
        });
        let err = cfg
            .validation_error()
            .expect("forward_auth must inherit body validation");
        assert!(err.contains("Content-Type"), "got: {err}");
    }
}
