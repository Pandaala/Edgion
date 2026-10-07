use super::*;
fn fingerprint_etcd(owner: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    fingerprint_etcd_fields(owner, |_| Ok(()))
}

use edgion_resources::resources::link_sys::etcd::{EtcdKeepAlive, EtcdTimeout};
use edgion_resources::resources::link_sys::LinkSysSpec;

fn etcd_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("etcd".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Etcd(EtcdClientConfig {
                endpoints: endpoints.into_iter().map(str::to_string).collect(),
                auth: None,
                tls: None,
                timeout: None,
                keep_alive: None,
                namespace: None,
            }),
        },
        status: None,
    }
}

#[test]
fn endpoint_order_does_not_change_fingerprint() {
    let left = etcd_ls(vec!["http://b.example:2379", "http://a.example:2379"], None);
    let right = etcd_ls(vec!["http://a.example:2379", "http://b.example:2379"], None);
    assert!(fingerprint_etcd(&left)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&right).unwrap()));
}

#[test]
fn namespace_and_keepalive_are_rebuild() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let mut ns = base.clone();
    let mut ka = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut ns.spec.config {
        cfg.namespace = Some("/edgion".to_string());
    }
    if let SystemConfig::Etcd(cfg) = &mut ka.spec.config {
        cfg.keep_alive = Some(EtcdKeepAlive {
            time: Some(10),
            timeout: Some(5),
            permit_without_stream: Some(true),
        });
    }
    assert!(!fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&ns).unwrap()));
    assert!(!fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&ka).unwrap()));
}

#[test]
fn dial_and_request_timeouts_are_rebuild() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let mut timeout = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut timeout.spec.config {
        cfg.timeout = Some(EtcdTimeout {
            dial: Some(3000),
            request: Some(5000),
        });
    }
    assert!(!fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&timeout).unwrap()));
}

#[test]
fn incomplete_keepalive_is_rejected() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let mut partial = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut partial.spec.config {
        cfg.keep_alive = Some(EtcdKeepAlive {
            time: Some(10),
            timeout: None,
            permit_without_stream: Some(true),
        });
    }
    assert!(fingerprint_etcd(&partial).is_err());
}

#[test]
fn timeout_fingerprint_uses_clamped_values() {
    let mut a = etcd_ls(vec!["http://a.example:2379"], None);
    let mut b = a.clone();
    if let SystemConfig::Etcd(cfg) = &mut a.spec.config {
        cfg.timeout = Some(EtcdTimeout {
            dial: Some(MAX_DIAL_TIMEOUT_MS),
            request: Some(MAX_REQUEST_TIMEOUT_MS),
        });
    }
    if let SystemConfig::Etcd(cfg) = &mut b.spec.config {
        cfg.timeout = Some(EtcdTimeout {
            dial: Some(MAX_DIAL_TIMEOUT_MS + 1),
            request: Some(MAX_REQUEST_TIMEOUT_MS + 1),
        });
    }
    assert!(fingerprint_etcd(&a)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&b).unwrap()));
}

#[test]
fn effective_defaults_and_normalized_endpoints_are_equivalent() {
    let mut base = etcd_ls(vec!["http://a.example:2379"], None);
    if let SystemConfig::Etcd(cfg) = &mut base.spec.config {
        cfg.keep_alive = Some(EtcdKeepAlive {
            time: Some(10),
            timeout: Some(5),
            permit_without_stream: None,
        });
    }
    let mut equivalent = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut equivalent.spec.config {
        cfg.endpoints = vec!["http://A.EXAMPLE:2379/".to_owned(), "http://a.example:2379".to_owned()];
        cfg.namespace = Some(String::new());
        cfg.keep_alive.as_mut().unwrap().permit_without_stream = Some(false);
    }
    assert!(fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&equivalent).unwrap()));
}

