use super::*;
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
