use super::super::test_util;
use super::*;
use edgion_resources::resources::backend_tls_policy::WellKnownCACertificates;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsValidation};

fn owner(endpoint: &str, tls: Option<serde_json::Value>) -> LinkSys {
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":"otlp","namespace":"apps"},
        "spec":{"type":"otlp","config":{"endpoint":endpoint,"tls":tls}}
    }))
    .unwrap()
}
fn empty() -> Arc<GlobalOutboundTlsSnapshot> {
    Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))
}
fn hash(input: &PreparedOtlpTls) -> [u8; 32] {
    let mut hash = FingerprintHasher::new();
    input.hash_inheritance(&mut hash);
    hash.finish()
}

#[test]
fn source_validation_preserves_explicit_otlp_identity_for_all_transport_selections() {
    for endpoint in ["http://collector.example:4317", "https://collector.example:4317"] {
        for tls in [None, Some(serde_json::json!({"enabled":false}))] {
            let valid = owner(endpoint, tls);
            assert!(PreparedOtlpTls::with_snapshot(Arc::new(valid.clone()), empty()).is_ok());
            for case in [
                "name",
                "empty-name",
                "invalid-name",
                "namespace",
                "empty-namespace",
                "invalid-namespace",
            ] {
                let mut invalid = valid.clone();
                match case {
                    "name" => invalid.metadata.name = None,
                    "empty-name" => invalid.metadata.name = Some("".into()),
                    "invalid-name" => invalid.metadata.name = Some("Bad/Name".into()),
                    "namespace" => invalid.metadata.namespace = None,
                    "empty-namespace" => invalid.metadata.namespace = Some("".into()),
                    _ => invalid.metadata.namespace = Some("Bad".into()),
                }
                assert!(
                    PreparedOtlpTls::with_snapshot(Arc::new(invalid), empty()).is_err(),
                    "{case}"
                );
            }
        }
    }
    let wrong = serde_json::from_value(serde_json::json!({"metadata":{"name":"redis","namespace":"apps"},
        "spec":{"type":"redis","config":{"addr":"redis://redis.example:6379"}}}))
    .unwrap();
    assert!(PreparedOtlpTls::with_snapshot(Arc::new(wrong), empty()).is_err());
}

#[test]
fn active_whole_pairing_is_proved_before_transport_materialization_and_inactive_is_inert() {
    let valid = serde_json::json!({"enabled":true,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"a"},{"kind":"Secret","name":"b"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"a","namespace":"apps"}},{"metadata":{"name":"b","namespace":"apps"}}],
        "clientCertificateRef":{"name":"client"},
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"}}
    });
    assert!(PreparedOtlpTls::with_snapshot(
        Arc::new(owner("https://collector.example:4317", Some(valid.clone()))),
        empty()
    )
    .is_ok());
    for case in [
        "group",
        "order",
        "extra-ca",
        "missing-client",
        "wrong-client",
        "extra-client",
        "marker",
        "bad-target",
    ] {
        let mut tls = valid.clone();
        match case {
            "group" => tls["validation"]["caCertificateRefs"][0]["group"] = "foreign.example".into(),
            "order" => tls["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "extra-ca" => tls["resolvedCaCertificates"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({})),
            "missing-client" => tls["resolvedClientCertificate"] = serde_json::Value::Null,
            "wrong-client" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = "other".into(),
            "extra-client" => tls["clientCertificateRef"] = serde_json::Value::Null,
            "marker" => {
                tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({"edgion.io/controller-ca-source":"ConfigMap"})
            }
            _ => tls["validation"]["caCertificateRefs"][0]["namespace"] = "".into(),
        }
        assert!(
            PreparedOtlpTls::with_snapshot(
                Arc::new(owner("https://collector.example:4317", Some(tls.clone()))),
                empty()
            )
            .is_err(),
            "{case}"
        );
        tls["enabled"] = false.into();
        assert!(
            PreparedOtlpTls::with_snapshot(Arc::new(owner("https://collector.example:4317", Some(tls))), empty())
                .is_ok(),
            "inactive {case}"
        );
    }
}

