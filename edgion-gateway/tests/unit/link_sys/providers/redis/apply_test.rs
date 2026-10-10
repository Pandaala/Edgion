use super::*;
use edgion_resources::resources::common::outbound_tls::OutboundTlsLocal;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::redis::{
    RedisAuth, RedisCluster, RedisPool, RedisSentinel, RedisTimeout, RedisTopology,
};
use edgion_resources::resources::link_sys::LinkSysSpec;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

// These historical cases isolate the existing local encoding. Owning replay
// tests below exercise proof and inherited identity before the runner decisions.
fn fingerprint_redis(owner: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    fingerprint_redis_fields(owner, |_| {})
}

#[test]
fn actual_redis_candidate_rejects_invalid_same_rv_source_and_pairs_before_planning() {
    use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
    let defaults = Arc::new(GlobalOutboundTlsSnapshot::from_configs([]));
    let mut valid = redis_ls(vec!["redis://redis.example:6379"], Some("42"));
    let SystemConfig::Redis(config) = &mut valid.spec.config else {
        unreachable!()
    };
    config.tls = Some(
        serde_json::from_value(serde_json::json!({
            "enabled":true,"verify":true,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"fp"}}]
        }))
        .unwrap(),
    );
    let prepared = PreparedRedisTls::with_snapshot(Arc::new(valid.clone()), Arc::clone(&defaults)).unwrap();
    let candidate = RedisCandidate::from_prepared(prepared).unwrap();
    let live = candidate.fingerprint().clone();
    assert!(matches!(
        crate::link_sys::runtime::apply::plan_apply(
            Some(&live),
            candidate.fingerprint(),
            ConnectionCoverage { live: 1, configured: 1 },
            false
        ),
        crate::link_sys::runtime::apply::ApplyPlan::Reuse
    ));
    for field in ["source", "foreign-group", "missing", "namespace"] {
        let mut bad = valid.clone();
        if field == "source" {
            bad.metadata.name = None;
        } else {
            let SystemConfig::Redis(config) = &mut bad.spec.config else {
                unreachable!()
            };
            let tls = config.tls.as_mut().unwrap();
            match field {
                "foreign-group" => {
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
                _ => tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into()),
            }
        }
        assert!(
            PreparedRedisTls::with_snapshot(Arc::new(bad.clone()), Arc::clone(&defaults)).is_err(),
            "{field}"
        );
        assert!(RedisApply::prepare(Arc::new(bad)).is_err(), "runner {field}");
    }
}

#[test]
fn consumed_global_changes_invalidate_same_rv_redis_reuse_and_masks_preserve_identity() {
    use edgion_common::link_sys_core::outbound_tls::test_util;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let a = test_util::snapshot(OutboundTlsConfig::default());
    let b = test_util::snapshot(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    });
    let valid = Arc::new(redis_ls(vec!["rediss://redis.example:6380"], Some("42")));
    let left =
        RedisCandidate::from_prepared(PreparedRedisTls::with_snapshot(Arc::clone(&valid), Arc::clone(&a)).unwrap())
            .unwrap();
    let right = RedisCandidate::from_prepared(PreparedRedisTls::with_snapshot(valid, Arc::clone(&b)).unwrap()).unwrap();
    assert!(!left.fingerprint().equal_runtime(right.fingerprint()));
    assert!(!left.fingerprint().rv_skip_allows_reuse(right.fingerprint()));
    let mut masked = redis_ls(vec!["rediss://redis.example:6380"], Some("42"));
    let SystemConfig::Redis(config) = &mut masked.spec.config else {
        unreachable!()
    };
    config.tls = Some(TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal {
            verify: Some(true),
            ..Default::default()
        },
        ..Default::default()
    });
    let masked = Arc::new(masked);
    let left = RedisCandidate::from_prepared(PreparedRedisTls::with_snapshot(Arc::clone(&masked), a).unwrap()).unwrap();
    let right = RedisCandidate::from_prepared(PreparedRedisTls::with_snapshot(masked, b).unwrap()).unwrap();
    assert!(left.fingerprint().equal_runtime(right.fingerprint()));
}

