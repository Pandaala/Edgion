//! HTTPRoute resource definition
//!
//! HTTPRoute defines HTTP rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use super::common::{Condition, ParentReference, RefDenied};
use super::http_route_preparse::BackendExtensionInfo;
use crate::core::gateway::lb::BackendSelector;
use crate::core::gateway::plugins::PluginRuntime;
use crate::types::ListenerKey;

/// API group for HTTPRoute
pub const HTTP_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for HTTPRoute
pub const HTTP_ROUTE_KIND: &str = "HTTPRoute";

/// HTTPRoute defines HTTP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "HTTPRoute",
    plural = "httproutes",
    status = "HTTPRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines the set of hostnames
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,

    /// Controller-resolved effective hostnames (intersection of route hostnames and listener hostnames).
    /// Set by the controller ProcessorHandler; the data plane uses these directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_hostnames: Option<Vec<String>>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Controller-identified invalid rule indices.
    /// Gateway skips these rules during route compilation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_rule_indices: Option<Vec<usize>>,

    /// Rules defines the HTTP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HTTPRouteRule>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRule {
    /// Name identifies this rule for observability and status correlation
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<HTTPRouteMatch>>,

    /// Filters define the plugins that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<HTTPRouteFilter>>,

    /// BackendRefs defines the lb(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,

    /// Timeouts defines timeouts for requests matching this rule
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<HTTPRouteTimeouts>,

    /// Retry defines the retry policy for requests matching this rule
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<HTTPRouteRetry>,

    /// SessionPersistence defines the session persistence configuration
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<HTTPBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    /// This is computed from plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Pre-parsed route-level timeout configurations (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_timeouts: Option<ParsedRouteTimeouts>,

    /// Pre-parsed max retries from HTTPRoute annotation (not serialized)
    /// Parsed from annotation "edgion.io/max-retries" during route loading
    /// None = use global default, Some(n) = use annotation value
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_max_retries: Option<u32>,

    /// Pre-parsed retry configuration from HTTPRouteRule.retry (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_retry: Option<ParsedRouteRetry>,
}

impl Clone for HTTPRouteRule {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            matches: self.matches.clone(),
            filters: self.filters.clone(),
            backend_refs: self.backend_refs.clone(),
            timeouts: self.timeouts.clone(),
            retry: self.retry.clone(),
            session_persistence: self.session_persistence.clone(),
            // Create a new uninitialized selector for cloned instance
            // The selector will be initialized lazily when needed
            backend_finder: BackendSelector::new(),
            plugin_runtime: self.plugin_runtime.clone(),
            parsed_timeouts: self.parsed_timeouts.clone(),
            parsed_max_retries: self.parsed_max_retries,
            parsed_retry: self.parsed_retry.clone(),
        }
    }
}

