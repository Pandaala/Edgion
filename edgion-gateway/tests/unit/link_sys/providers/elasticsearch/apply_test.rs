use super::*;
use edgion_resources::resources::link_sys::elasticsearch::{ElasticsearchClientConfig, EsTimeout};
use edgion_resources::resources::link_sys::LinkSysSpec;

fn fingerprint_es(owner: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    fingerprint_es_fields(owner, |_| Ok(()))
}

#[test]
fn invalid_date_pattern_is_rejected_before_same_rv_reuse_or_construction() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    use edgion_resources::resources::link_sys::elasticsearch::EsIndex;
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let valid = es_ls(vec!["http://es.example:9200"], Some("42"));
    let admitted = candidate(valid.clone(), Arc::clone(&defaults)).unwrap();
    let ready = EsLinkClient::from_candidate("fp/es", &admitted).unwrap();
    assert!(ready.runtime_fingerprint().rv_skip_allows_reuse(admitted.fingerprint()));
    for pattern in ["%", "suffix%", "%Q", "%#z"] {
        let mut invalid = valid.clone();
        let SystemConfig::Elasticsearch(config) = &mut invalid.spec.config else {
            unreachable!()
        };
        config.index = Some(EsIndex {
            prefix: None,
            date_pattern: Some(pattern.into()),
        });
        assert_eq!(
            config.validate_date_pattern().unwrap_err(),
            invalid.validate_config()[0]
        );
        let error = candidate(invalid.clone(), Arc::clone(&defaults)).err().unwrap();
        assert!(error.to_string().contains("index.datePattern"));
        assert!(EsApply::prepare(Arc::new(invalid.clone())).is_err());
        assert!(EsLinkClient::from_link_sys(Arc::new(invalid)).is_err());
    }
}

#[test]
fn admitted_date_patterns_preserve_effective_fingerprint_identity() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    use edgion_resources::resources::link_sys::elasticsearch::EsIndex;
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let base = es_ls(vec!["http://es.example:9200"], None);
    let baseline = candidate(base.clone(), Arc::clone(&defaults)).unwrap();
    for pattern in ["", "literal", "%%", "%Y.%m.%d", "%Z %:z"] {
        let mut explicit = base.clone();
        let SystemConfig::Elasticsearch(config) = &mut explicit.spec.config else {
            unreachable!()
        };
        config.index = Some(EsIndex {
            prefix: None,
            date_pattern: Some(pattern.into()),
        });
        let incoming = candidate(explicit, Arc::clone(&defaults)).unwrap();
        assert_eq!(
            baseline.fingerprint().equal_runtime(incoming.fingerprint()),
            pattern == "%Y.%m.%d"
        );
        let ready = EsLinkClient::from_candidate("fp/es", &incoming).unwrap();
        assert!(ready.runtime_fingerprint().equal_runtime(incoming.fingerprint()));
    }
}

fn candidate(
    owner: LinkSys,
    defaults: Arc<edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot>,
) -> anyhow::Result<EsCandidate> {
    EsCandidate::from_prepared(PreparedLinkSysTls::with_snapshot(Arc::new(owner), defaults)?)
}

