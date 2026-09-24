use super::*;
use edgion_resources::resources::link_sys::elasticsearch::{ElasticsearchClientConfig, EsTimeout};
use edgion_resources::resources::link_sys::LinkSysSpec;

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
