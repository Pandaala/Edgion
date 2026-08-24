//! Webhook service configuration for LinkSys
//!
//! Defines the connection parameters and health check settings for an external
//! HTTP webhook service. Referenced by the WebhookKeyGet plugin via "namespace/name".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::SecretSlot;

// Re-export types moved to common::external_http so that existing callers using
// `link_sys::webhook::{WebhookTarget, WebhookRetry, ExternalHttpConnection, …}` continue to compile.
pub use crate::resources::common::external_http::{
    validate_connection_target, validate_webhook_target, ActiveHealthCheck, ExternalHttpConnection, PassiveBackoff,
    PassiveHealthCheck, TargetKind, WebhookHealthCheck, WebhookRateLimit, WebhookRetry, WebhookServiceRef,
    WebhookTarget,
};

// Re-export the shared outbound-request types so that existing callers using
// `link_sys::webhook::{ListFields, CustomField, ...}` continue to compile.
pub use crate::resources::common::outbound_request::{
    BodySpec, CustomField, CustomSource, ListFields, MethodSpec, OriginField, OutboundRequestSpec, PathSpec,
    PresencePolicy,
};

// Re-export ResolvedSecrets so existing callers using
// `link_sys::webhook::ResolvedSecrets` continue to compile.
pub use crate::resources::common::external_http::ResolvedSecrets;

/// Backward-compatibility alias: webhook code historically used `WebhookOutboundRequest`;
/// downstream crates importing from `link_sys::webhook` still compile unchanged.
pub type WebhookOutboundRequest = OutboundRequestSpec;

// ============================================================
// WebhookAuth — authentication configuration
// ============================================================

/// Webhook authentication configuration.
///
/// Uses `type` field to select auth method. All methods require a Secret reference.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WebhookAuth {
    /// Basic authentication (username + password in Secret)
    Basic {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// Bearer token authentication (bearerToken in Secret)
    Bearer {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// HMAC signature authentication (hmacKey in Secret)
    Hmac {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// API Key authentication — injects the key as a header or query parameter.
    /// The secret must contain a `apiKey` data key.
    ApiKey {
        #[serde(flatten)]
        slot: SecretSlot,
        /// Where to inject the key: "header" or "query"
        r#in: String,
        /// Header name when `in = "header"` (e.g. "X-API-Key")
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "headerName")]
        header_name: Option<String>,
        /// Query parameter name when `in = "query"` (e.g. "api_key")
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "queryParam")]
        query_param: Option<String>,
    },
}

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
    // Authentication (DEPRECATED - shim field)
    // ============================================================
    /// Authentication configuration (DEPRECATED).
    ///
    /// **DEPRECATED:** This field accepts the old authentication format for backward compatibility
    /// during rolling upgrades. It is not used at runtime in this release.
    ///
    /// Migrate to `request.headers.custom` with `source: secret` for custom header injection.
    /// This field will be removed in v1.X+1.
    ///
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,

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

    // ============================================================
    // Connection (DEPRECATED - shim field)
    // ============================================================
    /// HTTP method for the webhook request (DEPRECATED).
    ///
    /// **DEPRECATED:** This field is accepted for backward compatibility but not used at runtime.
    /// Migrate to the new `request.method` field in Wave B.
    ///
    /// Default: GET (when absent in new configs)
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_method: Option<serde_json::Value>,

    /// Static headers sent on every outbound call (DEPRECATED - shim field).
    ///
    /// **DEPRECATED:** This field accepts the old header format for backward compatibility.
    /// It is not used at runtime in this release.
    ///
    /// Migrate to the new `request.headers` field in Wave B.
    /// This field will be removed in v1.X+1.
    ///
    /// Accepted but ignored in this release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_headers: Option<serde_json::Value>,
}

// ============================================================
// Default + Validation
// ============================================================

impl Default for WebhookServiceConfig {
    fn default() -> Self {
        Self {
            conn: ExternalHttpConnection::default(),
            auth: None,
            request: OutboundRequestSpec::default(),
            resolved_secrets: None,
            request_method: None,
            default_headers: None,
        }
    }
}

