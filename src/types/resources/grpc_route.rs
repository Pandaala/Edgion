//! GRPCRoute resource definition
//!
//! GRPCRoute defines gRPC rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use super::common::ParentReference;
use super::http_route::{
    BackendObjectReference, Fraction, HTTPHeader, LocalObjectReference, ParsedRouteTimeouts as HttpParsedRouteTimeouts,
    SessionPersistence,
};
use super::http_route_preparse::BackendExtensionInfo;
use crate::core::lb::BackendSelector;
use crate::core::plugins::PluginRuntime;

/// API group for GRPCRoute
pub const GRPC_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for GRPCRoute
pub const GRPC_ROUTE_KIND: &str = "GRPCRoute";

/// GRPCRoute defines gRPC rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GRPCRoute",
    plural = "grpcroutes",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines a set of hostnames to match against the gRPC Host header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,

    /// Rules defines the gRPC routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<GRPCRouteRule>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteRule {
    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GRPCRouteMatch>>,

    /// Filters define the plugins that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<GRPCRouteFilter>>,

    /// BackendRefs defines the backend(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<GRPCBackendRef>>,

    /// Timeouts defines timeouts for requests matching this rule
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<GRPCRouteTimeouts>,

    /// Retry defines the retry policy for requests matching this rule
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<GRPCRouteRetry>,

    /// SessionPersistence defines the session persistence configuration
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<GRPCBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    /// This is computed from plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Pre-parsed route-level timeout configurations (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_timeouts: Option<HttpParsedRouteTimeouts>,
}

impl Clone for GRPCRouteRule {
    fn clone(&self) -> Self {
        Self {
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
        }
    }
}

