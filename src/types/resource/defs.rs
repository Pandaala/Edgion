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
//! - `base_conf_kind_names()` function
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
        kind_aliases: ["edgiongwconfig", "ztracegatewayconfig"],
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
    Endpoint {
        enum_value: 19,
        kind_name: "endpoint",
        kind_aliases: ["endpoints"],
        cache_field: endpoints,
        capacity_field: endpoints_capacity,
        default_capacity: normal,
    },

    // ==================== Security and Policy Resources ====================
    EdgionTls {
        enum_value: 7,
        kind_name: "edgiontls",
        kind_aliases: ["ztracetls"],
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
    PluginMetaData {
        enum_value: 13,
        kind_name: "pluginmetadata",
        kind_aliases: [],
        cache_field: plugin_metadata,
        capacity_field: plugin_metadata_capacity,
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
mod tests {
    use super::*;
    use crate::types::resource::kind::ResourceKind;

    #[test]
    fn test_all_resource_infos_count() {
        // We have 20 resource types (excluding Unspecified)
        assert_eq!(ALL_RESOURCE_INFOS.len(), 20);
    }

    #[test]
    fn test_resource_kind_from_name() {
        // Test exact match
        assert_eq!(
            resource_kind_from_name("gatewayclass"),
            Some(ResourceKind::GatewayClass)
        );

        // Test alias
        assert_eq!(
            resource_kind_from_name("edgiongwconfig"),
            Some(ResourceKind::EdgionGatewayConfig)
        );

        // Test case insensitivity
        assert_eq!(
            resource_kind_from_name("GatewayClass"),
            Some(ResourceKind::GatewayClass)
        );

        // Test unknown
        assert_eq!(resource_kind_from_name("unknown"), None);
    }

    #[test]
    fn test_get_resource_info() {
        let info = get_resource_info(ResourceKind::GatewayClass);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.kind_name, "gatewayclass");
        assert!(info.cluster_scoped);
        assert!(info.is_base_conf);
    }

    #[test]
    fn test_base_conf_resources() {
        let base_conf = base_conf_kind_names();
        assert!(base_conf.contains(&"gateway_classes"));
        assert!(base_conf.contains(&"gateways"));
        assert!(base_conf.contains(&"edgion_gateway_configs"));
        assert_eq!(base_conf.len(), 3);
    }

    #[test]
    fn test_cluster_scoped() {
        assert!(is_kind_cluster_scoped(ResourceKind::GatewayClass));
        assert!(is_kind_cluster_scoped(ResourceKind::EdgionGatewayConfig));
        assert!(!is_kind_cluster_scoped(ResourceKind::Gateway));
        assert!(!is_kind_cluster_scoped(ResourceKind::HTTPRoute));
    }

    #[test]
    fn test_default_capacity_for_kind() {
        use crate::types::resource::macros::{CAPACITY_NORMAL, CAPACITY_SMALL};

        // PascalCase variant name (used by spawn)
        assert_eq!(default_capacity_for_kind("GatewayClass"), Some(CAPACITY_SMALL));
        assert_eq!(default_capacity_for_kind("HTTPRoute"), Some(CAPACITY_NORMAL));
        assert_eq!(default_capacity_for_kind("EdgionAcme"), Some(CAPACITY_SMALL));
        assert_eq!(default_capacity_for_kind("Service"), Some(CAPACITY_NORMAL));

        // Lowercase kind_name (used in defs)
        assert_eq!(default_capacity_for_kind("gatewayclass"), Some(CAPACITY_SMALL));
        assert_eq!(default_capacity_for_kind("httproute"), Some(CAPACITY_NORMAL));
        assert_eq!(default_capacity_for_kind("edgionacme"), Some(CAPACITY_SMALL));

        // cache_field_name
        assert_eq!(default_capacity_for_kind("gateway_classes"), Some(CAPACITY_SMALL));
        assert_eq!(default_capacity_for_kind("routes"), Some(CAPACITY_NORMAL));
        assert_eq!(default_capacity_for_kind("edgion_acme"), Some(CAPACITY_SMALL));

        // Unknown
        assert_eq!(default_capacity_for_kind("unknown"), None);
    }

    #[test]
    fn test_resource_kind_info_has_capacity() {
        let info = get_resource_info(ResourceKind::GatewayClass).unwrap();
        assert_eq!(info.default_capacity, 50);

        let info = get_resource_info(ResourceKind::HTTPRoute).unwrap();
        assert_eq!(info.default_capacity, 200);

        let info = get_resource_info(ResourceKind::EdgionAcme).unwrap();
        assert_eq!(info.default_capacity, 50);
    }

    #[test]
    fn test_sync_with_original_from_kind_name() {
        // This test ensures that all resources defined in resource_defs.rs
        // are also recognized by the original ResourceKind::from_kind_name method.
        // This validates the bidirectional sync between the two definitions.
        for info in ALL_RESOURCE_INFOS {
            let result = ResourceKind::from_kind_name(info.kind_name);
            assert!(
                result.is_some(),
                "ResourceKind::from_kind_name should recognize '{}' defined in resource_defs.rs",
                info.kind_name
            );
            assert_eq!(
                result.unwrap(),
                info.kind,
                "ResourceKind mismatch for '{}'",
                info.kind_name
            );
        }
    }

    #[test]
    fn test_macro_generated_matches_original() {
        // Verify that the macro-generated function produces the same results
        // as the original hand-written function
        let test_cases = [
            "gatewayclass",
            "edgiongatewayconfig",
            "edgiongwconfig", // alias
            "gateway",
            "httproute",
            "service",
            "endpointslice",
            "edgiontls",
            "ztracetls", // alias
            "secret",
            "edgionplugins",
            "grpcroute",
            "tcproute",
            "udproute",
            "pluginmetadata",
            "tlsroute",
            "linksys",
            "edgionstreamplugins",
            "referencegrant",
            "backendtlspolicy",
            "endpoint",
            "endpoints", // alias
            "unknown",   // should return None
        ];

        for name in test_cases {
            let original = ResourceKind::from_kind_name(name);
            let generated = resource_kind_from_name(name);
            assert_eq!(
                original, generated,
                "Mismatch for '{}': original={:?}, generated={:?}",
                name, original, generated
            );
        }
    }
}
