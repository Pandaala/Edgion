//! Resource change application logic for ConfigServer
//!
//! This module contains the apply_*_change methods that handle resource updates.
//! Note: Secret reference handling (SecretRefManager) is now handled by resource_processor.

use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ResourceMeta, ServerCache};
use crate::types::prelude_resources::*;

use super::config_server::ConfigServer;

/// Helper function to execute change on cache
fn execute_change_on_cache<T>(change: ResourceChange, cache: &ServerCache<T>, resource: T)
where
    T: Clone + Send + Sync + 'static + ResourceMeta + Resource,
{
    cache.apply_change(change, resource);
}

impl ConfigServer {
    // ==================== Simple Resource Changes ====================

    /// Apply HTTPRoute change
    pub fn apply_http_route_change(&self, change: ResourceChange, resource: HTTPRoute) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "HTTPRoute",
            route_name = ?resource.metadata.name,
            route_namespace = ?resource.metadata.namespace,
            "Applying HTTPRoute resource change"
        );
        execute_change_on_cache(change, &self.routes, resource);
    }

    /// Apply GRPCRoute change
    pub fn apply_grpc_route_change(&self, change: ResourceChange, resource: GRPCRoute) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "GRPCRoute",
            route_name = ?resource.metadata.name,
            route_namespace = ?resource.metadata.namespace,
            "Applying GRPCRoute resource change"
        );
        execute_change_on_cache(change, &self.grpc_routes, resource);
    }

    /// Apply TCPRoute change
    pub fn apply_tcp_route_change(&self, change: ResourceChange, resource: TCPRoute) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "TCPRoute",
            route_name = ?resource.metadata.name,
            route_namespace = ?resource.metadata.namespace,
            "Applying TCPRoute resource change"
        );
        execute_change_on_cache(change, &self.tcp_routes, resource);
    }

    /// Apply UDPRoute change
    pub fn apply_udp_route_change(&self, change: ResourceChange, resource: UDPRoute) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "UDPRoute",
            route_name = ?resource.metadata.name,
            route_namespace = ?resource.metadata.namespace,
            "Applying UDPRoute resource change"
        );
        execute_change_on_cache(change, &self.udp_routes, resource);
    }

    /// Apply TLSRoute change
    pub fn apply_tls_route_change(&self, change: ResourceChange, resource: TLSRoute) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "TLSRoute",
            route_name = ?resource.metadata.name,
            route_namespace = ?resource.metadata.namespace,
            "Applying TLSRoute resource change"
        );
        execute_change_on_cache(change, &self.tls_routes, resource);
    }

    /// Apply Service change
    pub fn apply_service_change(&self, change: ResourceChange, resource: Service) {
        tracing::info!(
            component = "config_server",
            kind = "Service",
            "Applying Service resource change"
        );
        execute_change_on_cache(change, &self.services, resource);
    }

    /// Apply EndpointSlice change
    pub fn apply_endpoint_slice_change(&self, change: ResourceChange, resource: EndpointSlice) {
        tracing::info!(
            component = "config_server",
            kind = "EndpointSlice",
            "Applying EndpointSlice resource change"
        );
        execute_change_on_cache(change, &self.endpoint_slices, resource);
    }

    /// Apply Endpoints change
    pub fn apply_endpoint_change(&self, change: ResourceChange, resource: Endpoints) {
        tracing::info!(
            component = "config_server",
            kind = "Endpoints",
            "Applying Endpoints resource change"
        );
        execute_change_on_cache(change, &self.endpoints, resource);
    }

    /// Apply EdgionPlugins change
    pub fn apply_edgion_plugins_change(&self, change: ResourceChange, resource: EdgionPlugins) {
        tracing::info!(
            component = "config_server",
            kind = "EdgionPlugins",
            "Applying EdgionPlugins resource change"
        );
        execute_change_on_cache(change, &self.edgion_plugins, resource);
    }

    /// Apply EdgionStreamPlugins change
    pub fn apply_edgion_stream_plugins_change(&self, change: ResourceChange, resource: EdgionStreamPlugins) {
        tracing::info!(
            component = "config_server",
            kind = "EdgionStreamPlugins",
            "Applying EdgionStreamPlugins resource change"
        );
        execute_change_on_cache(change, &self.edgion_stream_plugins, resource);
    }

    /// Apply PluginMetaData change
    pub fn apply_plugin_metadata_change(&self, change: ResourceChange, resource: PluginMetaData) {
        tracing::info!(
            component = "config_server",
            kind = "PluginMetaData",
            metadata_name = ?resource.metadata.name,
            data_type = ?resource.data_type(),
            "Applying PluginMetaData resource change"
        );
        execute_change_on_cache(change, &self.plugin_metadata, resource);
    }

    /// Apply LinkSys change
    pub fn apply_link_sys_change(&self, change: ResourceChange, resource: LinkSys) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "LinkSys",
            "Applying LinkSys resource change"
        );
        execute_change_on_cache(change, &self.link_sys, resource);
    }

    /// Apply GatewayClass change
    pub fn apply_gateway_class_change(&self, change: ResourceChange, resource: GatewayClass) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "GatewayClass",
            "Applying GatewayClass resource change"
        );
        execute_change_on_cache(change, &self.gateway_classes, resource);
    }

    /// Apply EdgionGatewayConfig change
    pub fn apply_edgion_gateway_config_change(&self, change: ResourceChange, resource: EdgionGatewayConfig) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "EdgionGatewayConfig",
            "Applying EdgionGatewayConfig resource change"
        );
        execute_change_on_cache(change, &self.edgion_gateway_configs, resource);
    }

    /// Apply ReferenceGrant change
    pub fn apply_reference_grant_change(&self, change: ResourceChange, resource: ReferenceGrant) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "ReferenceGrant",
            "Applying ReferenceGrant resource change"
        );
        execute_change_on_cache(change, &self.reference_grants, resource);
    }

    /// Apply BackendTLSPolicy change
    pub fn apply_backend_tls_policy_change(&self, change: ResourceChange, resource: BackendTLSPolicy) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "BackendTLSPolicy",
            "Applying BackendTLSPolicy resource change"
        );
        execute_change_on_cache(change, &self.backend_tls_policies, resource);
    }

    // ==================== Resource Changes (simplified) ====================
    // Note: Secret reference handling is now done by resource_processor

    /// Apply EdgionTls change
    /// Note: Secret resolution is handled by EdgionTlsProcessor in resource_processor
    pub fn apply_edgion_tls_change(&self, change: ResourceChange, resource: EdgionTls) {
        tracing::info!(
            component = "config_server",
            kind = "EdgionTls",
            name = ?resource.metadata.name,
            namespace = ?resource.metadata.namespace,
            "Applying EdgionTls resource change"
        );
        execute_change_on_cache(change, &self.edgion_tls, resource);
    }

    /// Apply Gateway change
    /// Note: Secret resolution is handled by GatewayProcessor in resource_processor
    pub fn apply_gateway_change(&self, change: ResourceChange, resource: Gateway) {
        tracing::info!(
            component = "config_server",
            kind = "Gateway",
            name = ?resource.metadata.name,
            namespace = ?resource.metadata.namespace,
            "Applying Gateway resource change"
        );
        execute_change_on_cache(change, &self.gateways, resource);
    }

    /// Apply Secret change
    /// Note: SecretStore updates and cascading requeue handled by SecretProcessor in resource_processor
    pub fn apply_secret_change(&self, change: ResourceChange, resource: Secret) {
        tracing::info!(
            component = "config_server",
            kind = "Secret",
            name = ?resource.metadata.name,
            namespace = ?resource.metadata.namespace,
            "Applying Secret resource change"
        );
        execute_change_on_cache(change, &self.secrets, resource);
    }

    // ==================== Typed List Methods (for compatibility) ====================

    /// List HTTP routes
    pub fn list_routes(&self) -> crate::core::conf_sync::types::ListData<HTTPRoute> {
        self.routes.list_owned()
    }

    /// List gRPC routes
    pub fn list_grpc_routes(&self) -> crate::core::conf_sync::types::ListData<GRPCRoute> {
        self.grpc_routes.list_owned()
    }

    /// List TCP routes
    pub fn list_tcp_routes(&self) -> crate::core::conf_sync::types::ListData<TCPRoute> {
        self.tcp_routes.list_owned()
    }

    /// List UDP routes
    pub fn list_udp_routes(&self) -> crate::core::conf_sync::types::ListData<UDPRoute> {
        self.udp_routes.list_owned()
    }

    /// List TLS routes
    pub fn list_tls_routes(&self) -> crate::core::conf_sync::types::ListData<TLSRoute> {
        self.tls_routes.list_owned()
    }

    /// List LinkSys
    pub fn list_link_sys(&self) -> crate::core::conf_sync::types::ListData<LinkSys> {
        self.link_sys.list_owned()
    }

    /// List PluginMetaData
    pub fn list_plugin_metadata(&self) -> crate::core::conf_sync::types::ListData<PluginMetaData> {
        self.plugin_metadata.list_owned()
    }

    /// List Services
    pub fn list_services(&self) -> crate::core::conf_sync::types::ListData<Service> {
        self.services.list_owned()
    }

    /// List EndpointSlices
    pub fn list_endpoint_slices(&self) -> crate::core::conf_sync::types::ListData<EndpointSlice> {
        self.endpoint_slices.list_owned()
    }

    /// List Endpoints
    pub fn list_endpoints(&self) -> crate::core::conf_sync::types::ListData<Endpoints> {
        self.endpoints.list_owned()
    }

    /// List EdgionTls
    pub fn list_edgion_tls(&self) -> crate::core::conf_sync::types::ListData<EdgionTls> {
        self.edgion_tls.list_owned()
    }

    /// List EdgionPlugins
    pub fn list_edgion_plugins(&self) -> crate::core::conf_sync::types::ListData<EdgionPlugins> {
        self.edgion_plugins.list_owned()
    }

    /// List EdgionStreamPlugins
    pub fn list_edgion_stream_plugins(&self) -> crate::core::conf_sync::types::ListData<EdgionStreamPlugins> {
        self.edgion_stream_plugins.list_owned()
    }

    /// List ReferenceGrants
    pub fn list_reference_grants(&self) -> crate::core::conf_sync::types::ListData<ReferenceGrant> {
        self.reference_grants.list_owned()
    }

    /// List BackendTLSPolicies
    pub fn list_backend_tls_policies(&self) -> crate::core::conf_sync::types::ListData<BackendTLSPolicy> {
        self.backend_tls_policies.list_owned()
    }

    /// List Secrets
    pub fn list_secrets(&self) -> crate::core::conf_sync::types::ListData<Secret> {
        self.secrets.list_owned()
    }

    /// List GatewayClasses
    pub fn list_gateway_classes(&self) -> crate::core::conf_sync::types::ListData<GatewayClass> {
        self.gateway_classes.list_owned()
    }

    /// List Gateways
    pub fn list_gateways(&self) -> crate::core::conf_sync::types::ListData<Gateway> {
        self.gateways.list_owned()
    }

    /// List EdgionGatewayConfigs
    pub fn list_edgion_gateway_configs(&self) -> crate::core::conf_sync::types::ListData<EdgionGatewayConfig> {
        self.edgion_gateway_configs.list_owned()
    }

    /// Print all configuration
    pub async fn print_config(&self) {
        use crate::core::utils::format_resource_info;

        println!("\n==========================");

        // GatewayClass
        let list_data = self.list_gateway_classes();
        println!(
            "GatewayClass (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, gc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(gc));
        }

        // EdgionGatewayConfig
        let list_data = self.list_edgion_gateway_configs();
        println!(
            "EdgionGatewayConfig (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, egwc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(egwc));
        }

        // Gateway
        let list_data = self.list_gateways();
        println!(
            "Gateway (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, gw) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(gw));
        }

        println!();

        // HTTPRoutes
        let list_data = self.list_routes();
        println!(
            "HTTPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // GRPCRoutes
        let list_data = self.list_grpc_routes();
        println!(
            "GRPCRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TCPRoutes
        let list_data = self.list_tcp_routes();
        println!(
            "TCPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // UDPRoutes
        let list_data = self.list_udp_routes();
        println!(
            "UDPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TLSRoutes
        let list_data = self.list_tls_routes();
        println!(
            "TLSRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // LinkSys
        let list_data = self.list_link_sys();
        println!(
            "LinkSys (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, link_sys) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(link_sys));
        }

        // Services
        let list_data = self.list_services();
        println!(
            "Services (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, svc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(svc));
        }

        // EndpointSlices
        let list_data = self.list_endpoint_slices();
        println!(
            "EndpointSlices (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, es) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(es));
        }

        // EdgionTls
        let list_data = self.list_edgion_tls();
        println!(
            "EdgionTls (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, tls) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(tls));
        }

        // EdgionPlugins
        let list_data = self.list_edgion_plugins();
        println!(
            "EdgionPlugins (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, plugin) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(plugin));
        }

        // PluginMetaData
        let list_data = self.list_plugin_metadata();
        println!(
            "PluginMetaData (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, metadata) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(metadata));
        }

        // Secrets
        let list_data = self.list_secrets();
        println!(
            "Secrets (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, secret) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(secret));
        }

        println!("==========================\n");
    }
}
