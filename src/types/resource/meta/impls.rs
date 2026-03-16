//! ResourceMeta trait implementations for all resource types
//!
//! This file consolidates all ResourceMeta implementations using the impl_resource_meta! macro.
//! For resources with custom pre_parse logic, the actual logic is defined in the resource's
//! own impl block (e.g., HTTPRoute::preparse(), LinkSys::validate_config()).

use crate::impl_resource_meta;

// =============================================================================
// K8s native resources
// =============================================================================

use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

impl_resource_meta!(Service, Service, "Service");
impl_resource_meta!(Secret, Secret, "Secret");
impl_resource_meta!(Endpoints, Endpoint, "Endpoints");
impl_resource_meta!(EndpointSlice, EndpointSlice, "EndpointSlice");

// =============================================================================
// Gateway API resources
// =============================================================================

use crate::types::resources::{
    BackendTLSPolicy, GRPCRoute, Gateway, GatewayClass, HTTPRoute, ReferenceGrant, TCPRoute, TLSRoute, UDPRoute,
};

// Cluster-scoped
impl_resource_meta!(GatewayClass, GatewayClass, "GatewayClass", cluster_scoped);

// Namespaced
impl_resource_meta!(Gateway, Gateway, "Gateway");
impl_resource_meta!(ReferenceGrant, ReferenceGrant, "ReferenceGrant");
impl_resource_meta!(BackendTLSPolicy, BackendTLSPolicy, "BackendTLSPolicy");

// Routes (with pre_parse)
impl_resource_meta!(HTTPRoute, HTTPRoute, "HTTPRoute", |self| {
    self.preparse();
    self.parse_timeouts();
    self.parse_annotations();
});

impl_resource_meta!(GRPCRoute, GRPCRoute, "GRPCRoute", |self| {
    self.preparse();
    self.parse_timeouts();
});

impl_resource_meta!(TCPRoute, TCPRoute, "TCPRoute");
impl_resource_meta!(UDPRoute, UDPRoute, "UDPRoute");
impl_resource_meta!(TLSRoute, TLSRoute, "TLSRoute");

// =============================================================================
// Edgion custom resources
// =============================================================================

use crate::types::resources::{
    EdgionAcme, EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionTls, LinkSys, PluginMetaData,
};

// Cluster-scoped
impl_resource_meta!(
    EdgionGatewayConfig,
    EdgionGatewayConfig,
    "EdgionGatewayConfig",
    cluster_scoped
);

// Namespaced
impl_resource_meta!(EdgionTls, EdgionTls, "EdgionTls");

// With pre_parse
impl_resource_meta!(EdgionPlugins, EdgionPlugins, "EdgionPlugins", |self| {
    self.preparse();
});

impl_resource_meta!(
    EdgionStreamPlugins,
    EdgionStreamPlugins,
    "EdgionStreamPlugins",
    |self| {
        self.init_stream_plugin_runtime();
        self.init_tls_route_plugin_runtime();
    }
);

impl_resource_meta!(LinkSys, LinkSys, "LinkSys", |self| {
    self.validate_config();
});

impl_resource_meta!(PluginMetaData, PluginMetaData, "PluginMetaData", |self| {
    self.validate_pre_parse();
});

// ACME
impl_resource_meta!(EdgionAcme, EdgionAcme, "EdgionAcme");