fn redis_ls(endpoints: Vec<&str>, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("redis".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: endpoints.into_iter().map(str::to_string).collect(),
                auth: None,
                db: None,
                timeout: None,
                pool: None,
                topology: None,
                tls: None,
            }),
        },
        status: None,
    }
}

fn with_auth(mut ls: LinkSys, username: Option<&str>, password: &[u8]) -> LinkSys {
    let SystemConfig::Redis(cfg) = &mut ls.spec.config else {
        panic!("redis");
    };
    let mut data = BTreeMap::new();
    if let Some(username) = username {
        data.insert("username".to_string(), ByteString(username.as_bytes().to_vec()));
    }
    data.insert("password".to_string(), ByteString(password.to_vec()));
    cfg.auth = Some(RedisAuth {
        slot: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "creds".to_string(),
                namespace: None,
            },
            secret: Some(k8s_openapi::api::core::v1::Secret {
                metadata: kube::api::ObjectMeta {
                    name: Some("creds".into()),
                    namespace: Some("fp".into()),
                    ..Default::default()
                },
                data: Some(data),
                ..Default::default()
            }),
        },
    });
    ls
}

fn with_password(ls: LinkSys, password: &[u8]) -> LinkSys {
    with_auth(ls, None, password)
}

fn with_tls(mut ls: LinkSys, enabled: bool, verify: Option<bool>, ca: Option<&[u8]>) -> LinkSys {
    let SystemConfig::Redis(cfg) = &mut ls.spec.config else {
        panic!("redis");
    };
    cfg.tls = Some(TlsConfig {
        enabled,
        outbound: OutboundTlsLocal {
            verify,
            validation: None,
            client_certificate_ref: None,
        },
        resolved_ca_certificates: ca.map(|bytes| {
            let mut data = BTreeMap::new();
            data.insert("ca.crt".to_string(), ByteString(bytes.to_vec()));
            vec![k8s_openapi::api::core::v1::Secret {
                data: Some(data),
                ..Default::default()
            }]
        }),
        resolved_client_certificate: None,
    });
    ls
}

#[test]
fn endpoint_order_does_not_change_cluster_fingerprint() {
    let mut left = redis_ls(vec!["redis://b.example:6379", "redis://a.example:6379"], None);
    let mut right = redis_ls(vec!["redis://a.example:6379", "redis://b.example:6379"], None);
    for ls in [&mut left, &mut right] {
        if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
            cfg.topology = Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster { max_redirects: None }),
            });
        }
    }
    assert!(fingerprint_redis(&left)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&right).unwrap()));
}

#[test]
fn rediss_transport_differs_from_redis() {
    let plain = redis_ls(vec!["redis://a.example:6379"], None);
    let tls = redis_ls(vec!["rediss://a.example:6379"], None);
    assert!(!fingerprint_redis(&plain)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&tls).unwrap()));
}

#[test]
fn pool_size_is_rebuild() {
    let mut small = redis_ls(vec!["redis://a.example:6379"], None);
    let mut large = small.clone();
    if let SystemConfig::Redis(cfg) = &mut small.spec.config {
        cfg.pool = Some(RedisPool { size: Some(2) });
    }
    if let SystemConfig::Redis(cfg) = &mut large.spec.config {
        cfg.pool = Some(RedisPool { size: Some(8) });
    }
    assert!(!fingerprint_redis(&small)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&large).unwrap()));
}

#[test]
fn default_pool_matches_explicit_eight() {
    let implicit = redis_ls(vec!["redis://a.example:6379"], None);
    let mut explicit = implicit.clone();
    if let SystemConfig::Redis(cfg) = &mut explicit.spec.config {
        cfg.pool = Some(RedisPool {
            size: Some(REDIS_DEFAULT_POOL_SIZE),
        });
    }
    assert!(fingerprint_redis(&implicit)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&explicit).unwrap()));
}

#[test]
fn secret_bytes_change_secret_hash() {
    let a = with_password(redis_ls(vec!["redis://a.example:6379"], Some("10")), b"pw-a");
    let b = with_password(redis_ls(vec!["redis://a.example:6379"], Some("10")), b"pw-b");
    let fa = fingerprint_redis(&a).unwrap();
    let fb = fingerprint_redis(&b).unwrap();
    assert!(!fa.equal_runtime(&fb));
    assert!(!fa.rv_skip_allows_reuse(&fb));
}

