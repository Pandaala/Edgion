//! ResourceMeta trait implementations for all resource types
//!
//! This file consolidates all ResourceMeta implementations using the impl_resource_meta! macro.
//! For resources with custom pre_parse logic, the actual logic is defined in the resource's
//! own impl block (e.g., HTTPRoute::preparse(), LinkSys::validate_config()).

use crate::impl_resource_meta;

// =============================================================================
// K8s native resources
// =============================================================================

use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

impl_resource_meta!(Service, Service, "Service");
impl_resource_meta!(Secret, Secret, "Secret");
impl_resource_meta!(ConfigMap, ConfigMap, "ConfigMap");
impl_resource_meta!(EndpointSlice, EndpointSlice, "EndpointSlice");

// =============================================================================
// Gateway API resources
// =============================================================================

use crate::resources::{
    BackendTLSPolicy, GRPCRoute, Gateway, GatewayClass, HTTPRoute, ReferenceGrant, ReferenceGrantBeta, TCPRoute,
    TLSRoute, TLSRouteAlpha3, UDPRoute,
};

// Cluster-scoped
impl_resource_meta!(GatewayClass, GatewayClass, "GatewayClass", cluster_scoped);

// Namespaced
impl_resource_meta!(Gateway, Gateway, "Gateway");
impl_resource_meta!(ReferenceGrant, ReferenceGrant, "ReferenceGrant");
impl_resource_meta!(ReferenceGrantBeta, ReferenceGrant, "ReferenceGrant");
impl_resource_meta!(BackendTLSPolicy, BackendTLSPolicy, "BackendTLSPolicy");

// Routes (with pre_parse)
impl_resource_meta!(HTTPRoute, HTTPRoute, "HTTPRoute", |self| {
    self.preparse();
    self.parse_timeouts();
    self.parse_retry();
});

impl_resource_meta!(GRPCRoute, GRPCRoute, "GRPCRoute", |self| {
    self.preparse();
    self.parse_timeouts();
    self.parse_retry();
});

impl_resource_meta!(TCPRoute, TCPRoute, "TCPRoute");
impl_resource_meta!(UDPRoute, UDPRoute, "UDPRoute");
impl_resource_meta!(TLSRoute, TLSRoute, "TLSRoute");
impl_resource_meta!(TLSRouteAlpha3, TLSRoute, "TLSRoute");

// =============================================================================
// Edgion custom resources
// =============================================================================

use crate::resources::{
    EdgionAcme, EdgionBackend, EdgionBackendTrafficPolicy, EdgionConfigData, EdgionGatewayConfig, EdgionPlugins,
    EdgionStreamPlugins, EdgionTls, LinkSys,
};

// Cluster-scoped
impl_resource_meta!(
    EdgionGatewayConfig,
    EdgionGatewayConfig,
    "EdgionGatewayConfig",
    cluster_scoped
);

// Namespaced
impl_resource_meta!(EdgionTls, EdgionTls, "EdgionTls", |self| {
    // Resolve operator log labels once at ingest so the TLS handshake callback
    // (ssl.log) reads a cached Arc instead of re-scanning metadata per handshake.
    self.spec.resolved_log_labels = crate::resources::common::resolve_log_labels(&self.metadata);
});

// PluginRuntime is built gateway-side via `compile_plugins`; no pre_parse needed.
impl_resource_meta!(EdgionPlugins, EdgionPlugins, "EdgionPlugins");

impl_resource_meta!(EdgionBackend, EdgionBackend, "EdgionBackend", |self| {
    // The same stateless validation is used by Controller admission. The data
    // plane cannot reject a conf_sync item, so retain the existing resource and
    // expose any invalid transport state as a warning.
    if let Err(errors) = self.spec.validate() {
        let key = self.key_name();
        tracing::warn!(edgion_backend = %key, error_count = errors.len(), "EdgionBackend validation error");
    }
});

// Stream runtime views are built gateway-side by the canonical scoped publisher; no pre_parse needed.
impl_resource_meta!(EdgionStreamPlugins, EdgionStreamPlugins, "EdgionStreamPlugins");

impl_resource_meta!(LinkSys, LinkSys, "LinkSys", |self| {
    // Data-plane side: log validation errors but don't fail the pre_parse step.
    // The Controller surfaces the same errors via the Accepted condition
    // (see LinkSysHandler::preparse + update_status).
    let errors = self.validate_config();
    if !errors.is_empty() {
        let key = self.key_name_str();
        for err in &errors {
            tracing::warn!(link_sys = %key, error = %err, "LinkSys validation error");
        }
    }
});

impl_resource_meta!(EdgionConfigData, EdgionConfigData, "EdgionConfigData", |self| {
    // Data-plane side: log validation problems but don't fail the pre_parse step.
    // The Controller records the same errors in the Accepted status condition
    // (see EdgionConfigDataHandler::preparse + update_status).
    let errors = self.validate_pre_parse();
    if !errors.is_empty() {
        let key = self.key_name_str();
        for err in &errors {
            tracing::warn!(edgion_config_data = %key, error = %err, "EdgionConfigData validation error");
        }
    }
});

// ACME
impl_resource_meta!(EdgionAcme, EdgionAcme, "EdgionAcme");

// Backend Traffic Policy
impl_resource_meta!(
    EdgionBackendTrafficPolicy,
    EdgionBackendTrafficPolicy,
    "EdgionBackendTrafficPolicy"
);
