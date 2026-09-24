use super::*;
use crate::resource::kind::ResourceKind;

#[test]
fn test_all_resource_infos_count() {
    // We have 22 resource types (excluding Unspecified)
    assert_eq!(ALL_RESOURCE_INFOS.len(), 22);
}

#[test]
fn test_resource_kind_from_name() {
    assert_eq!(
        resource_kind_from_name("gatewayclass"),
        Some(ResourceKind::GatewayClass)
    );

    // alias lookup
    assert_eq!(
        resource_kind_from_name("edgiongwconfig"),
        Some(ResourceKind::EdgionGatewayConfig)
    );

    // case-insensitive
    assert_eq!(
        resource_kind_from_name("GatewayClass"),
        Some(ResourceKind::GatewayClass)
    );

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
fn test_cluster_scoped() {
    assert!(is_kind_cluster_scoped(ResourceKind::GatewayClass));
    assert!(is_kind_cluster_scoped(ResourceKind::EdgionGatewayConfig));
    assert!(!is_kind_cluster_scoped(ResourceKind::Gateway));
    assert!(!is_kind_cluster_scoped(ResourceKind::HTTPRoute));
}

#[test]
fn test_default_capacity_for_kind() {
    use crate::resource::macros::{CAPACITY_NORMAL, CAPACITY_SMALL};

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
        "secret",
        "edgionplugins",
        "grpcroute",
        "tcproute",
        "udproute",
        "edgionconfigdata",
        "tlsroute",
        "linksys",
        "edgionstreamplugins",
        "referencegrant",
        "backendtlspolicy",
        "unknown", // should return None
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
