use super::*;
use edgion_resources::resources::common::TlsConfig;

#[test]
fn default_redirect_is_deny() {
    assert!(matches!(OutboundRedirect::default(), OutboundRedirect::Deny));
}

#[test]
fn owned_client_rejects_all_raw_tls_substitutions_and_inactive_ignores_bad_global_source() {
    use crate::link_sys_core::outbound_tls::{validate_local_tls_selection, GlobalOutboundTlsSnapshot};
    use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    let owner: edgion_resources::LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"resolver","namespace":"apps"},"spec":{"type":"httpdns","config":{"preset":"aliyun"}}
    }))
    .unwrap();
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let proof = validate_local_tls_selection(binding.outbound_tls_selection().unwrap()).unwrap();
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global","namespace":"apps"},"spec":{}
    }))
    .unwrap();
    let defaults = std::sync::Arc::new(GlobalOutboundTlsSnapshot::from_configs([std::sync::Arc::new(invalid)]));
    assert!(build_outbound_client_with_tls_selection(OutboundClientConfig::default(), &proof, &defaults).is_ok());
    let preconfigured =
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    for cfg in [
        OutboundClientConfig {
            tls: Some(TlsConfig::default()),
            ..Default::default()
        },
        OutboundClientConfig {
            tls_defaults: Some(std::sync::Arc::clone(&defaults)),
            ..Default::default()
        },
        OutboundClientConfig {
            preconfigured_tls: Some(preconfigured),
            ..Default::default()
        },
    ] {
        let error = build_outbound_client_with_tls_selection(cfg, &proof, &defaults).unwrap_err();
        assert_eq!(
            error.to_string(),
            "OutboundClient: proved local TLS cannot be replaced by raw TLS inputs"
        );
    }
}

#[test]
fn fixed_header_owned_factory_rejects_every_raw_tls_override() {
    use crate::link_sys_core::outbound_tls::{GlobalOutboundTlsSnapshot, PreparedLinkSysTls};
    use std::sync::Arc;
    let owner: edgion_resources::LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"es","namespace":"apps"},
        "spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"]}}
    }))
    .unwrap();
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(invalid)]));
    let prepared = PreparedLinkSysTls::with_snapshot(Arc::new(owner), Arc::clone(&defaults)).unwrap();
    prepared
        .with_tls_selection(|proof, global| {
            let headers = reqwest::header::HeaderMap::from_iter([(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            )]);
            assert!(build_outbound_client_with_tls_selection_and_headers(
                OutboundClientConfig::default(),
                proof,
                global,
                headers
            )
            .is_ok());
            let preconfigured =
                rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_root_certificates(rustls::RootCertStore::empty())
                    .with_no_client_auth();
            for cfg in [
                OutboundClientConfig {
                    tls: Some(TlsConfig::default()),
                    ..Default::default()
                },
                OutboundClientConfig {
                    tls_defaults: Some(Arc::clone(&defaults)),
                    ..Default::default()
                },
                OutboundClientConfig {
                    preconfigured_tls: Some(preconfigured),
                    ..Default::default()
                },
            ] {
                assert_eq!(
                    build_outbound_client_with_tls_selection_and_headers(cfg, proof, global, Default::default())
                        .unwrap_err()
                        .to_string(),
                    "OutboundClient: proved local TLS cannot be replaced by raw TLS inputs"
                );
            }
            Ok(())
        })
        .unwrap();
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