impl WebhookServiceConfig {
    /// Validate the configuration and return an error message if invalid.
    ///
    /// Note: Deprecated shim fields (`auth`, `request_method`, `default_headers`)
    /// are not validated; they are accepted for backward compatibility but not used at runtime.
    pub fn get_validation_error(&self) -> Option<String> {
        // Validate the target
        if let Some(err) = crate::resources::common::validate_connection_target(&self.conn) {
            return Some(err);
        }

        // Note: request_method is now a deprecated shim field and is not validated.
        // It was previously validated as a valid HTTP method, but is no longer used at runtime.

        // Note: `1 <= timeoutMs <= MAX_OUTBOUND_TIMEOUT_MS` and `maxResponseBytes > 0`
        // are validated by `validate_connection_target` above (shared with
        // forward_auth), so both consumers inherit the same checks instead of
        // duplicating them here.

        // Validate status on error (now on the shared connection struct)
        if !(200..=599).contains(&self.conn.status_on_error) {
            return Some("statusOnError must be between 200 and 599".to_string());
        }

        // Validate the request field-name invariants (args/headers/cookies).
        if let Some(err) = validate_request_fields(&self.request) {
            return Some(err);
        }

        // Validate every secretRef segment in every request template: must be a
        // well-formed `${secretRef:ns/name:key}`. This also catches a bare
        // `${secretRef}` (no name) that `collect_secret_ref_names` skips.
        use crate::resources::edgion_plugins::plugin_configs::{validate_secret_ref_segments, CompiledTemplate};
        let mut templates_to_check: Vec<&str> = Vec::new();
        if let Some(p) = &self.request.path {
            templates_to_check.push(&p.template);
        }
        if let Some(m) = &self.request.method {
            templates_to_check.push(&m.template);
        }
        for group in [&self.request.args, &self.request.headers, &self.request.cookies] {
            for f in &group.custom {
                if let CustomSource::Template(t) = &f.source {
                    templates_to_check.push(t);
                }
            }
        }
        match &self.request.body {
            Some(BodySpec::Json { template }) | Some(BodySpec::Raw { template }) => {
                templates_to_check.push(template);
            }
            Some(BodySpec::Form { fields }) => {
                for f in fields {
                    if let CustomSource::Template(t) = &f.source {
                        templates_to_check.push(t);
                    }
                }
            }
            None => {}
        }
        for t in templates_to_check {
            if let Ok(compiled) = CompiledTemplate::compile(t) {
                if let Err(e) = validate_secret_ref_segments(&compiled) {
                    return Some(e);
                }
            }
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
mod webhook_outbound_request_tests {
    use super::*;

    #[test]
    fn test_webhook_outbound_request_all_origin() {
        let yaml_str = r#"
args:
  origin:
    - name: tenant_id
    - name: trace_id
      presence: required
headers:
  origin:
    - name: X-Tenant-Id
cookies:
  origin:
    - name: session_id
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(request.args.origin.len(), 2);
        assert_eq!(request.headers.origin.len(), 1);
        assert_eq!(request.cookies.origin.len(), 1);
        assert!(request.path.is_none());
    }

    #[test]
    fn test_webhook_outbound_request_with_path_and_method() {
        let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args: {}
headers: {}
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert!(request.path.is_some());
        assert_eq!(request.path.as_ref().unwrap().template, "/webhook/${ctx:action}");
        assert_eq!(request.path.as_ref().unwrap().allow_override, true);
        assert!(request.method.is_some());
        assert_eq!(request.method.as_ref().unwrap().template, "POST");
    }

    #[test]
    fn test_custom_field_with_literal_value() {
        let yaml_str = r#"
- name: X-Source
  value: edgion
"#;
        let custom_fields: Vec<CustomField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(custom_fields.len(), 1);
        assert_eq!(custom_fields[0].name, "X-Source");
        match &custom_fields[0].source {
            CustomSource::Value(val) => {
                assert_eq!(val, "edgion");
            }
            _ => panic!("Expected Value source"),
        }
    }

    #[test]
    fn test_custom_field_with_template() {
        let yaml_str = r#"
- name: X-Action
  template: "${ctx:action}"
"#;
        let custom_fields: Vec<CustomField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(custom_fields.len(), 1);
        assert_eq!(custom_fields[0].name, "X-Action");
        match &custom_fields[0].source {
            CustomSource::Template(tpl) => {
                assert_eq!(tpl, "${ctx:action}");
            }
            _ => panic!("Expected Template source"),
        }
    }

    #[test]
    fn test_presence_policy_default() {
        let origin_field = OriginField {
            name: "X-Foo".to_string(),
            presence: Default::default(),
        };
        assert!(matches!(origin_field.presence, PresencePolicy::SkipIfMissing));
    }

    #[test]
    fn test_presence_policy_required() {
        let yaml_str = r#"
- name: X-Required
  presence: required
"#;
        let fields: Vec<OriginField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].presence, PresencePolicy::Required));
    }