#[test]
fn resource_version_is_not_a_rebuild_field() {
    let a = redis_ls(vec!["redis://a.example:6379"], Some("1"));
    let b = redis_ls(vec!["redis://a.example:6379"], Some("2"));
    assert!(fingerprint_redis(&a)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&b).unwrap()));
}

#[test]
fn sync_version_annotation_is_ignored() {
    let mut a = redis_ls(vec!["redis://a.example:6379"], Some("1"));
    let mut b = a.clone();
    a.metadata.annotations = Some(
        [("edgion.io/sync-version".to_string(), "aaa".to_string())]
            .into_iter()
            .collect(),
    );
    b.metadata.annotations = Some(
        [("edgion.io/sync-version".to_string(), "bbb".to_string())]
            .into_iter()
            .collect(),
    );
    assert!(fingerprint_redis(&a)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&b).unwrap()));
}

#[test]
fn db_and_timeouts_are_rebuild() {
    let base = redis_ls(vec!["redis://a.example:6379"], None);
    let mut db = base.clone();
    let mut timeout = base.clone();
    if let SystemConfig::Redis(cfg) = &mut db.spec.config {
        cfg.db = Some(1);
    }
    if let SystemConfig::Redis(cfg) = &mut timeout.spec.config {
        cfg.timeout = Some(RedisTimeout {
            connect: Some("3s".to_string()),
            command: Some("5s".to_string()),
        });
    }
    assert!(!fingerprint_redis(&base)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&db).unwrap()));
    assert!(!fingerprint_redis(&base)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&timeout).unwrap()));
}

#[test]
fn username_is_part_of_secret_fingerprint() {
    let a = with_auth(
        redis_ls(vec!["redis://a.example:6379"], Some("10")),
        Some("alice"),
        b"pw",
    );
    let b = with_auth(redis_ls(vec!["redis://a.example:6379"], Some("10")), Some("bob"), b"pw");
    assert!(!fingerprint_redis(&a)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&b).unwrap()));
}

#[test]
fn tls_verify_is_rebuild_when_enabled() {
    let base = redis_ls(vec!["rediss://a.example:6379"], None);
    let verify = with_tls(base.clone(), true, Some(true), None);
    let skip = with_tls(base, true, Some(false), None);
    assert!(!fingerprint_redis(&verify)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&skip).unwrap()));
}

#[test]
fn disabled_tls_ignores_unused_ca_and_verify() {
    let base = redis_ls(vec!["redis://a.example:6379"], None);
    let left = with_tls(base.clone(), false, Some(true), Some(b"ca-a"));
    let right = with_tls(base, false, Some(false), Some(b"ca-b"));
    assert!(fingerprint_redis(&left)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&right).unwrap()));
}

#[test]
fn sentinel_master_name_is_rebuild() {
    fn sentinel_ls(master: &str) -> LinkSys {
        let mut ls = redis_ls(vec![], None);
        if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
            cfg.topology = Some(RedisTopology {
                mode: RedisTopologyMode::Sentinel,
                sentinel: Some(RedisSentinel {
                    master_name: master.to_string(),
                    sentinels: vec!["redis://sentinel.example:26379".to_string()],
                }),
                cluster: None,
            });
        }
        ls
    }
    assert!(!fingerprint_redis(&sentinel_ls("mymaster"))
        .unwrap()
        .equal_runtime(&fingerprint_redis(&sentinel_ls("other")).unwrap()));
}

#[test]
fn cluster_max_redirects_is_rebuild() {
    let mut left = redis_ls(vec!["redis://a.example:6379"], None);
    let mut right = left.clone();
    for (ls, redirects) in [(&mut left, Some(5)), (&mut right, Some(8))] {
        if let SystemConfig::Redis(cfg) = &mut ls.spec.config {
            cfg.topology = Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster {
                    max_redirects: redirects,
                }),
            });
        }
    }
    assert!(!fingerprint_redis(&left)
        .unwrap()
        .equal_runtime(&fingerprint_redis(&right).unwrap()));
}