#[test]
fn https_implicit_tls_consumes_captured_globals_while_http_consumes_none() {
    let a = empty();
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    for tls in [None, Some(serde_json::json!({"enabled":false}))] {
        for endpoint in ["http://collector.example:4317", "https://collector.example:4317"] {
            let actual = Arc::new(owner(endpoint, tls.clone()));
            let left = PreparedOtlpTls::with_snapshot(actual.clone(), a.clone()).unwrap();
            let right = PreparedOtlpTls::with_snapshot(actual, b.clone()).unwrap();
            assert_eq!(hash(&left) == hash(&right), !endpoint.starts_with("https"));
            if endpoint.starts_with("https") {
                assert!(right.resolve_tls().is_err(), "OTLP cannot disable verification");
            } else {
                assert!(right.resolve_tls().unwrap().is_none());
            }
        }
    }
    assert!(PreparedOtlpTls::with_snapshot(
        Arc::new(owner(
            "http://collector.example:4317",
            Some(serde_json::json!({"enabled":true}))
        )),
        empty()
    )
    .is_err());
}

#[test]
fn local_overrides_mask_inherited_changes_and_failures_on_the_matching_axes() {
    let policy = OutboundTlsConfig {
        verify: false,
        validation: Some(OutboundTlsValidation {
            hostname: Some("global.example".into()),
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
            ..Default::default()
        }),
        ..Default::default()
    };
    let actual = Arc::new(owner(
        "https://collector.example:4317",
        Some(serde_json::json!({"enabled":true,"verify":true,"validation":{}})),
    ));
    let left = PreparedOtlpTls::with_snapshot(actual.clone(), empty()).unwrap();
    let right = PreparedOtlpTls::with_snapshot(actual, test_util::snapshot(policy)).unwrap();
    assert_eq!(hash(&left), hash(&right));
    assert!(right.resolve_tls().unwrap().is_some());

    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    let bad = Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)]));
    assert!(
        PreparedOtlpTls::with_snapshot(Arc::new(owner("https://collector.example:4317", None)), bad.clone()).is_err()
    );
    assert!(
        PreparedOtlpTls::with_snapshot(Arc::new(owner("http://collector.example:4317", None)), bad.clone()).is_ok()
    );
    let local = Arc::new(owner(
        "https://collector.example:4317",
        Some(serde_json::json!({"enabled":true,"verify":true})),
    ));
    assert!(
        PreparedOtlpTls::with_snapshot(local, bad).is_ok(),
        "local verify masks only the invalid verify source"
    );
}

fn global_pair(ca: &str, client: &str, invalid: bool) -> Arc<GlobalOutboundTlsSnapshot> {
    let owner: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global"},"spec":{"outboundTls":{
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca","namespace":"apps"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"apps"},"data":{"ca.crt":ca}}],
            "clientCertificateRef":{"name":"client","namespace":"apps"},
            "resolvedClientCertificate":{"metadata":{"name":"client","namespace":if invalid {"other"} else {"apps"}},"data":{"tls.crt":client,"tls.key":"a2V5"}}
        }}
    })).unwrap();
    Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(owner)]))
}

#[test]
fn inherited_ca_and_client_material_change_identity_and_local_client_masks_only_its_axis() {
    let a = global_pair("Y2EtYQ==", "Y2VydC1h", false);
    let ca = global_pair("Y2EtYg==", "Y2VydC1h", false);
    let client = global_pair("Y2EtYQ==", "Y2VydC1i", false);
    let invalid = global_pair("Y2EtYQ==", "Y2VydC1h", true);
    let actual = Arc::new(owner("https://collector.example:4317", None));
    let base = PreparedOtlpTls::with_snapshot(actual.clone(), a.clone()).unwrap();
    assert_ne!(
        hash(&base),
        hash(&PreparedOtlpTls::with_snapshot(actual.clone(), ca.clone()).unwrap())
    );
    assert_ne!(
        hash(&base),
        hash(&PreparedOtlpTls::with_snapshot(actual.clone(), client.clone()).unwrap())
    );
    assert!(PreparedOtlpTls::with_snapshot(actual, invalid.clone()).is_err());

    let local = Arc::new(owner(
        "https://collector.example:4317",
        Some(serde_json::json!({
            "enabled":true,"clientCertificateRef":{"name":"local"},
            "resolvedClientCertificate":{"metadata":{"name":"local","namespace":"apps"},"data":{"tls.crt":"bG9jYWw=","tls.key":"a2V5"}}
        })),
    ));
    let base = PreparedOtlpTls::with_snapshot(local.clone(), a).unwrap();
    assert_eq!(
        hash(&base),
        hash(&PreparedOtlpTls::with_snapshot(local.clone(), client).unwrap())
    );
    assert_eq!(
        hash(&base),
        hash(&PreparedOtlpTls::with_snapshot(local.clone(), invalid).unwrap())
    );
    assert_ne!(
        hash(&base),
        hash(&PreparedOtlpTls::with_snapshot(local, ca).unwrap()),
        "client override must not mask inherited CA"
    );
}
