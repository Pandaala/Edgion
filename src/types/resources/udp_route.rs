//! UDPRoute resource definition
//!
//! UDPRoute defines UDP rules for mapping requests to backends

use std::fmt;
use std::sync::Arc;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::lb::BackendSelector;
use crate::core::plugins::{PluginRuntime, StreamPluginRuntime};
use super::http_route_preparse::BackendExtensionInfo;

/// API group for UDPRoute
pub const UDP_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for UDPRoute
pub const UDP_ROUTE_KIND: &str = "UDPRoute";

/// UDPRoute defines UDP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "UDPRoute",
    plural = "udproutes",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct UDPRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Rules defines the UDP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<UDPRouteRule>>,
}

/// ParentReference identifies a parent resource (usually Gateway)
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

/// UDPRouteRule defines UDP routing rules
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UDPRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<UDPBackendRef>>,

    /// Filters define the plugins that are applied to UDP connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<UDPRouteFilter>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<UDPBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Stream plugin runtime (runtime only, not serialized)
    /// This is computed from filters at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub stream_plugin_runtime: Arc<StreamPluginRuntime>,
}

impl Clone for UDPRouteRule {
    fn clone(&self) -> Self {
        Self {
            backend_refs: self.backend_refs.clone(),
            filters: self.filters.clone(),
            backend_finder: BackendSelector::new(),
            plugin_runtime: self.plugin_runtime.clone(),
            stream_plugin_runtime: self.stream_plugin_runtime.clone(),
        }
    }
}

impl fmt::Debug for UDPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UDPRouteRule")
            .field("backend_refs", &self.backend_refs)
            .field("filters", &self.filters)
            .field("backend_finder", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .field("stream_plugin_runtime", &self.stream_plugin_runtime)
            .finish()
    }
}

/// UDPBackendRef defines a backend for UDP traffic
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UDPBackendRef {
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

    /// Parsed extension info (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub extension_info: BackendExtensionInfo,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,
}

/// UDPRouteFilter defines processing steps for UDP connections
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UDPRouteFilter {
    /// Type identifies the type of filter to apply
    #[serde(rename = "type")]
    pub filter_type: UDPRouteFilterType,

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<UDPLocalObjectReference>,
}

/// UDPRouteFilterType identifies a type of UDPRoute filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum UDPRouteFilterType {
    /// ExtensionRef is used for configuring custom UDP plugins
    ExtensionRef,
}

/// UDPLocalObjectReference identifies an API object within the namespace of the referrer
pub type UDPLocalObjectReference = crate::types::resources::http_route::LocalObjectReference;