impl fmt::Debug for HTTPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HTTPRouteRule")
            .field("name", &self.name)
            .field("matches", &self.matches)
            .field("plugins", &self.filters)
            .field("backend_refs", &self.backend_refs)
            .field("timeouts", &self.timeouts)
            .field("retry", &self.retry)
            .field("session_persistence", &self.session_persistence)
            .field("lb", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteMatch {
    /// Path specifies a HTTP request path matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathMatch>,

    /// Headers specifies HTTP request header match_engine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<HTTPHeaderMatch>>,

    /// QueryParams specifies HTTP query parameter match_engine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_params: Option<Vec<HTTPQueryParamMatch>>,

    /// Method specifies HTTP method matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathMatch {
    /// Type specifies how to match_engine against the path Value
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Value of the HTTP path to match_engine against
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeaderMatch {
    /// Type specifies how to match_engine the header
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the HTTP Header
    pub name: String,

    /// Value is the value of HTTP Header
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPQueryParamMatch {
    /// Type specifies how to match_engine the query parameter
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the query parameter
    pub name: String,

    /// Value is the value of the query parameter
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPBackendRef {
    /// Name is the name of the lb Service
    pub name: String,

    /// Namespace is the namespace of the lb Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Weight specifies the proportion of requests forwarded to the lb
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Filters defined at this level should be executed if and only if the request is being forwarded to this backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<HTTPRouteFilter>>,

    /// Parsed extension info (runtime only, not serialized)
    /// This is computed from plugins[].extensionRef at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub extension_info: BackendExtensionInfo,

    /// Filter runtime (runtime only, not serialized)
    /// This is computed from plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// BackendTLSPolicy for this backend (runtime only, not serialized)
    /// Queried and populated during select_backend
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_tls_policy: Option<Arc<crate::types::resources::BackendTLSPolicy>>,

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    /// When present, Gateway will reject requests to this backend
    /// and log the denial details to access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_denied: Option<RefDenied>,
}

/// HTTPRouteFilter defines processing steps that must be completed during the request/response lifecycle
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteFilter {
    /// Type identifies the type of filter to apply
    #[serde(rename = "type")]
    pub filter_type: HTTPRouteFilterType,

    /// RequestHeaderModifier defines a schema for a filter that modifies request headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header_modifier: Option<HTTPHeaderFilter>,

    /// ResponseHeaderModifier defines a schema for a filter that modifies response headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_header_modifier: Option<HTTPHeaderFilter>,

    /// RequestRedirect defines a schema for a filter that responds to the request with an HTTP redirection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_redirect: Option<HTTPRequestRedirectFilter>,

    /// URLRewrite defines a schema for a filter that modifies a request during forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_rewrite: Option<HTTPURLRewriteFilter>,

    /// RequestMirror defines a schema for a filter that mirrors requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_mirror: Option<HTTPRequestMirrorFilter>,

    /// CORS defines a Gateway API native CORS filter (GEP-1767)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cors: Option<HTTPCORSFilter>,

    /// ExternalAuth defines a Gateway API native external authentication filter (GEP-1494)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_auth: Option<crate::types::resources::edgion_plugins::ForwardAuthConfig>,

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<LocalObjectReference>,

    /// Max depth for nested ExtensionRef plugin resolution (default 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref_max_depth: Option<usize>,
}

/// HTTPRouteFilterType identifies a type of HTTPRoute filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum HTTPRouteFilterType {
    /// RequestHeaderModifier can be used to add or remove an HTTP header from an HTTP request
    RequestHeaderModifier,
    /// ResponseHeaderModifier can be used to add or remove an HTTP header from an HTTP response
    ResponseHeaderModifier,
    /// RequestRedirect can be used to redirect a request to another location
    RequestRedirect,
    /// URLRewrite can be used to modify a request during forwarding
    URLRewrite,
    /// RequestMirror can be used to mirror HTTP requests to a different backend
    RequestMirror,
    /// ExtensionRef is used for configuring custom HTTP plugins
    ExtensionRef,
    /// CORS defines a Gateway API native CORS filter (GEP-1767)
    CORS,
    /// ExternalAuth defines a Gateway API native external authentication filter (GEP-1494)
    /// Maps internally to ForwardAuth plugin
    ExternalAuth,
}

/// HTTPHeaderFilter defines a filter that modifies the headers of an HTTP request or response
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeaderFilter {
    /// Set overwrites the request with the given header (name, value) before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HTTPHeader>>,

    /// Add adds the given header(s) (name, value) to the request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HTTPHeader>>,

    /// Remove the given header(s) from the HTTP request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

/// HTTPHeader represents an HTTP Header name and value as defined by RFC 7230
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct HTTPHeader {
    /// Name is the name of the HTTP Header
    pub name: String,
    /// Value is the value of HTTP Header
    pub value: String,
}

/// HTTPRequestRedirectFilter defines a filter that redirects a request
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRequestRedirectFilter {
    /// Scheme is the scheme to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    /// Hostname is the hostname to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Path defines parameters used to modify the path of the incoming request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathModifier>,

    /// Port is the port to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// StatusCode is the HTTP status code to be used in response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i32>,
}

/// HTTPURLRewriteFilter defines a filter that modifies a request during forwarding
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPURLRewriteFilter {
    /// Hostname is the value to be used to replace the Host header value during forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Path defines a path rewrite
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathModifier>,
}

/// HTTPPathModifier defines configuration for path modifiers
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathModifier {
    /// Type defines the type of path modifier
    #[serde(rename = "type")]
    pub modifier_type: HTTPPathModifierType,

    /// ReplaceFullPath specifies the value with which to replace the full path of a request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_full_path: Option<String>,

    /// ReplacePrefixMatch specifies the value with which to replace the prefix match of a request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_prefix_match: Option<String>,
}

/// HTTPPathModifierType defines the type of path redirect or rewrite
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum HTTPPathModifierType {
    /// ReplaceFullPath replaces the full path
    ReplaceFullPath,
    /// ReplacePrefixMatch replaces any prefix path
    ReplacePrefixMatch,
}

/// HTTPRequestMirrorFilter defines configuration for the RequestMirror filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRequestMirrorFilter {
    /// BackendRef references a resource where mirrored requests are sent
    pub backend_ref: BackendObjectReference,

    /// Fraction represents the fraction of requests that should be mirrored to BackendRef
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<Fraction>,

    /// Percentage of requests to mirror (0-100). Gateway API v1.5 feature.
    /// Accepts both "percent" (Gateway API spec field name) and "percentage" (internal alias).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "percent")]
    pub percentage: Option<i32>,

    /// Connection timeout for mirror backend in milliseconds.
    #[serde(default = "default_mirror_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    /// Write/send timeout for mirror backend in milliseconds.
    #[serde(default = "default_mirror_write_timeout_ms")]
    pub write_timeout_ms: u64,

    /// Maximum number of body chunks buffered for mirror when mirror is slower.
    #[serde(default = "default_mirror_max_buffered_chunks")]
    pub max_buffered_chunks: usize,

    /// Whether mirror should emit a dedicated access log line.
    #[serde(default = "default_mirror_log_enabled")]
    pub mirror_log: bool,

    /// Maximum number of concurrent mirror tasks per gateway process.
    #[serde(default = "default_mirror_max_concurrent")]
    pub max_concurrent: usize,

    /// Maximum milliseconds to wait for channel space when the mirror body channel is full,
    /// before abandoning the mirror entirely.
    ///
    /// - `0` (default): immediately abandon — zero impact on main request latency.
    /// - `> 0`: brief back-pressure window — the request body filter will await channel
    ///   drain for at most this many milliseconds before giving up. Recommended range:
    ///   100–500ms. Beyond this the mirror is abandoned and main request continues normally.
    ///
    /// This is a trade-off: a small value gives the mirror backend a chance to catch up
    /// without immediately discarding valuable traffic.
    #[serde(default)]
    pub channel_full_timeout_ms: u64,
}

fn default_mirror_connect_timeout_ms() -> u64 {
    1000
}

fn default_mirror_write_timeout_ms() -> u64 {
    1000
}

fn default_mirror_max_buffered_chunks() -> usize {
    5
}

fn default_mirror_log_enabled() -> bool {
    true
}

fn default_mirror_max_concurrent() -> usize {
    1024
}

/// HTTPCORSFilter defines a Gateway API native CORS filter (GEP-1767).
///
/// Per Gateway API v1.5, origin values are validated to reject malformed entries
/// containing path components, query strings, fragments, or trailing slashes in
/// the host portion.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPCORSFilter {
    /// AllowOrigins defines the origins that are permitted to make CORS requests.
    /// Each value must be a valid origin: `scheme://host[:port]` or `*`.
    /// Wildcard `*` is allowed only in the subdomain position (e.g. `https://*.example.com`).
    /// Origins with paths, query strings, fragments, or trailing slashes are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_origins: Option<Vec<String>>,

    /// AllowCredentials indicates whether the request can include user credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_credentials: Option<bool>,

    /// AllowMethods defines the HTTP methods that are permitted in CORS requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_methods: Option<Vec<String>>,

    /// AllowHeaders defines the HTTP headers that are permitted in CORS requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_headers: Option<Vec<String>>,

    /// ExposeHeaders defines the HTTP headers that browsers are allowed to access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose_headers: Option<Vec<String>>,

    /// MaxAge defines the maximum time (in seconds) a preflight response can be cached.
    #[serde(default)]
    pub max_age: u32,
}

impl HTTPCORSFilter {
    /// Validate the CORS filter configuration per Gateway API v1.5 tightened CEL rules.
    ///
    /// Rejects origins that contain:
    /// - Path components (e.g. `https://example.com/path`)
    /// - Query strings (e.g. `https://example.com?q=1`)
    /// - Fragments (e.g. `https://example.com#section`)
    /// - Trailing slashes (e.g. `https://example.com/`)
    /// - Missing scheme (e.g. `example.com`)
    /// - Empty host (e.g. `https://`)
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if let Some(ref origins) = self.allow_origins {
            for (i, origin) in origins.iter().enumerate() {
                if origin == "*" {
                    continue;
                }
                if let Err(e) = Self::validate_origin(origin) {
                    errors.push(format!("allowOrigins[{}] \"{}\": {}", i, origin, e));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate a single origin value per Gateway API v1.5 rules.
    /// Valid format: `scheme://host[:port]` where host may use `*` only as
    /// a subdomain wildcard prefix (e.g. `*.example.com`).
    fn validate_origin(origin: &str) -> Result<(), String> {
        // Must contain ://
        let Some(scheme_end) = origin.find("://") else {
            return Err("missing scheme (expected scheme://host)".to_string());
        };

        let scheme = &origin[..scheme_end];
        if scheme.is_empty() {
            return Err("empty scheme".to_string());
        }

        // Scheme must be alphanumeric (+ hyphen)
        if !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        {
            return Err("invalid scheme characters".to_string());
        }

        let after_scheme = &origin[scheme_end + 3..];

        if after_scheme.is_empty() {
            return Err("empty host".to_string());
        }

        // Reject path components, query strings, fragments
        if after_scheme.contains('/') {
            return Err("origin must not contain a path component".to_string());
        }
        if after_scheme.contains('?') {
            return Err("origin must not contain a query string".to_string());
        }
        if after_scheme.contains('#') {
            return Err("origin must not contain a fragment".to_string());
        }

        // Extract host (strip port if present)
        let host = if let Some(colon_pos) = after_scheme.rfind(':') {
            let port_candidate = &after_scheme[colon_pos + 1..];
            if port_candidate.chars().all(|c| c.is_ascii_digit()) && !port_candidate.is_empty() {
                &after_scheme[..colon_pos]
            } else {
                after_scheme
            }
        } else {
            after_scheme
        };

        if host.is_empty() {
            return Err("empty host".to_string());
        }

        // Wildcard is only valid as `*.suffix` (subdomain wildcard)
        if host.contains('*') {
            if !host.starts_with("*.") {
                return Err("wildcard '*' is only valid as a subdomain prefix (e.g. *.example.com)".to_string());
            }
            let suffix = &host[2..];
            if suffix.is_empty() || suffix.contains('*') {
                return Err("invalid wildcard pattern in host".to_string());
            }
        }

        Ok(())
    }
}

/// BackendObjectReference identifies an API object within the namespace of the referrer
/// Used specifically for mirror backend references (without weight/plugins)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendObjectReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,

    /// Kind is kind of the referent (defaults to "Service")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number to use for this resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

/// Fraction represents a fraction of requests to mirror
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fraction {
    /// Numerator specifies the numerator of the fraction
    pub numerator: i32,

    /// Denominator specifies the denominator of the fraction (defaults to 100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<i32>,
}

/// LocalObjectReference identifies an API object within the namespace of the referrer
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalObjectReference {
    /// Group is the group of the referent (defaults to empty string for core API group)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,

    /// Kind is kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,
}

/// HTTPRouteTimeouts defines timeouts that can be configured for an HTTPRoute
/// Support: Extended
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteTimeouts {
    /// Request specifies the maximum duration for a gateway to respond to an HTTP request
    /// This timeout covers the entire request-response transaction including all retries.
    /// Gateway API v1.4 standard field
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,

    /// BackendRequest specifies a timeout for an individual request from the gateway
    /// to a backend. This covers the time from when the request first starts being
    /// sent from the gateway to when the full response has been received from the backend.
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_request: Option<String>,
}

/// HTTPRouteRetry defines retry policy for an HTTPRoute
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRetry {
    /// Attempts specifies the maximum number of times an individual request
    /// from the gateway to a backend should be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<i32>,

    /// Backoff specifies the minimum duration a Gateway should wait between
    /// retry attempts and is represented in Gateway API Duration formatting.
    /// Format: Duration (e.g., "10ms", "1s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<String>,

    /// Codes specifies the HTTP response status codes that a Gateway should
    /// retry for. When not specified, retries will be triggered only
    /// on connection errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<Vec<i32>>,
}

/// SessionPersistence defines the desired state of SessionPersistence
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPersistence {
    /// SessionName defines the name of the persistent session token
    /// which may be reflected in the cookie or the header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,

    /// AbsoluteTimeout defines the absolute timeout of the persistent
    /// session, regardless of activity.
    /// Format: Duration (e.g., "1h", "24h")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_timeout: Option<String>,

    /// IdleTimeout defines the idle timeout of the persistent session.
    /// Once the session has been idle for more than the specified
    /// IdleTimeout, the session becomes invalid.
    /// Format: Duration (e.g., "30m", "1h")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<String>,

    /// Type defines the type of session persistence such as through
    /// the use a header or cookie.
    /// Defaults to cookie based session persistence.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub persistence_type: Option<SessionPersistenceType>,

    /// CookieConfig provides configuration settings that are specific
    /// to cookie-based session persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_config: Option<CookieConfig>,

    /// Edgion extension: strict mode.
    /// When true, if the session-bound backend is unavailable, return error (no fallback).
    /// When false (default), fallback to normal load balancing and update the session token.
    /// Similar to Envoy's `strict` field on stateful_session filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// SessionPersistenceType specifies the type of session persistence
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum SessionPersistenceType {
    /// Cookie uses a cookie for session persistence
    Cookie,
    /// Header uses a header for session persistence
    Header,
}

/// CookieConfig defines configuration for cookie-based session persistence
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CookieConfig {
    /// LifetimeType specifies whether the cookie has a permanent or
    /// session-based lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime_type: Option<CookieLifetimeType>,
}

/// CookieLifetimeType specifies the type of cookie lifetime
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum CookieLifetimeType {
    /// Permanent cookies persist until deleted by the client
    Permanent,
    /// Session cookies are deleted when the client session ends
    Session,
}

// ============================================
// Pre-parsed Route Retry
// ============================================

/// Pre-parsed retry configuration from HTTPRouteRule.retry
/// Stored in HTTPRouteRule for zero-cost runtime access
#[derive(Debug, Clone)]
pub struct ParsedRouteRetry {
    /// Max retry attempts (from retry.attempts)
    pub attempts: Option<u32>,
    /// Base backoff duration (from retry.backoff, parsed from string)
    pub backoff: Option<std::time::Duration>,
    /// HTTP status codes that trigger retry (from retry.codes, validated to 100-599)
    pub codes: Option<Vec<u16>>,
}

// ============================================
// Pre-parsed Route Timeouts
// ============================================

use std::time::Duration;

/// Three-state timeout setting per Gateway API spec.
/// - `Unset`: field not specified → fall back to global default
/// - `Disabled`: explicitly set to `"0s"` → no timeout (infinite)
/// - `Duration`: explicitly set to a positive value
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteTimeout {
    Unset,
    Disabled,
    Duration(Duration),
}

impl RouteTimeout {
    /// Returns the duration if explicitly set to a positive value, otherwise None.
    pub fn as_duration(&self) -> Option<Duration> {
        match self {
            RouteTimeout::Duration(d) => Some(*d),
            _ => None,
        }
    }

    /// Returns true if explicitly disabled via "0s".
    pub fn is_disabled(&self) -> bool {
        matches!(self, RouteTimeout::Disabled)
    }

    pub fn parse(s: &str, field_name: &str) -> Self {
        use crate::core::common::utils::parse_duration;
        match parse_duration(s) {
            Ok(d) if d.is_zero() => RouteTimeout::Disabled,
            Ok(d) => RouteTimeout::Duration(d),
            Err(e) => {
                tracing::warn!("Invalid {} timeout '{}': {}", field_name, s, e);
                RouteTimeout::Unset
            }
        }
    }
}

#[derive(Clone)]
pub struct ParsedRouteTimeouts {
    /// Parsed request timeout (end-to-end including retries)
    /// Gateway API v1.4 standard field
    pub request_timeout: RouteTimeout,

    /// Parsed backend request timeout
    /// Gateway API v1.4 standard field
    pub backend_request_timeout: RouteTimeout,
}

impl ParsedRouteTimeouts {
    /// Parse route timeouts from HTTPRouteTimeouts
    /// Returns None if no timeouts are configured
    pub fn from_config(config: &HTTPRouteTimeouts) -> Option<Self> {
        let request_timeout = config
            .request
            .as_ref()
            .map(|s| RouteTimeout::parse(s, "request"))
            .unwrap_or(RouteTimeout::Unset);

        let backend_request_timeout = config
            .backend_request
            .as_ref()
            .map(|s| RouteTimeout::parse(s, "backend_request"))
            .unwrap_or(RouteTimeout::Unset);

        // Clamp: backendRequest must be <= request (Gateway API spec)
        let backend_request_timeout = match (backend_request_timeout.as_duration(), request_timeout.as_duration()) {
            (Some(br), Some(req)) if br > req => {
                tracing::warn!(
                    "backendRequest timeout ({:?}) exceeds request timeout ({:?}), clamping to request timeout",
                    br,
                    req
                );
                RouteTimeout::Duration(req)
            }
            _ => backend_request_timeout,
        };

        // Only create if at least one timeout is explicitly configured
        if matches!(request_timeout, RouteTimeout::Unset) && matches!(backend_request_timeout, RouteTimeout::Unset) {
            None
        } else {
            Some(Self {
                request_timeout,
                backend_request_timeout,
            })
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteStatus {
    /// Parents describe the status of the route with respect to each parent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<RouteParentStatus>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteParentStatus {
    /// ParentRef references the parent that this status corresponds to.
    pub parent_ref: ParentReference,

    /// ControllerName is the name of the controller that manages this Route.
    pub controller_name: String,

    /// Conditions describe the current conditions of this route.
    pub conditions: Vec<Condition>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_backend_request_clamped_to_request() {
        let config = HTTPRouteTimeouts {
            request: Some("10s".to_string()),
            backend_request: Some("30s".to_string()),
        };
        let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
        assert_eq!(parsed.request_timeout, RouteTimeout::Duration(Duration::from_secs(10)));
        assert_eq!(
            parsed.backend_request_timeout,
            RouteTimeout::Duration(Duration::from_secs(10))
        );
    }

    #[test]
    fn test_backend_request_within_limit() {
        let config = HTTPRouteTimeouts {
            request: Some("30s".to_string()),
            backend_request: Some("10s".to_string()),
        };
        let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
        assert_eq!(parsed.request_timeout, RouteTimeout::Duration(Duration::from_secs(30)));
        assert_eq!(
            parsed.backend_request_timeout,
            RouteTimeout::Duration(Duration::from_secs(10))
        );
    }

    #[test]
    fn test_backend_request_only_no_clamp() {
        let config = HTTPRouteTimeouts {
            request: None,
            backend_request: Some("10s".to_string()),
        };
        let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
        assert_eq!(parsed.request_timeout, RouteTimeout::Unset);
        assert_eq!(
            parsed.backend_request_timeout,
            RouteTimeout::Duration(Duration::from_secs(10))
        );
    }

    #[test]
    fn test_zero_timeout_means_disable() {
        // Gateway API spec: "0s" means "no timeout" (disable), not zero-second timeout
        let config = HTTPRouteTimeouts {
            request: Some("0s".to_string()),
            backend_request: Some("0s".to_string()),
        };
        let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
        assert_eq!(parsed.request_timeout, RouteTimeout::Disabled);
        assert_eq!(parsed.backend_request_timeout, RouteTimeout::Disabled);
    }

    #[test]
    fn test_zero_request_with_nonzero_backend() {
        let config = HTTPRouteTimeouts {
            request: Some("0s".to_string()),
            backend_request: Some("10s".to_string()),
        };
        let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
        assert_eq!(parsed.request_timeout, RouteTimeout::Disabled);
        assert_eq!(
            parsed.backend_request_timeout,
            RouteTimeout::Duration(Duration::from_secs(10))
        );
    }

    #[test]
    fn test_no_timeouts_returns_none() {
        let config = HTTPRouteTimeouts {
            request: None,
            backend_request: None,
        };
        assert!(ParsedRouteTimeouts::from_config(&config).is_none());
    }
}
