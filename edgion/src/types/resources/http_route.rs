//! HTTPRoute resource definition
//!
//! HTTPRoute defines HTTP rules for mapping requests to backends

use std::fmt;
use arc_swap::ArcSwap;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::backend::WeightedRoundRobin;

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
    pub filters: Option<Vec<serde_json::Value>>,

    /// BackendRefs defines the backend(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,

    /// Weighted round-robin selector for backend selection (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub lb: ArcSwap<Option<WeightedRoundRobin<HTTPBackendRef>>>,
}

impl Clone for HTTPRouteRule {
    fn clone(&self) -> Self {
        Self {
            matches: self.matches.clone(),
            filters: self.filters.clone(),
            backend_refs: self.backend_refs.clone(),
            // Create a new empty ArcSwap for cloned instance
            // The selector will be initialized lazily when needed
            lb: ArcSwap::from_pointee(None),
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
}

