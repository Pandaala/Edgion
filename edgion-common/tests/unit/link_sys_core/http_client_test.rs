use super::*;
use edgion_resources::resources::common::TlsConfig;

#[test]
fn default_redirect_is_deny() {
    assert!(matches!(OutboundRedirect::default(), OutboundRedirect::Deny));
}

#[test]
fn conflicting_tls_configuration_is_rejected() {
    let tls =
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    let error = build_outbound_client(OutboundClientConfig {
        tls: Some(TlsConfig::default()),
        preconfigured_tls: Some(tls),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("mutually exclusive"));
}

#[test]
fn build_default_client_succeeds() {
    assert!(build_outbound_client(OutboundClientConfig::default()).is_ok());
}

#[test]
fn configured_http2_response_header_list_size_is_bounded() {
    assert!(validate_outbound_http2_max_header_list_size(DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE - 1).is_err());
    assert!(validate_outbound_http2_max_header_list_size(DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE).is_ok());
    assert!(validate_outbound_http2_max_header_list_size(MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE).is_ok());
    assert!(validate_outbound_http2_max_header_list_size(MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE + 1).is_err());
}

#[test]
fn build_with_all_transport_knobs_succeeds() {
    let cfg = OutboundClientConfig {
        pool_max_idle_per_host: Some(5),
        pool_idle_timeout: Some(Duration::from_secs(10)),
        connect_timeout: Some(Duration::from_secs(3)),
        total_timeout: Some(Duration::from_secs(10)),
        redirect: OutboundRedirect::Limit(10),
        disable_proxy: true,
        ..Default::default()
    };
    assert!(build_outbound_client(cfg).is_ok());
}

#[test]
fn custom_redirect_policy_is_accepted() {
    let cfg = OutboundClientConfig {
        redirect: OutboundRedirect::Custom(reqwest::redirect::Policy::limited(3)),
        ..Default::default()
    };
    assert!(build_outbound_client(cfg).is_ok());
}

#[test]
fn insecure_tls_is_honored_when_verify_false() {
    use edgion_resources::resources::common::OutboundTlsLocal;

    let _guard = super::super::outbound_tls::outbound_tls_test_guard();
    let cfg = OutboundClientConfig {
        tls: Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        tls_context: Some("TestProvider"),
        ..Default::default()
    };
    let result = build_outbound_client(cfg);
    assert!(
        result.is_ok(),
        "verify=false must be honored (operator responsibility), got: {:?}",
        result.err()
    );
}
