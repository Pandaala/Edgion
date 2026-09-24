use super::*;

#[test]
fn service_backend_kind_from_kind_table() {
    let cases: &[(&str, Option<String>, ServiceBackendKind)] = &[
        ("default_none", None, ServiceBackendKind::Service),
        ("default_empty", Some(String::new()), ServiceBackendKind::Service),
        (
            "standard_service",
            Some("Service".to_string()),
            ServiceBackendKind::Service,
        ),
        (
            "standard_cluster_ip",
            Some("ServiceClusterIp".to_string()),
            ServiceBackendKind::ServiceClusterIp,
        ),
        (
            "standard_import",
            Some("ServiceImport".to_string()),
            ServiceBackendKind::ServiceImport,
        ),
        (
            "standard_external_name",
            Some("ServiceExternalName".to_string()),
            ServiceBackendKind::ServiceExternalName,
        ),
        (
            "endpoint_slice_long",
            Some("ServiceEndpointSlice".to_string()),
            ServiceBackendKind::ServiceEndpointSlice,
        ),
        (
            "endpoint_slice_short",
            Some("EndpointSlice".to_string()),
            ServiceBackendKind::ServiceEndpointSlice,
        ),
        (
            "unknown_kind",
            Some("UnknownKind".to_string()),
            ServiceBackendKind::Unsupported("UnknownKind".to_string()),
        ),
        (
            "unknown_random",
            Some("RandomString".to_string()),
            ServiceBackendKind::Unsupported("RandomString".to_string()),
        ),
        (
            "legacy_service_endpoint",
            Some("ServiceEndpoint".to_string()),
            ServiceBackendKind::Unsupported("ServiceEndpoint".to_string()),
        ),
        (
            "legacy_endpoint",
            Some("Endpoint".to_string()),
            ServiceBackendKind::Unsupported("Endpoint".to_string()),
        ),
    ];

    for (label, input, expected) in cases {
        assert_eq!(
            ServiceBackendKind::from_kind(input.as_ref()),
            *expected,
            "case '{}' failed",
            label
        );
    }
}