#[test]
fn actual_source_and_whole_active_pairing_precede_same_rv_and_zero_live_planning() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let valid = es_ls(vec!["https://es.example:9200"], Some("42"));
    for tls in [
        None,
        Some(serde_json::json!({"enabled":false})),
        Some(serde_json::json!({
            "enabled":true,"verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"fp"},"data":{"ca.crt":"bm90LXBlbQ=="}}]
        })),
    ] {
        let mut actual = valid.clone();
        let SystemConfig::Elasticsearch(config) = &mut actual.spec.config else {
            unreachable!()
        };
        config.tls = tls.map(|tls| serde_json::from_value(tls).unwrap());
        let incoming = candidate(actual.clone(), Arc::clone(&defaults)).unwrap();
        let ready = EsLinkClient::from_candidate("fp/es", &incoming).unwrap();
        assert!(ready.runtime_fingerprint().rv_skip_allows_reuse(incoming.fingerprint()));
        for bad_ns in ["", "Bad"] {
            let mut invalid = actual.clone();
            invalid.metadata.namespace = Some(bad_ns.into());
            assert!(candidate(invalid, Arc::clone(&defaults)).is_err());
        }
        if tls_is_active(&actual) {
            for field in ["group", "missing", "namespace", "name"] {
                let mut invalid = actual.clone();
                let SystemConfig::Elasticsearch(config) = &mut invalid.spec.config else {
                    unreachable!()
                };
                let tls = config.tls.as_mut().unwrap();
                match field {
                    "group" => {
                        tls.outbound
                            .validation
                            .as_mut()
                            .unwrap()
                            .ca_certificate_refs
                            .as_mut()
                            .unwrap()[0]
                            .group = Some("foreign.example".into())
                    }
                    "missing" => tls.resolved_ca_certificates = None,
                    "namespace" => {
                        tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
                    }
                    _ => tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("wrong".into()),
                }
                assert!(candidate(invalid.clone(), Arc::clone(&defaults)).is_err(), "{field}");
                assert!(EsApply::prepare(Arc::new(invalid)).is_err(), "runner {field}");
            }
        }
    }
    let wrong: LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"redis"},"spec":{"type":"redis","config":{"endpoints":["redis://redis.example:6379"]}}
    }))
    .unwrap();
    assert!(candidate(wrong, defaults).is_err());
}

fn tls_is_active(owner: &LinkSys) -> bool {
    matches!(&owner.spec.config, SystemConfig::Elasticsearch(config) if config.tls.as_ref().is_some_and(|tls| tls.enabled))
}

#[test]
fn consumed_inherited_axes_block_same_rv_reuse_but_native_and_masked_axes_do_not() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    for enabled in [false, true] {
        let mut actual = es_ls(vec!["https://es.example:9200"], Some("42"));
        let SystemConfig::Elasticsearch(config) = &mut actual.spec.config else {
            unreachable!()
        };
        config.tls = Some(serde_json::from_value(serde_json::json!({"enabled":enabled})).unwrap());
        let left = candidate(actual.clone(), Arc::clone(&a)).unwrap();
        let right = candidate(actual.clone(), Arc::clone(&b)).unwrap();
        assert_eq!(left.fingerprint().rv_skip_allows_reuse(right.fingerprint()), !enabled);
        if enabled {
            let SystemConfig::Elasticsearch(config) = &mut actual.spec.config else {
                unreachable!()
            };
            config.tls.as_mut().unwrap().outbound.verify = Some(true);
            assert!(candidate(actual.clone(), Arc::clone(&a))
                .unwrap()
                .fingerprint()
                .equal_runtime(candidate(actual, Arc::clone(&b)).unwrap().fingerprint()));
        }
    }
}

#[test]
fn actual_capture_survives_publication_and_repeated_client_construction() {
    use edgion_common::link_sys_core::outbound_tls::{
        outbound_global_default, set_outbound_global_default, test_util, GlobalOutboundTlsSnapshot,
    };
    use edgion_resources::resources::common::OutboundTlsConfig;
    const CHILD: &str = "EDGION_ES_OWNED_CAPTURE_CHILD";
    const TEST: &str = "link_sys::providers::elasticsearch::apply::tests::actual_capture_survives_publication_and_repeated_client_construction";
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
            set_outbound_global_default(Arc::clone(&self.0));
        }
    }
    let _restore = Restore(outbound_global_default());
    let a = test_util::snapshot(OutboundTlsConfig::default());
    set_outbound_global_default(Arc::clone(&a));
    let mut actual = es_ls(vec!["https://es.example:9200"], Some("42"));
    let SystemConfig::Elasticsearch(config) = &mut actual.spec.config else {
        unreachable!()
    };
    config.tls = Some(serde_json::from_value(serde_json::json!({"enabled":true})).unwrap());
    let actual = Arc::new(actual);
    let captured = EsApply::prepare(Arc::clone(&actual)).unwrap();
    let bad: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"bad","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    set_outbound_global_default(Arc::new(GlobalOutboundTlsSnapshot::from_configs([Arc::new(bad)])));
    let first = EsApply::build("fp/es", &captured).unwrap();
    let second = EsApply::build("fp/es", &captured).unwrap();
    assert!(first.runtime_fingerprint().equal_runtime(second.runtime_fingerprint()));
    assert!(EsApply::prepare(actual).is_err());
}

