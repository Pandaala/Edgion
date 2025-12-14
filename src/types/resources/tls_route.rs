//! TLSRoute resource definition
//!
//! TLSRoute defines TLS rules for mapping requests to backends

use std::fmt;
use std::sync::Arc;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::lb::BackendSelector;
use crate::core::filters::PluginRuntime;
use super::http_route_preparse::BackendExtensionInfo;

/// API group for TLSRoute
pub const TLS_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for TLSRoute
pub const TLS_ROUTE_KIND: &str = "TLSRoute";

/// TLSRoute defines TLS rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "TLSRoute",
    plural = "tlsroutes",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines a set of SNI names that should match against the SNI attribute
    /// of TLS ClientHello message in TLS handshake
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,

    /// Rules defines the TLS routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TLSRouteRule>>,
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

/// TLSRouteRule defines TLS routing rules
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TLSBackendRef>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<TLSBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,
}

impl Clone for TLSRouteRule {
    fn clone(&self) -> Self {
        Self {
            backend_refs: self.backend_refs.clone(),
            backend_finder: BackendSelector::new(),
            plugin_runtime: self.plugin_runtime.clone(),
        }
    }
}

impl fmt::Debug for TLSRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TLSRouteRule")
            .field("backend_refs", &self.backend_refs)
            .field("backend_finder", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .finish()
    }
}

/// TLSBackendRef defines a backend for TLS traffic
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSBackendRef {
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