    #[test]
    fn test_presence_policy_warn() {
        let yaml_str = r#"
- name: X-Warn
  presence: warnIfMissing
"#;
        let fields: Vec<OriginField> = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].presence, PresencePolicy::WarnIfMissing));
    }

    #[test]
    fn test_webhook_outbound_request_all_custom() {
        let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args:
  custom:
    - name: source
      value: edgion
    - name: action
      template: "${ctx:action}"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-default/webhook-token:token}"
"#;
        let request: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        assert!(request.path.is_some());
        assert!(request.method.is_some());
        assert_eq!(request.path.as_ref().unwrap().allow_override, true);
        assert_eq!(request.args.custom.len(), 2);
        assert_eq!(request.headers.custom.len(), 1);
    }

    #[test]
    fn test_list_fields_default() {
        let fields: ListFields = Default::default();
        assert_eq!(fields.origin.len(), 0);
        assert_eq!(fields.custom.len(), 0);
    }

    #[test]
    fn test_deny_unknown_fields_on_path_spec() {
        let yaml_str = r#"
template: "/webhook"
allowOverride: true
unknownField: value
"#;
        let result: Result<PathSpec, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_deny_unknown_fields_on_method_spec() {
        let yaml_str = r#"
template: "POST"
unknownField: value
"#;
        let result: Result<MethodSpec, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_deny_unknown_fields_on_webhook_outbound_request() {
        let yaml_str = r#"
path:
  template: "/webhook"
unknownField: value
"#;
        let result: Result<WebhookOutboundRequest, _> = serde_yaml::from_str(yaml_str);
        assert!(result.is_err());
    }

    /// Build an otherwise-valid config so request-field validation is the only
    /// thing that can fail.
    fn valid_base_config() -> WebhookServiceConfig {
        let mut config = WebhookServiceConfig::default();
        config.conn.target.service.name = Some("webhook-svc".to_string());
        config.conn.target.service.port = Some(8080);
        config
    }

    fn origin(name: &str) -> OriginField {
        OriginField {
            name: name.to_string(),
            presence: PresencePolicy::SkipIfMissing,
        }
    }

    fn custom(name: &str) -> CustomField {
        CustomField {
            name: name.to_string(),
            source: CustomSource::Value("x".to_string()),
        }
    }

    #[test]
    fn test_same_field_origin_and_custom_rejected() {
        // args: same name in origin and custom.
        let mut config = valid_base_config();
        config.request.args.origin = vec![origin("tenant_id")];
        config.request.args.custom = vec![custom("tenant_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("appears in both origin and custom"), "got: {err}");

        // headers: case-insensitive collision (X-Trace-Id vs x-trace-id).
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("X-Trace-Id")];
        config.request.headers.custom = vec![custom("x-trace-id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("appears in both origin and custom"), "got: {err}");

        // cookies: case-sensitive — distinct case must NOT collide.
        let mut config = valid_base_config();
        config.request.cookies.origin = vec![origin("Session")];
        config.request.cookies.custom = vec![custom("session")];
        assert!(
            config.get_validation_error().is_none(),
            "cookie names are case-sensitive; different case should be allowed"
        );
    }

    #[test]
    fn test_duplicate_names_in_origin_rejected() {
        // args duplicate in origin.
        let mut config = valid_base_config();
        config.request.args.origin = vec![origin("trace_id"), origin("trace_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("origin"),
            "got: {err}"
        );

        // headers duplicate in origin, case-insensitive.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("X-Id"), origin("x-id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("origin"),
            "got: {err}"
        );
    }

    #[test]
    fn test_duplicate_names_in_custom_rejected() {
        let mut config = valid_base_config();
        config.request.headers.custom = vec![custom("Authorization"), custom("Authorization")];
        let err = config.get_validation_error().expect("should reject");
        assert!(
            err.contains("duplicate field name") && err.contains("custom"),
            "got: {err}"
        );
    }

    #[test]
    fn test_cookie_both_origin_and_custom_rejected() {
        // headers.origin[name: Cookie] with cookies.origin present.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("Cookie")];
        config.request.cookies.origin = vec![origin("session_id")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

        // case-insensitive header name "cookie" with cookies.custom present.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("cookie")];
        config.request.cookies.custom = vec![custom("token")];
        let err = config.get_validation_error().expect("should reject");
        assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

        // Cookie header with no cookies.* configured is fine.
        let mut config = valid_base_config();
        config.request.headers.origin = vec![origin("Cookie")];
        assert!(config.get_validation_error().is_none(), "Cookie header alone is valid");
    }

    #[test]
    fn collect_secret_ref_names_across_fields() {
        let yaml_str = r#"
path:
  template: "/wh/${secretRef:ns/p:k}"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-system/tok:token}"
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        let mut names = cfg.collect_secret_ref_names();
        names.sort();
        assert_eq!(names, vec!["edgion-system/tok:token".to_string(), "ns/p:k".to_string()]);
    }

    #[test]
    fn unresolved_secret_refs_reports_missing() {
        let yaml_str = r#"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-system/tok:token}"
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        // No resolved_secrets -> reported unresolved.
        assert_eq!(
            cfg.unresolved_secret_refs(),
            vec!["edgion-system/tok:token".to_string()]
        );
    }

    #[test]
    fn validation_rejects_malformed_secret_ref() {
        let yaml_str = r#"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:tok:token}"
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        let err = cfg.get_validation_error().expect("must reject no-namespace secretRef");
        assert!(err.contains("secretRef"), "got: {err}");
    }

    #[test]
    fn resolved_secrets_debug_is_redacted() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("ns/n:k".to_string(), "supersecret".to_string());
        let rs = ResolvedSecrets(m);
        let dbg = format!("{:?}", rs);
        assert!(!dbg.contains("supersecret"), "Debug must not print values: {dbg}");
        assert!(dbg.contains("redacted"), "Debug should mark redaction: {dbg}");
    }

    #[test]
    fn resolved_secrets_redacts_under_guard_real_without() {
        use crate::redact::AdminRedactGuard;
        let mut m = std::collections::BTreeMap::new();
        m.insert("ns/n:k".to_string(), "supersecret".to_string());
        let wrapped: Option<ResolvedSecrets> = Some(ResolvedSecrets(m));

        // Under guard -> "[redacted]".
        {
            let _g = AdminRedactGuard::new();
            let json = serde_json::to_string(&RedactProbe {
                resolved_secrets: wrapped.clone(),
            })
            .unwrap();
            assert!(json.contains("[redacted]"), "guarded: {json}");
            assert!(!json.contains("supersecret"), "guarded leaked: {json}");
        }
        // Without guard -> real value (conf_sync transport).
        let json = serde_json::to_string(&RedactProbe {
            resolved_secrets: wrapped,
        })
        .unwrap();
        assert!(json.contains("supersecret"), "transport must ship real value: {json}");
    }

    #[derive(serde::Serialize)]
    struct RedactProbe {
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "crate::redact::redact_opt::serialize"
        )]
        resolved_secrets: Option<ResolvedSecrets>,
    }

    #[test]
    fn collect_secret_ref_names_includes_body_templates() {
        let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:prod/webhook:token}"}'
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        assert_eq!(cfg.collect_secret_ref_names(), vec!["prod/webhook:token".to_string()]);
    }

    #[test]
    fn collect_secret_ref_names_includes_form_field_templates() {
        let yaml_str = r#"
body:
  type: form
  fields:
    - name: token
      template: "${secretRef:prod/webhook:token}"
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        assert_eq!(cfg.collect_secret_ref_names(), vec!["prod/webhook:token".to_string()]);
    }

    #[test]
    fn validation_rejects_malformed_secret_ref_in_body() {
        // No namespace in the secretRef — validate_secret_ref_segments must reject.
        let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:tok:token}"}'
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        let err = cfg
            .get_validation_error()
            .expect("must reject malformed body secretRef");
        assert!(err.contains("secretRef"), "got: {err}");
    }

    #[test]
    fn unresolved_secret_refs_reports_body_only_ref() {
        let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:prod/webhook:token}"}'
"#;
        let req: WebhookOutboundRequest = serde_yaml::from_str(yaml_str).expect("deserialize");
        let cfg = WebhookServiceConfig {
            request: req,
            ..valid_base_config()
        };
        assert_eq!(cfg.unresolved_secret_refs(), vec!["prod/webhook:token".to_string()]);
    }
}

// Unit tests for this module live in `tests/webhook_test.rs`.
#[cfg(test)]
#[path = "tests/webhook_test.rs"]
mod tests;
