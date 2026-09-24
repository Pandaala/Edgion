//! Unified Resource Definitions
//!
//! This module is the SINGLE SOURCE OF TRUTH for all resource type metadata.
//! All resource types are defined here using the `define_resources!` macro,
//! which auto-generates helper functions and ensures compile-time completeness.
//!
//! # Adding a New Resource
//! 1. Add a new entry to the `define_resources!` macro below
//! 2. The compiler will check if all required fields are provided
//! 3. The exhaustive check will ensure the ResourceKind variant exists
//!
//! # Generated Items
//! - `ResourceKindInfo` struct
//! - `ALL_RESOURCE_INFOS` static array
//! - `resource_kind_from_name()` function
//! - `get_resource_info()` function
//! - `all_resource_kind_names()` function
//! - `is_kind_cluster_scoped()` function

// The macro is imported via #[macro_use] in mod.rs, no explicit use needed

/// Default resource kinds that should NOT be synced to Gateway.
/// These resources are only processed on the Controller side.
/// This can be overridden via configuration file.
pub const DEFAULT_NO_SYNC_KINDS: &[&str] = &["ReferenceGrant", "Secret"];

define_resources! {
    // ==================== Base Configuration Resources ====================
    GatewayClass {
        enum_value: 1,
        kind_name: "gatewayclass",
        kind_aliases: [],
        cache_field: gateway_classes,
        capacity_field: gateway_classes_capacity,
        default_capacity: small,
        cluster_scoped: true,
        is_base_conf: true,
    },
    EdgionGatewayConfig {
        enum_value: 2,
        kind_name: "edgiongatewayconfig",
        kind_aliases: ["edgiongwconfig"],
        cache_field: edgion_gateway_configs,
        capacity_field: edgion_gateway_configs_capacity,
        default_capacity: small,
        cluster_scoped: true,
        is_base_conf: true,
    },
    Gateway {
        enum_value: 3,
        kind_name: "gateway",
        kind_aliases: [],
        cache_field: gateways,
        capacity_field: gateways_capacity,
        default_capacity: normal,
        is_base_conf: true,
    },

    // ==================== Route Resources ====================
    HTTPRoute {
        enum_value: 4,
        kind_name: "httproute",
        kind_aliases: [],
        cache_field: routes,
        capacity_field: routes_capacity,
        default_capacity: normal,
    },
    GRPCRoute {
        enum_value: 10,
        kind_name: "grpcroute",
        kind_aliases: [],
        cache_field: grpc_routes,
        capacity_field: grpc_routes_capacity,
        default_capacity: normal,
    },
    TCPRoute {
        enum_value: 11,
        kind_name: "tcproute",
        kind_aliases: [],
        cache_field: tcp_routes,
        capacity_field: tcp_routes_capacity,
        default_capacity: normal,
    },
    UDPRoute {
        enum_value: 12,
        kind_name: "udproute",
        kind_aliases: [],
        cache_field: udp_routes,
        capacity_field: udp_routes_capacity,
        default_capacity: normal,
    },
    TLSRoute {
        enum_value: 14,
        kind_name: "tlsroute",
        kind_aliases: [],
        cache_field: tls_routes,
        capacity_field: tls_routes_capacity,
        default_capacity: normal,
    },

    // ==================== Backend Resources ====================
    Service {
        enum_value: 5,
        kind_name: "service",
        kind_aliases: [],
        cache_field: services,
        capacity_field: services_capacity,
        default_capacity: normal,
    },
    EndpointSlice {
        enum_value: 6,
        kind_name: "endpointslice",
        kind_aliases: [],
        cache_field: endpoint_slices,
        capacity_field: endpoint_slices_capacity,
        default_capacity: normal,
    },
    // ==================== Security and Policy Resources ====================
    EdgionTls {
        enum_value: 7,
        kind_name: "edgiontls",
        kind_aliases: [],
        cache_field: edgion_tls,
        capacity_field: edgion_tls_capacity,
        default_capacity: normal,
    },
    Secret {
        enum_value: 8,
        kind_name: "secret",
        kind_aliases: [],
        cache_field: secrets,
        capacity_field: secrets_capacity,
        default_capacity: normal,
        in_registry: false,  // Secret follows related resources, not tracked independently
    },
    ReferenceGrant {
        enum_value: 17,
        kind_name: "referencegrant",
        kind_aliases: [],
        cache_field: reference_grants,
        capacity_field: reference_grants_capacity,
        default_capacity: normal,
    },
    BackendTLSPolicy {
        enum_value: 18,
        kind_name: "backendtlspolicy",
        kind_aliases: [],
        cache_field: backend_tls_policies,
        capacity_field: backend_tls_policies_capacity,
        default_capacity: normal,
    },

    // ==================== Plugin and Extension Resources ====================
    EdgionPlugins {
        enum_value: 9,
        kind_name: "edgionplugins",
        kind_aliases: [],
        cache_field: edgion_plugins,
        capacity_field: edgion_plugins_capacity,
        default_capacity: normal,
    },
    EdgionStreamPlugins {
        enum_value: 16,
        kind_name: "edgionstreamplugins",
        kind_aliases: [],
        cache_field: edgion_stream_plugins,
        capacity_field: edgion_stream_plugins_capacity,
        default_capacity: normal,
    },
    EdgionConfigData {
        enum_value: 13,
        kind_name: "edgionconfigdata",
        kind_aliases: [],
        cache_field: edgion_config_data,
        capacity_field: edgion_config_data_capacity,
        default_capacity: normal,
    },

    // ==================== ACME Resources ====================
    EdgionAcme {
        enum_value: 20,
        kind_name: "edgionacme",
        kind_aliases: [],
        cache_field: edgion_acme,
        capacity_field: edgion_acme_capacity,
        default_capacity: small,
    },

    // ==================== ConfigMap (CA cert resolution for Gateway frontendValidation) ====================
    ConfigMap {
        enum_value: 21,
        kind_name: "configmap",
        kind_aliases: [],
        cache_field: configmaps,
        capacity_field: configmaps_capacity,
        default_capacity: normal,
        in_registry: false,  // ConfigMap follows related resources, not tracked independently
    },

    // ==================== Backend Traffic Policy ====================
    EdgionBackendTrafficPolicy {
        enum_value: 22,
        kind_name: "edgionbackendtrafficpolicy",
        kind_aliases: [],
        cache_field: edgion_backend_traffic_policy,
        capacity_field: edgion_backend_traffic_policy_capacity,
        default_capacity: small,
    },

    // ==================== AI Gateway Resources ====================
    EdgionBackend {
        enum_value: 23,
        kind_name: "edgionbackend",
        kind_aliases: [],
        cache_field: edgion_backends,
        capacity_field: edgion_backends_capacity,
        default_capacity: normal,
    },

    // ==================== Infrastructure Resources ====================
    LinkSys {
        enum_value: 15,
        kind_name: "linksys",
        kind_aliases: [],
        cache_field: link_sys,
        capacity_field: link_sys_capacity,
        default_capacity: normal,
    },
}

#[cfg(test)]
#[path = "../../tests/unit/resource/defs_test.rs"]
mod tests;
