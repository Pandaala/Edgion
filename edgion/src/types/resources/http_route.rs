//! HTTPRoute resource definition
//!
//! HTTPRoute defines HTTP rules for mapping requests to backends

use std::fmt;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::lb::BackendSelector;

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

    /// Rules defines the HTTP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HTTPRouteRule>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParentReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// SectionName is the name of a section within the target resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,

    /// Port is the network port this Route targets
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRule {
    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<HTTPRouteMatch>>,

    /// Filters define the filters that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<HTTPRouteFilter>>,

    /// BackendRefs defines the lb(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<HTTPBackendRef>,
}

impl Clone for HTTPRouteRule {
    fn clone(&self) -> Self {
        Self {
            matches: self.matches.clone(),
            filters: self.filters.clone(),
            backend_refs: self.backend_refs.clone(),
            // Create a new uninitialized selector for cloned instance
            // The selector will be initialized lazily when needed
            backend_finder: BackendSelector::new(),
        }
    }
}

impl fmt::Debug for HTTPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HTTPRouteRule")
            .field("matches", &self.matches)
            .field("filters", &self.filters)
            .field("backend_refs", &self.backend_refs)
            .field("lb", &"<skipped>")
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

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<LocalObjectReference>,
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
    /// ExtensionRef is used for configuring custom HTTP filters
    ExtensionRef,
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
    pub backend_ref: HTTPBackendRef,
}

/// LocalObjectReference identifies an API object within the namespace of the referrer
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalObjectReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,
}

