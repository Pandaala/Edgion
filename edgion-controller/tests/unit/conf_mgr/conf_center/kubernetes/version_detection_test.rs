use super::*;

#[test]
fn tls_route_preflight_matches_the_single_canonical_watch_and_status_type() {
    use edgion_resources::TLSRoute;
    use kube::Resource;
    let entries: Vec<_> = canonical_sources().filter(|entry| entry.kind == "TLSRoute").collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].version, TLSRoute::version(&()));
    assert_eq!(GROUP, TLSRoute::group(&()));
    assert_eq!(TLSRoute::api_version(&()), "gateway.networking.k8s.io/v1");
}

#[test]
fn unavailable_discovery_results_are_classified_as_absent() {
    for error in [
        DiscoveryError::MissingKind("Gateway".into()),
        DiscoveryError::MissingApiGroup("gateway.networking.k8s.io".into()),
        DiscoveryError::MissingResource("gateways".into()),
        DiscoveryError::EmptyApiGroup("gateway.networking.k8s.io/v1".into()),
    ] {
        assert!(is_api_unavailable(&kube::Error::Discovery(error)));
    }
}

#[test]
fn invalid_discovery_configuration_is_not_classified_as_absent() {
    let error = kube::Error::Discovery(DiscoveryError::InvalidGroupVersion("invalid".into()));
    assert!(!is_api_unavailable(&error));
}

#[test]
fn only_api_not_found_is_classified_as_absent() {
    let response = |code| kube::error::ErrorResponse {
        status: "Failure".into(),
        message: "test".into(),
        reason: "test".into(),
        code,
    };
    assert!(is_api_unavailable(&kube::Error::Api(response(404))));
    assert!(!is_api_unavailable(&kube::Error::Api(response(403))));
}

#[test]
fn backend_is_a_known_no_watch_kind() {
    assert!(other_known_kind_names().contains("EdgionBackend"));
}
