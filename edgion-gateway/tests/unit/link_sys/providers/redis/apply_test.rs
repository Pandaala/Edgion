use super::*;
use edgion_resources::resources::common::outbound_tls::OutboundTlsLocal;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::redis::{
    RedisAuth, RedisCluster, RedisPool, RedisSentinel, RedisTimeout, RedisTopology,
};
use edgion_resources::resources::link_sys::LinkSysSpec;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

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