#[test]
fn prepare_and_fingerprint_are_pure_while_construction_retains_skip_verify_audit() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct Audits(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> Layer<S> for Audits {
        fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            if event.metadata().target() == "edgion_common::link_sys_core::outbound_tls" {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(Audits(Arc::clone(&count)));
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut actual = es_ls(vec!["https://es.example:9200"], Some("42"));
    let SystemConfig::Elasticsearch(config) = &mut actual.spec.config else {
        unreachable!()
    };
    config.tls = Some(
        serde_json::from_value(serde_json::json!({"enabled":true,"verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"fp"},"data":{"ca.crt":"bm90LXBlbQ=="}}]
        }))
        .unwrap(),
    );
    let captured = candidate(actual, Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))).unwrap();
    for _ in 0..3 {
        let _ = EsApply::fingerprint(&captured).unwrap();
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    let ready = EsApply::build("fp/es", &captured).unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(ready.connection_coverage().live, 0);
}

fn es_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("es".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                endpoints: endpoints.into_iter().map(str::to_string).collect(),
                auth: None,
                tls: None,
                timeout: None,
                pool: None,
                bulk: None,
                index: None,
            }),
        },
        status: None,
    }
}

#[test]
fn endpoint_order_does_not_change_fingerprint() {
    let left = es_ls(vec!["http://b.example:9200", "http://a.example:9200"], None);
    let right = es_ls(vec!["http://a.example:9200", "http://b.example:9200"], None);
    assert!(fingerprint_es(&left)
        .unwrap()
        .equal_runtime(&fingerprint_es(&right).unwrap()));
}

#[test]
fn defaulted_timeouts_match_explicit_defaults() {
    let base = es_ls(vec!["http://a.example:9200"], None);
    let mut explicit = base.clone();
    if let SystemConfig::Elasticsearch(cfg) = &mut explicit.spec.config {
        cfg.timeout = Some(EsTimeout {
            connect: Some(5_000),
            request: Some(30_000),
        });
    }
    assert!(fingerprint_es(&base)
        .unwrap()
        .equal_runtime(&fingerprint_es(&explicit).unwrap()));
}

#[test]
fn timeout_clamp_does_not_change_fingerprint() {
    let mut a = es_ls(vec!["http://a.example:9200"], None);
    let mut b = a.clone();
    if let SystemConfig::Elasticsearch(cfg) = &mut a.spec.config {
        cfg.timeout = Some(EsTimeout {
            connect: Some(30_000),
            request: Some(120_000),
        });
    }
    if let SystemConfig::Elasticsearch(cfg) = &mut b.spec.config {
        cfg.timeout = Some(EsTimeout {
            connect: Some(30_001),
            request: Some(120_001),
        });
    }
    assert!(fingerprint_es(&a).unwrap().equal_runtime(&fingerprint_es(&b).unwrap()));
}

#[test]
fn bulk_response_limit_fingerprint_uses_effective_runtime_value() {
    fn with_cap(cap: Option<usize>) -> LinkSys {
        let mut owner = es_ls(vec!["http://es.example:9200"], None);
        let SystemConfig::Elasticsearch(config) = &mut owner.spec.config else {
            unreachable!()
        };
        config.bulk = Some(serde_json::from_value(serde_json::json!({"maxResponseBytes": cap})).unwrap());
        owner
    }
    let absent = fingerprint_es(&with_cap(None)).unwrap();
    let default = fingerprint_es(&with_cap(Some(1024 * 1024))).unwrap();
    assert!(absent.equal_runtime(&default));
    let changed = fingerprint_es(&with_cap(Some(2 * 1024 * 1024))).unwrap();
    assert!(!absent.equal_runtime(&changed));
    let maximum = fingerprint_es(&with_cap(Some(8 * 1024 * 1024))).unwrap();
    let clamped = fingerprint_es(&with_cap(Some(usize::MAX))).unwrap();
    assert!(maximum.equal_runtime(&clamped));
}
