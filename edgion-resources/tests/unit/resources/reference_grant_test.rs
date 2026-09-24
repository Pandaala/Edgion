use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

#[allow(clippy::too_many_arguments)]
fn create_test_grant(
    namespace: &str,
    name: &str,
    from_namespace: &str,
    from_group: &str,
    from_kind: &str,
    to_group: &str,
    to_kind: &str,
    to_name: Option<&str>,
) -> ReferenceGrant {
    ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: from_group.to_string(),
                kind: from_kind.to_string(),
                namespace: from_namespace.to_string(),
            }],
            to: vec![ReferenceGrantTo {
                group: to_group.to_string(),
                kind: to_kind.to_string(),
                name: to_name.map(|s| s.to_string()),
            }],
        },
    }
}

#[test]
fn test_allows_reference_basic() {
    // Grant: allow HTTPRoute from ns-source to access Service in ns-target
    let grant = create_test_grant(
        "ns-target",
        "test-grant",
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );

    // Should allow: matching from and to
    assert!(grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("my-service")
    ));

    // Should deny: wrong from namespace
    assert!(!grant.allows_reference(
        "ns-other",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("my-service")
    ));

    // Should deny: wrong from kind
    assert!(!grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "TCPRoute",
        "",
        "Service",
        Some("my-service")
    ));

    // Should deny: wrong to kind
    assert!(!grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Secret",
        Some("my-secret")
    ));
}

#[test]
fn test_allows_reference_with_specific_name() {
    // Grant: allow HTTPRoute from ns-source to access specific Service "allowed-svc" in ns-target
    let grant = create_test_grant(
        "ns-target",
        "test-grant",
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("allowed-svc"),
    );

    // Should allow: specific service name matches
    assert!(grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("allowed-svc")
    ));

    // Should deny: different service name
    assert!(!grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("other-svc")
    ));
}

#[test]
fn test_allows_reference_wildcard_name() {
    // Grant: allow HTTPRoute from ns-source to access any Service in ns-target (name: None)
    let grant = create_test_grant(
        "ns-target",
        "test-grant",
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        None,
    );

    // Should allow: any service name
    assert!(grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("any-service")
    ));

    assert!(grant.allows_reference(
        "ns-source",
        "gateway.networking.k8s.io",
        "HTTPRoute",
        "",
        "Service",
        Some("another-service")
    ));
}