fn candidate(
    owner: LinkSys,
    defaults: Arc<edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> anyhow::Result<EtcdCandidate> {
    EtcdCandidate::from_prepared(PreparedLinkSysTls::with_snapshot(Arc::new(owner), defaults)?)
}

fn with_tls(owner: &mut LinkSys, tls: serde_json::Value) {
    let SystemConfig::Etcd(config) = &mut owner.spec.config else {
        unreachable!()
    };
    config.tls = Some(serde_json::from_value(tls).unwrap());
}

#[test]
fn actual_source_and_ordered_pairing_precede_same_rv_and_zero_live_planning() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    for tls in [
        None,
        Some(serde_json::json!({"enabled":false})),
        Some(serde_json::json!({"enabled":true,"verify":true})),
    ] {
        let mut actual = etcd_ls(vec!["https://etcd.example:2379"], Some("42"));
        if let Some(tls) = tls {
            with_tls(&mut actual, tls);
        }
        let valid = candidate(actual.clone(), defaults.clone()).unwrap();
        let zero_live = EtcdApply::build("fp/etcd", &valid).unwrap();
        assert_eq!(EtcdApply::coverage(&zero_live).live, 0);
        assert!(EtcdApply::fingerprint_live(&zero_live).rv_skip_allows_reuse(&EtcdApply::fingerprint(&valid).unwrap()));
        for field in ["name", "empty-namespace", "bad-namespace"] {
            let mut invalid = actual.clone();
            match field {
                "name" => invalid.metadata.name = None,
                "empty-namespace" => invalid.metadata.namespace = Some("".into()),
                _ => invalid.metadata.namespace = Some("Bad".into()),
            }
            assert!(candidate(invalid, defaults.clone()).is_err());
        }
        actual.metadata.namespace = None;
        assert!(candidate(actual, defaults.clone()).is_ok());
    }
    let mut actual = etcd_ls(vec!["https://etcd.example:2379"], Some("42"));
    let valid = serde_json::json!({"enabled":true,"verify":true,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"a"},{"kind":"Secret","name":"b"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"a","namespace":"fp"}},{"metadata":{"name":"b","namespace":"fp"}}],
        "clientCertificateRef":{"name":"client"},
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"fp"}}
    });
    with_tls(&mut actual, valid.clone());
    assert!(
        candidate(actual.clone(), defaults.clone()).is_ok(),
        "pure preparation does not parse PEM"
    );
    for field in [
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
        match field {
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
        with_tls(&mut actual, tls.clone());
        assert!(candidate(actual.clone(), defaults.clone()).is_err(), "{field}");
        assert!(EtcdApply::prepare(Arc::new(actual.clone())).is_err(), "runner {field}");
        tls["enabled"] = false.into();
        with_tls(&mut actual, tls);
        assert!(candidate(actual.clone(), defaults.clone()).is_ok(), "inactive {field}");
    }
    let wrong = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"es"},"spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"]}}
    }))
    .unwrap();
    assert!(candidate(wrong, defaults).is_err());
}

#[test]
fn consumed_inherited_axes_change_identity_but_native_and_local_overrides_mask_globals() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsValidation};
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        validation: Some(OutboundTlsValidation {
            hostname: Some("changed.example".into()),
            well_known_ca_certificates: Some(
                edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System,
            ),
            ..Default::default()
        }),
        ..Default::default()
    });
    for enabled in [false, true] {
        let mut actual = etcd_ls(vec!["https://etcd.example:2379"], Some("42"));
        with_tls(&mut actual, serde_json::json!({"enabled":enabled}));
        let left = candidate(actual.clone(), a.clone()).unwrap();
        let right = candidate(actual.clone(), b.clone()).unwrap();
        assert_eq!(
            EtcdApply::fingerprint(&left)
                .unwrap()
                .rv_skip_allows_reuse(&EtcdApply::fingerprint(&right).unwrap()),
            !enabled
        );
        with_tls(
            &mut actual,
            serde_json::json!({"enabled":enabled,"verify":true,"validation":{}}),
        );
        let left = candidate(actual.clone(), a.clone()).unwrap();
        let right = candidate(actual, b.clone()).unwrap();
        assert!(EtcdApply::fingerprint(&left)
            .unwrap()
            .equal_runtime(&EtcdApply::fingerprint(&right).unwrap()));
    }
}

#[test]
fn actual_capture_survives_publication_and_connection_input_retains_owner_and_connector() {
    use edgion_common::link_sys_core::outbound_tls::{
        outbound_global_default, set_outbound_global_default, test_util, GlobalOutboundTlsSnapshot,
    };
    use edgion_resources::resources::common::OutboundTlsConfig;
    const CHILD: &str = "EDGION_ETCD_OWNED_CAPTURE_CHILD";
    const TEST: &str = "link_sys::providers::etcd::apply::tests::actual_capture_survives_publication_and_connection_input_retains_owner_and_connector";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, TEST)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
        return;
    }
    struct Restore(Arc<GlobalOutboundTlsSnapshot>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_outbound_global_default(self.0.clone());
        }
    }
    let _restore = Restore(outbound_global_default());
    let _ = rustls::crypto::ring::default_provider().install_default();
    set_outbound_global_default(test_util::snapshot(OutboundTlsConfig::default()));
    let mut actual = etcd_ls(vec!["https://etcd.example:2379"], Some("42"));
    with_tls(&mut actual, serde_json::json!({"enabled":true}));
    let actual = Arc::new(actual);
    let captured = EtcdApply::prepare(actual.clone()).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    let first = EtcdApply::build("fp/etcd", &captured).unwrap();
    let second = EtcdApply::build("fp/etcd", &captured).unwrap();
    assert!(EtcdApply::fingerprint_live(&first).equal_runtime(&EtcdApply::fingerprint_live(&second)));
    assert!(std::ptr::eq(captured.tls.owner(), actual.as_ref()));
    assert_eq!(first.endpoint_coverage(), second.endpoint_coverage());
    assert!(EtcdApply::prepare(actual).is_err());
}
