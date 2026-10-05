use super::*;

#[test]
fn invalid_etcd_policy_never_builds_a_fresh_gateway_client() {
    for policy in [
        serde_json::json!({"rejectOldCluster": true}),
        serde_json::json!({"keepAlive": {"time": 1}}),
        serde_json::json!({"observability": {"logging": {"level": "info"}}}),
    ] {
        let mut config = policy;
        config["endpoints"] = serde_json::json!(["http://etcd.example:2379"]);
        let resource: LinkSys = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "LinkSys",
            "metadata": {"name": "etcd-policy", "namespace": "default"},
            "spec": {"type": "etcd", "config": config}
        }))
        .unwrap();
        assert!(EtcdApply::fingerprint(&resource).is_err());
        assert!(EtcdApply::build("default/etcd-policy", &resource).is_err());
    }
}
use edgion_resources::resources::link_sys::etcd::{EtcdKeepAlive, EtcdObservability, EtcdTimeout};
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
                auto_sync_interval: None,
                max_call_send_size: None,
                max_call_recv_size: None,
                user_agent: None,
                reject_old_cluster: None,
                min_cluster_version: None,
                observability: None,
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
fn supported_fields_rebuild_and_invalid_policy_is_rejected() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let original = fingerprint_etcd(&base).unwrap();
    for field in [
        "autoSyncInterval",
        "maxCallSendSize",
        "maxCallRecvSize",
        "userAgent",
        "observability",
    ] {
        let mut changed = base.clone();
        if let SystemConfig::Etcd(cfg) = &mut changed.spec.config {
            match field {
                "autoSyncInterval" => cfg.auto_sync_interval = Some(30),
                "maxCallSendSize" => cfg.max_call_send_size = Some(1024),
                "maxCallRecvSize" => cfg.max_call_recv_size = Some(1024),
                "userAgent" => cfg.user_agent = Some("edgion-test".to_owned()),
                _ => {
                    cfg.observability = Some(EtcdObservability {
                        metrics: Some(edgion_resources::resources::link_sys::etcd::EtcdMetrics {
                            enabled: true,
                            labels: None,
                        }),
                        logging: None,
                    })
                }
            }
        }
        assert!(!original.equal_runtime(&fingerprint_etcd(&changed).unwrap()), "{field}");
    }
    let mut invalid = base;
    if let SystemConfig::Etcd(cfg) = &mut invalid.spec.config {
        cfg.reject_old_cluster = Some(true);
    }
    assert!(fingerprint_etcd(&invalid).is_err());
}

#[test]
fn effective_defaults_and_normalized_endpoints_are_equivalent() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let mut equivalent = etcd_ls(vec!["http://A.EXAMPLE:2379/", "http://a.example:2379"], None);
    if let SystemConfig::Etcd(cfg) = &mut equivalent.spec.config {
        cfg.namespace = Some(String::new());
        cfg.auto_sync_interval = Some(0);
        cfg.max_call_recv_size = Some(4 * 1024 * 1024);
        cfg.reject_old_cluster = Some(false);
    }
    assert!(fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&equivalent).unwrap()));
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
fn version_and_logging_runtime_changes_rebuild() {
    let base = etcd_ls(vec!["http://a.example:2379"], None);
    let mut versioned = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut versioned.spec.config {
        cfg.reject_old_cluster = Some(true);
        cfg.min_cluster_version = Some("3.5.0".to_owned());
    }
    assert!(!fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&versioned).unwrap()));
    let mut logged = base.clone();
    if let SystemConfig::Etcd(cfg) = &mut logged.spec.config {
        cfg.observability = Some(serde_json::from_value(serde_json::json!({"logging":{"enabled":true}})).unwrap());
    }
    assert!(!fingerprint_etcd(&base)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&logged).unwrap()));
    let mut explicit_info = logged.clone();
    if let SystemConfig::Etcd(cfg) = &mut explicit_info.spec.config {
        cfg.observability.as_mut().unwrap().logging.as_mut().unwrap().level = Some("info".to_owned());
    }
    assert!(fingerprint_etcd(&logged)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&explicit_info).unwrap()));
    if let SystemConfig::Etcd(cfg) = &mut explicit_info.spec.config {
        cfg.observability
            .as_mut()
            .unwrap()
            .logging
            .as_mut()
            .unwrap()
            .slow_log_threshold = Some(100);
    }
    assert!(!fingerprint_etcd(&logged)
        .unwrap()
        .equal_runtime(&fingerprint_etcd(&explicit_info).unwrap()));
}
