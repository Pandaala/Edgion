//! Webhook service configuration for LinkSys
//!
//! Defines the connection parameters and health check settings for an external
//! HTTP webhook service. Referenced by the WebhookKeyGet plugin via "namespace/name".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{ExternalHttpConnection, ListFields, OutboundRequestSpec, ResolvedSecrets};

// ============================================================
// WebhookServiceConfig — main configuration
// ============================================================

/// Webhook HTTP service configuration.
///
/// Defines how to connect to an external HTTP service used for key resolution,
/// identity mapping, or other webhook-based lookups.
///
/// ## YAML Example (LinkSys CRD)
///
/// Kubernetes Service target:
/// ```yaml
/// apiVersion: edgion.io/v1
/// kind: LinkSys
/// metadata:
///   name: key-resolver
///   namespace: prod
/// spec:
///   type: webhook
///   config:
///     target:
///       name: "my-webhook-svc"
///       namespace: "prod"
///       port: 8080
///     request:
///       path:
///         template: "/webhook"
///       method:
///         template: "POST"
///       headers:
///         origin:
///           - name: "X-Tenant-Id"
///       custom:
///         - name: "X-Source"
///           value: "edgion"
///     timeoutMs: 3000
///     success:
///       statusCodes: [200]
///     statusOnError: 503
///     healthCheck:
///       active:
///         path: "/healthz"
///         intervalSec: 10
///         unhealthyThreshold: 3
/// ```
///
/// NOTE: `deny_unknown_fields` is intentionally absent here — `#[serde(flatten)]`
/// disables it anyway (spec §3.2); removing it makes the relaxation explicit.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct WebhookServiceConfig {
    // ============================================================
    // Connection (flattened — target, tls, timeout, retry, rate_limit,
    // health_check, max_response_bytes, success)
    // ============================================================
    /// Shared external HTTP connection configuration.
    /// Contains target, TLS, timeout, retry, rate-limit, health-check,
    /// max_response_bytes, and success.
    #[serde(flatten)]
    pub conn: ExternalHttpConnection,

    // ============================================================
    // Outbound Request
    // ============================================================
    /// Outbound webhook request specification.
    /// Defines the allowed request shape: path, method, args, headers, cookies.
    /// Default: empty (no fields forwarded).
    #[serde(default)]
    pub request: OutboundRequestSpec,

    /// Controller-resolved `${secretRef:...}` values, carried inline to the
    /// gateway. Keyed by the raw template var_name (`"ns/name:key"`). Never set
    /// by operators (`#[schemars(skip)]` — the API server prunes it); redacted in
    /// admin output; shipped real over conf_sync.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_secrets: Option<ResolvedSecrets>,
}

// ============================================================
// Default + Validation
// ============================================================

impl WebhookServiceConfig {
    /// Validate the configuration and return an error message if invalid.
    ///
    pub fn get_validation_error(&self) -> Option<String> {
        // Validate the target
        if let Some(err) = crate::resources::common::validate_connection_target(&self.conn) {
            return Some(err);
        }

        // Note: `1 <= timeoutMs <= MAX_OUTBOUND_TIMEOUT_MS` and `maxResponseBytes > 0`
        // are validated by `validate_connection_target` above (shared with
        // forward_auth), so both consumers inherit the same checks instead of
        // duplicating them here.

        // Validate status on error (now on the shared connection struct)
        if !crate::http_status::is_valid_final_http_status(self.conn.status_on_error) {
            return Some("statusOnError must be between 200 and 599".to_string());
        }

        // Validate the request field-name invariants (args/headers/cookies).
        if let Some(err) = validate_request_fields(&self.request) {
            return Some(err);
        }

        None
    }

    /// Collect the raw var_names of every `${secretRef:...}` referenced across
    /// the outbound request templates (path, method, args/headers/cookies
    /// custom). Delegates to `OutboundRequestSpec::collect_secret_ref_names`.
    pub fn collect_secret_ref_names(&self) -> Vec<String> {
        self.request.collect_secret_ref_names()
    }

    /// Referenced `secretRef`s that are absent from `resolved_secrets`
    /// (i.e. the controller has not resolved them). Used by the gateway
    /// readiness gate to refuse a webhook with unresolved secrets.
    pub fn unresolved_secret_refs(&self) -> Vec<String> {
        let resolved = self.resolved_secrets.as_ref();
        self.collect_secret_ref_names()
            .into_iter()
            .filter(|name| match resolved {
                Some(rs) => !rs.0.contains_key(name),
                None => true,
            })
            .collect()
    }
}

/// Validate field-name invariants across the request's args/headers/cookies.
///
/// Four spec-MUST edge cases are enforced:
/// 0. Semantic constraints on the spec itself (e.g. forwardAll on non-header groups).
/// 1. A field name appearing in both `origin` and `custom` of the same group.
/// 2. Duplicate names within `origin`, or within `custom`, of the same group.
/// 3. A `headers.origin` entry named `Cookie` conflicting with a non-empty `cookies.*`.
///
/// Header names are compared case-insensitively (per HTTP spec); cookie names
/// (and arg names) are compared case-sensitively. The first error encountered
/// is returned.
fn validate_request_fields(request: &OutboundRequestSpec) -> Option<String> {
    // Validation 0: spec-level semantic constraints (e.g. forwardAll on args/cookies).
    if let Some(err) = request.validation_error() {
        return Some(err);
    }

    // args and cookies: case-sensitive names.
    if let Some(err) = validate_field_group(&request.args, false) {
        return Some(err);
    }
    // headers: case-insensitive names.
    if let Some(err) = validate_field_group(&request.headers, true) {
        return Some(err);
    }
    if let Some(err) = validate_field_group(&request.cookies, false) {
        return Some(err);
    }

    // Validation 3: headers.origin[name: Cookie] conflicts with any cookies.*.
    let has_cookie_header = request
        .headers
        .origin
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case("Cookie"));
    let has_cookies = !request.cookies.origin.is_empty() || !request.cookies.custom.is_empty();
    if has_cookie_header && has_cookies {
        return Some(
            "cannot specify both headers.origin[name:Cookie] and cookies.* — use one or the other".to_string(),
        );
    }

    None
}

/// Validate a single field group (args, headers, or cookies).
///
/// `case_insensitive` controls whether names are normalized to lowercase before
/// comparison (true for headers). Checks, in order:
/// - duplicate names within `origin`,
/// - duplicate names within `custom`,
/// - a name shared between `origin` and `custom`.
fn validate_field_group(group: &ListFields, case_insensitive: bool) -> Option<String> {
    let norm = |name: &str| -> String {
        if case_insensitive {
            name.to_lowercase()
        } else {
            name.to_string()
        }
    };

    // Validation 2: duplicate names within origin.
    let mut origin_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for field in &group.origin {
        if !origin_names.insert(norm(&field.name)) {
            return Some(format!("duplicate field name '{}' in origin", field.name));
        }
    }

    // Validation 2: duplicate names within custom.
    let mut custom_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for field in &group.custom {
        if !custom_names.insert(norm(&field.name)) {
            return Some(format!("duplicate field name '{}' in custom", field.name));
        }
    }

    // Validation 1: a name appearing in both origin and custom.
    for field in &group.custom {
        if origin_names.contains(&norm(&field.name)) {
            return Some(format!("field '{}' appears in both origin and custom", field.name));
        }
    }

    None
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/webhook_test/webhook_outbound_request_tests.rs"]
mod webhook_outbound_request_tests;

// Unit tests for this module live in `tests/webhook_test.rs`.
#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/webhook_test.rs"]
mod tests;