impl fmt::Debug for GRPCRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GRPCRouteRule")
            .field("matches", &self.matches)
            .field("plugins", &self.filters)
            .field("backend_refs", &self.backend_refs)
            .field("timeouts", &self.timeouts)
            .field("retry", &self.retry)
            .field("session_persistence", &self.session_persistence)
            .field("backend_finder", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteMatch {
    /// Method specifies a gRPC request service/method matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<GRPCMethodMatch>,

    /// Headers specifies gRPC request header matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<GRPCHeaderMatch>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCMethodMatch {
    /// Type specifies how to match against the service and method
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<GRPCMethodMatchType>,

    /// Service is the name of the gRPC service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Method is the name of the gRPC method
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// GRPCMethodMatchType specifies how to match the method
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum GRPCMethodMatchType {
    /// Exact match
    Exact,
    /// RegularExpression match
    RegularExpression,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCHeaderMatch {
    /// Type specifies how to match the header
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the gRPC Header
    pub name: String,

    /// Value is the value of gRPC Header
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCBackendRef {
    /// Name is the name of the backend Service
    pub name: String,

    /// Namespace is the namespace of the backend Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Weight specifies the proportion of requests forwarded to the backend
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
    pub filters: Option<Vec<GRPCRouteFilter>>,

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
}

/// GRPCRouteFilter defines processing steps that must be completed during the request/response lifecycle
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteFilter {
    /// Type identifies the type of filter to apply
    #[serde(rename = "type")]
    pub filter_type: GRPCRouteFilterType,

    /// RequestHeaderModifier defines a schema for a filter that modifies request headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header_modifier: Option<GRPCHeaderFilter>,

    /// ResponseHeaderModifier defines a schema for a filter that modifies response headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_header_modifier: Option<GRPCHeaderFilter>,

    /// RequestMirror defines a schema for a filter that mirrors requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_mirror: Option<GRPCRequestMirrorFilter>,

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<LocalObjectReference>,

    /// Max depth for nested ExtensionRef plugin resolution (default 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref_max_depth: Option<usize>,
}

/// GRPCRouteFilterType identifies a type of GRPCRoute filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum GRPCRouteFilterType {
    /// RequestHeaderModifier can be used to add or remove a gRPC header from a gRPC request
    RequestHeaderModifier,
    /// ResponseHeaderModifier can be used to add or remove a gRPC header from a gRPC response
    ResponseHeaderModifier,
    /// RequestMirror can be used to mirror gRPC requests to a different backend
    RequestMirror,
    /// ExtensionRef is used for configuring custom gRPC plugins
    ExtensionRef,
}

/// GRPCHeaderFilter defines a filter that modifies the headers of a gRPC request or response
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCHeaderFilter {
    /// Set overwrites the request with the given header (name, value) before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HTTPHeader>>,

    /// Add adds the given header(s) (name, value) to the request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HTTPHeader>>,

    /// Remove the given header(s) from the gRPC request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

/// GRPCRequestMirrorFilter defines configuration for the RequestMirror filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRequestMirrorFilter {
    /// BackendRef references a resource where mirrored requests are sent
    pub backend_ref: BackendObjectReference,

    /// Fraction represents the fraction of requests that should be mirrored to BackendRef
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<Fraction>,
}

/// GRPCRouteTimeouts defines timeouts that can be configured for a GRPCRoute
/// Support: Extended
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteTimeouts {
    /// Request specifies the maximum duration for a gateway to respond to a gRPC request
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

/// GRPCRouteRetry defines retry policy for a GRPCRoute
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteRetry {
    /// Attempts specifies the maximum number of times an individual request
    /// from the gateway to a backend should be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<i32>,

    /// Backoff specifies the minimum duration a Gateway should wait between
    /// retry attempts and is represented in Gateway API Duration formatting.
    /// Format: Duration (e.g., "10ms", "1s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<String>,

    /// Codes specifies the gRPC status codes that a Gateway should
    /// retry for. When not specified, retries will be triggered only
    /// on connection errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<Vec<i32>>,
}

// Note: ParsedRouteTimeouts is imported from http_route as it's shared between HTTP and gRPC routes

// ============================================
// Pre-parsing logic for GRPCRoute
// ============================================

/// Extension trait for GRPCRoute to parse hidden logic
impl GRPCRoute {
    /// Parse all extension_ref in backend_refs and populate extension_info fields
    ///
    /// This method should be called after deserializing GRPCRoute from YAML/JSON
    /// to populate the runtime-only extension_info fields.
    pub fn preparse(&mut self) {
        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        // Get namespace for ExtensionRef lookups
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");

        for rule in rules.iter_mut() {
            // Initialize rule-level plugin_runtime from rule.plugins
            if let Some(filters) = &rule.filters {
                rule.plugin_runtime = Arc::new(PluginRuntime::from_grpcroute_filters(filters, namespace));
            }

            let Some(backend_refs) = rule.backend_refs.as_mut() else {
                continue;
            };

            for backend_ref in backend_refs.iter_mut() {
                // Find ExtensionRef filter in backend_ref.plugins
                let extension_info = backend_ref
                    .filters
                    .as_ref()
                    .and_then(|filters| {
                        filters
                            .iter()
                            .find(|f| f.filter_type == GRPCRouteFilterType::ExtensionRef)
                            .and_then(|f| f.extension_ref.as_ref())
                            .map(BackendExtensionInfo::from_extension_ref)
                    })
                    .unwrap_or_default();

                backend_ref.extension_info = extension_info;

                // Initialize plugin_runtime from plugins
                if let Some(filters) = &backend_ref.filters {
                    backend_ref.plugin_runtime = Arc::new(PluginRuntime::from_grpcroute_filters(filters, namespace));
                }
            }
        }
    }

    /// Parse and pre-process timeout configurations for all rules
    ///
    /// This method is called during route loading (in pre_parse) to avoid runtime parsing overhead.
    /// It parses timeout strings into Duration objects and stores them in rule.parsed_timeouts.
    pub fn parse_timeouts(&mut self) {
        use crate::core::utils::parse_duration;

        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        for rule in rules.iter_mut() {
            // Parse timeouts for each rule
            if let Some(timeouts) = &rule.timeouts {
                // Parse GRPCRouteTimeouts manually (same structure as HTTPRouteTimeouts)
                let request_timeout = timeouts.request.as_ref().and_then(|s| parse_duration(s).ok());

                let backend_request_timeout = timeouts.backend_request.as_ref().and_then(|s| parse_duration(s).ok());

                if request_timeout.is_some() || backend_request_timeout.is_some() {
                    rule.parsed_timeouts = Some(HttpParsedRouteTimeouts {
                        request_timeout,
                        backend_request_timeout,
                    });

                    tracing::debug!("Parsed route-level timeouts for GRPCRoute rule");
                }
            }
        }
    }
}
