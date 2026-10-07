use super::*;
use crate::link_sys::runtime::runner::ApplyOwner;
use crate::plugins::http::common::outbound::target::ResolvedTarget;
use std::sync::atomic::AtomicU64;

#[derive(Clone, Copy)]
enum InjectedSecret {
    Missing,
    Valid,
    MalformedForInvalid,
}

fn create_test_webhook_linksys(namespace: &str, name: &str) -> LinkSys {
    create_test_webhook_linksys_with_port(namespace, name, 8080)
}

fn create_test_webhook_linksys_with_port(namespace: &str, name: &str, port: i32) -> LinkSys {
    use edgion_resources::resources::common::{ExternalHttpConnection, WebhookServiceRef, WebhookTarget};
    use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            kind: None,
                            name: Some("resolve-svc".to_string()),
                            namespace: Some(namespace.to_string()),
                            port: Some(port),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        status: None,
    }
}

fn create_redis_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let secret = match inject {
        InjectedSecret::Missing => None,
        InjectedSecret::Valid => {
            let mut data = BTreeMap::new();
            data.insert("password".to_string(), ByteString(b"pw".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
        InjectedSecret::MalformedForInvalid => {
            let mut data = BTreeMap::new();
            data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
    };

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://127.0.0.1:6379".to_string()],
                auth: Some(RedisAuth {
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "creds".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                }),
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

fn create_etcd_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::etcd::{EtcdAuth, EtcdClientConfig};
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let secret = match inject {
        InjectedSecret::Missing => None,
        InjectedSecret::Valid => {
            let mut data = BTreeMap::new();
            data.insert("password".to_string(), ByteString(b"pw".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
        InjectedSecret::MalformedForInvalid => {
            let mut data = BTreeMap::new();
            data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
    };

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Etcd(EtcdClientConfig {
                endpoints: vec!["http://127.0.0.1:2379".to_string()],
                auth: Some(EtcdAuth {
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "creds".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                }),
                tls: None,
                timeout: None,
                keep_alive: None,
                namespace: None,
            }),
        },
        status: None,
    }
}

fn create_es_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::elasticsearch::{ElasticsearchAuth, ElasticsearchClientConfig};
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let secret = match inject {
        InjectedSecret::Missing => None,
        InjectedSecret::Valid => {
            let mut data = BTreeMap::new();
            data.insert("password".to_string(), ByteString(b"pw".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
        InjectedSecret::MalformedForInvalid => {
            let mut data = BTreeMap::new();
            data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
    };

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                endpoints: vec!["http://127.0.0.1:9200".to_string()],
                auth: Some(ElasticsearchAuth::Basic {
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "creds".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                }),
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

fn create_kafka_linksys_sasl(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::kafka::{KafkaClientConfig, KafkaSaslConfig};
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let secret = match inject {
        InjectedSecret::Missing => None,
        InjectedSecret::Valid => {
            let mut data = BTreeMap::new();
            data.insert("password".to_string(), ByteString(b"pw".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
        InjectedSecret::MalformedForInvalid => {
            let mut data = BTreeMap::new();
            data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
            Some(Secret {
                data: Some(data),
                ..Default::default()
            })
        }
    };

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Kafka(KafkaClientConfig {
                brokers: vec!["127.0.0.1:9092".to_string()],
                sasl: Some(KafkaSaslConfig {
                    username: "alice".to_string(),
                    password: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "creds".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                }),
                tls: None,
                channel_size: None,
                max_topics: None,
                max_pending_records: None,
                max_pending_bytes: None,
                linger_ms: None,
            }),
        },
        status: None,
    }
}

fn create_otlp_linksys(namespace: &str, name: &str, auth: InjectedSecret) -> LinkSys {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpClientConfig};
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let secret = match auth {
        InjectedSecret::Missing => None,
        InjectedSecret::Valid => Some(Secret {
            data: Some(BTreeMap::from([(
                "bearerToken".to_string(),
                ByteString(b"token".to_vec()),
            )])),
            ..Default::default()
        }),
        InjectedSecret::MalformedForInvalid => Some(Secret {
            data: Some(BTreeMap::from([("wrong".to_string(), ByteString(b"value".to_vec()))])),
            ..Default::default()
        }),
    };

    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Otlp(OtlpClientConfig {
                endpoint: "http://192.0.2.1:4317".to_string(),
                timeout_ms: 10_000,
                auth: Some(OtlpAuth {
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "auth".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                }),
                tls: None,
            }),
        },
        status: None,
    }
}

#[test]
fn webhook_with_unresolved_secret_ref_is_not_ready() {
    use edgion_resources::resources::common::{CustomField, CustomSource, ListFields, OutboundRequestSpec};
    use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
    let mut config = WebhookServiceConfig::default();
    config.conn.target.service.name = Some("svc".to_string());
    config.conn.target.service.port = Some(8080);
    config.request = OutboundRequestSpec {
        headers: ListFields {
            origin: vec![],
            custom: vec![CustomField {
                name: "Authorization".to_string(),
                source: CustomSource::Template("Bearer ${secretRef:ns/tok:token}".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    // resolved_secrets is None → the ref is unresolved.
    assert!(!config.unresolved_secret_refs().is_empty());
    assert!(
        !webhook_secrets_ready(&config),
        "must be not-ready when refs unresolved"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn redis_relist_and_pending_update_preserve_published_generation() {
    let registry = RuntimeRegistry::new();
    let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
    let candidate = RedisApply::prepare(Arc::new(resource.clone())).unwrap();
    let client = RedisApply::build("test/link", &candidate).unwrap();
    let runner = PersistentRunner::<RedisApply>::spawn_with_published("test/link".to_string(), client.clone());
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Redis(runner.clone()))),
    );
    let old = registry.get("test/link").unwrap();
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    let pending = one(create_redis_linksys("test", "link", InjectedSecret::Missing));
    registry.apply(&pending, &HashSet::new(), &pending, false, false);
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    drop(registry);
    assert!(runner.current().is_none());
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn etcd_relist_and_pending_update_preserve_published_generation() {
    let registry = RuntimeRegistry::new();
    let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
    let client = EtcdApply::build("test/link", &EtcdApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client.clone());
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
    );
    let old = registry.get("test/link").unwrap();
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    let pending = one(create_etcd_linksys("test", "link", InjectedSecret::Missing));
    registry.apply(&pending, &HashSet::new(), &pending, false, false);
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    drop(registry);
    assert!(runner.current().is_none());
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn es_relist_and_pending_update_preserve_published_generation() {
    let registry = RuntimeRegistry::new();
    let resource = create_es_linksys("test", "link", InjectedSecret::Valid);
    let client = EsApply::build("test/link", &EsApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<EsApply>::spawn_with_published("test/link".to_string(), client.clone());
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Elasticsearch(runner.clone()))),
    );
    let old = registry.get("test/link").unwrap();
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    let pending = one(create_es_linksys("test", "link", InjectedSecret::Missing));
    registry.apply(&pending, &HashSet::new(), &pending, false, false);
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    drop(registry);
    assert!(runner.current().is_none());
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn kafka_relist_and_pending_update_preserve_published_generation() {
    let registry = RuntimeRegistry::new();
    let resource = create_kafka_linksys_sasl("test", "link", InjectedSecret::Valid);
    let client = KafkaApply::build("test/link", &KafkaApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<KafkaApply>::spawn_with_published("test/link".to_string(), client.clone());
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Kafka(runner.clone()))),
    );
    let old = registry.get("test/link").unwrap();
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    let pending = one(create_kafka_linksys_sasl("test", "link", InjectedSecret::Missing));
    registry.apply(&pending, &HashSet::new(), &pending, false, false);
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    drop(registry);
    assert!(runner.current().is_none());
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn otlp_relist_and_pending_update_preserve_published_generation() {
    let registry = RuntimeRegistry::new();
    let resource = create_otlp_linksys("test", "link", InjectedSecret::Valid);
    let client = OtlpApply::build("test/link", &OtlpApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<OtlpApply>::spawn_with_published("test/link".to_string(), client.clone());
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Otlp(runner.clone()))),
    );
    let old = registry.get("test/link").unwrap();
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    let pending = one(create_otlp_linksys("test", "link", InjectedSecret::Missing));
    registry.apply(&pending, &HashSet::new(), &pending, false, false);
    assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
    drop(registry);
    assert!(runner.current().is_none());
    // OTLP stop owns its exit-drain join; registry deletion itself never waits.
}

#[tokio::test(flavor = "current_thread")]
async fn old_probe_result_cannot_overwrite_published_health_gauge() {
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    let registry = RuntimeRegistry::new();
    let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
    let old_client = EtcdApply::build("test/link", &EtcdApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let old = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), old_client.clone());
    registry.install("test/link", Some(RuntimeEntry::new(ProviderRuntime::Etcd(old.clone()))));
    let new_client = EtcdApply::build("test/link", &EtcdApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let new = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), new_client);
    registry.install("test/link", Some(RuntimeEntry::new(ProviderRuntime::Etcd(new.clone()))));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || {
        record_registry_health(&registry);
        let health = EtcdApply::published_health(&old_client).unwrap();
        crate::link_sys::runtime::health_probe::apply_health_result(
            health,
            "test/link",
            "etcd",
            &AtomicU64::new(0),
            true,
            false,
        );
    });
    assert!(snapshotter.snapshot().into_vec().iter().any(|(key, _, _, value)| {
        key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
            && matches!(value, DebugValue::Gauge(level) if *level == 0.0)
    }));
    drop(registry);
    old.await_join().await;
    new.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn typed_getters_only_return_the_current_provider() {
    let _guard = I5_DISPATCH_LOCK.lock().await;
    let key = "test/link";
    let resources = resources(InjectedSecret::Valid);
    let entries = vec![
        RuntimeEntry::new(ProviderRuntime::Redis(
            PersistentRunner::<RedisApply>::spawn_with_published(
                key.to_string(),
                RedisApply::build(key, &RedisApply::prepare(Arc::new(resources[0].clone())).unwrap()).unwrap(),
            ),
        )),
        RuntimeEntry::new(ProviderRuntime::Etcd(
            PersistentRunner::<EtcdApply>::spawn_with_published(
                key.to_string(),
                EtcdApply::build(key, &EtcdApply::prepare(Arc::new(resources[1].clone())).unwrap()).unwrap(),
            ),
        )),
        RuntimeEntry::new(ProviderRuntime::Elasticsearch(
            PersistentRunner::<EsApply>::spawn_with_published(
                key.to_string(),
                EsApply::build(key, &EsApply::prepare(Arc::new(resources[2].clone())).unwrap()).unwrap(),
            ),
        )),
        RuntimeEntry::new(ProviderRuntime::Kafka(
            PersistentRunner::<KafkaApply>::spawn_with_published(
                key.to_string(),
                KafkaApply::build(key, &KafkaApply::prepare(Arc::new(resources[3].clone())).unwrap()).unwrap(),
            ),
        )),
        RuntimeEntry::new(ProviderRuntime::Otlp(
            PersistentRunner::<OtlpApply>::spawn_with_published(
                key.to_string(),
                OtlpApply::build(key, &OtlpApply::prepare(Arc::new(resources[4].clone())).unwrap()).unwrap(),
            ),
        )),
        {
            let resource = &resources[5];
            RuntimeEntry::new(ProviderRuntime::Webhook(Arc::new(
                WebhookCandidate::from_link_sys(Arc::new(resource.clone()))
                    .unwrap()
                    .prepare(key)
                    .unwrap(),
            )))
        },
    ];
    for (index, entry) in entries.into_iter().enumerate() {
        RUNTIMES.install(key, Some(entry));
        let visible = [
            get_redis_client(key).is_some(),
            get_etcd_client(key).is_some(),
            get_es_client(key).is_some(),
            get_kafka_client(key).is_some(),
            get_otlp_client(key).is_some(),
            get_webhook_client(key).is_some(),
        ];
        for (provider, callable) in visible.into_iter().enumerate() {
            assert_eq!(callable, provider == index);
        }
    }
    RUNTIMES.install(key, None);
    assert!(get_webhook_client(key).is_none());
    assert!(!list_runtime_clients().iter().any(|current| current == key));
}

fn source_webhook_port(store: &LinkSysStore, key: &str) -> i32 {
    let resource = store.get(key).expect("source contains webhook");
    let SystemConfig::Webhook(config) = resource.spec.config else {
        panic!("expected webhook")
    };
    config.conn.target.service.port.expect("test service has a port")
}

#[test]
fn test_full_set() {
    let store = LinkSysStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/webhook1".to_string(),
        create_test_webhook_linksys("default", "webhook1"),
    );
    data.insert(
        "default/webhook2".to_string(),
        create_test_webhook_linksys("default", "webhook2"),
    );

    store.replace_all(data);

    assert!(store.contains("default/webhook1"));
    assert!(store.contains("default/webhook2"));
    assert!(!store.contains("default/webhook3"));
    assert_eq!(store.count(), 2);
}

#[test]
fn test_partial_update_add() {
    let store = LinkSysStore::new();

    let mut add = HashMap::new();
    add.insert(
        "default/webhook1".to_string(),
        create_test_webhook_linksys("default", "webhook1"),
    );

    store.update(add, &HashSet::new());

    assert!(store.contains("default/webhook1"));
}

#[test]
fn test_partial_update_remove() {
    let store = LinkSysStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/webhook1".to_string(),
        create_test_webhook_linksys("default", "webhook1"),
    );
    store.replace_all(data);

    let mut remove = HashSet::new();
    remove.insert("default/webhook1".to_string());
    store.update(HashMap::new(), &remove);

    assert!(!store.contains("default/webhook1"));
}

#[test]
fn test_get() {
    let store = LinkSysStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/webhook1".to_string(),
        create_test_webhook_linksys("default", "webhook1"),
    );
    store.replace_all(data);

    let ls = store.get("default/webhook1");
    assert!(ls.is_some());

    let ls = store.get("default/nonexistent");
    assert!(ls.is_none());
}

#[test]
fn dependency_rebuild_dispatches_current_snapshot_without_republishing_it() {
    let store = LinkSysStore::new();
    let source_lock = Mutex::new(());
    let key = "default/rebuild".to_string();
    let source = Arc::new(HashMap::from([(
        key.clone(),
        create_test_webhook_linksys_with_port("default", "rebuild", 8181),
    )]));
    store.resources.store(Arc::clone(&source));

    let dispatched = Mutex::new(None);
    store.rebuild_clients_with_dispatch(&source_lock, |snapshot| {
        assert!(source_lock.try_lock().is_none());
        *dispatched.lock() = Some(snapshot);
    });

    let dispatched_snapshot = dispatched
        .into_inner()
        .expect("dependency rebuild dispatches the current snapshot");
    assert!(Arc::ptr_eq(&source, &dispatched_snapshot));
    assert!(Arc::ptr_eq(&source, &store.resources.load_full()));
    assert_eq!(source_webhook_port(&store, &key), 8181);
}

fn resources(auth: InjectedSecret) -> Vec<LinkSys> {
    vec![
        create_redis_linksys("test", "link", auth),
        create_etcd_linksys("test", "link", auth),
        create_es_linksys("test", "link", auth),
        create_kafka_linksys_sasl("test", "link", auth),
        create_otlp_linksys("test", "link", auth),
        create_test_webhook_linksys("test", "link"),
    ]
}

fn one(resource: LinkSys) -> LinkSysMap {
    HashMap::from([("test/link".to_string(), resource)])
}

fn webhook_port(registry: &RuntimeRegistry, key: &str) -> i32 {
    let entry = registry.get(key).expect("webhook is registered");
    let ProviderRuntime::Webhook(client) = &entry.provider else {
        panic!("expected webhook")
    };
    match &client.managed_client().resolved_target {
        ResolvedTarget::Service { port, .. } => port.expect("test service has port"),
        other => panic!("expected service target, got {other:?}"),
    }
}

#[test]
fn empty_unified_store_reports_no_residue() {
    let registry = RuntimeRegistry::new();
    assert!(registry.entries.load().is_empty());
    assert_eq!(registry.count(ProviderKind::HttpDns), 0);
    assert!(registry.keys(ProviderKind::HttpDns).is_empty());
    for resource in resources(InjectedSecret::Valid) {
        let kind = ProviderKind::of(&resource.spec.config).unwrap();
        assert_eq!(registry.count(kind), 0);
        assert!(registry.keys(kind).is_empty());
    }
}

fn httpdns_resource(timeout: u64) -> LinkSys {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "LinkSys",
        "metadata": {"name": "link", "namespace": "test", "resourceVersion": "same-rv"},
        "spec": {"type": "httpdns", "config": {
            "preset": "aliyun", "fallback": {"type": "none"}, "connection": {"timeoutMs": timeout}
        }}
    }))
    .unwrap()
}

fn httpdns_handle(registry: &RuntimeRegistry) -> Arc<edgion_common::dns::PreparedHttpDns> {
    let entry = registry.get("test/link").unwrap();
    let ProviderRuntime::HttpDns(prepared) = &entry.provider else {
        panic!("expected HTTP DNS")
    };
    prepared.clone()
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_same_rv_actual_owner_failure_replaces_ready_reuses_failed_and_recovers() {
    let registry = RuntimeRegistry::new();
    let original = one(httpdns_resource(100));
    registry.apply(&original, &HashSet::new(), &original, true, false);
    let ready = httpdns_handle(&registry);
    assert!(ready.is_ready());
    let mut malformed = original.clone();
    malformed.get_mut("test/link").unwrap().metadata.name = None;
    assert_eq!(
        original["test/link"].metadata.resource_version,
        malformed["test/link"].metadata.resource_version
    );
    registry.apply(&malformed, &HashSet::new(), &malformed, false, false);
    let failed = httpdns_handle(&registry);
    assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed));
    assert!(edgion_common::dns::resolve(
        &edgion_common::dns::ResolverSpec::HttpDns(Arc::clone(&failed)),
        "localhost"
    )
    .await
    .is_err());
    registry.apply(&malformed, &HashSet::new(), &malformed, false, false);
    assert!(Arc::ptr_eq(&failed, &httpdns_handle(&registry)));
    registry.apply(&original, &HashSet::new(), &original, false, false);
    let repaired = httpdns_handle(&registry);
    assert!(repaired.is_ready() && !Arc::ptr_eq(&failed, &repaired));
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_same_rv_active_reference_or_material_failure_cannot_reuse_ready_client() {
    let registry = RuntimeRegistry::new();
    let mut resource = httpdns_resource(100);
    let SystemConfig::HttpDns(config) = &mut resource.spec.config else {
        unreachable!()
    };
    config.connection.as_mut().unwrap().tls = Some(
        serde_json::from_value(serde_json::json!({
            "enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"test"}}]
        }))
        .unwrap(),
    );
    let original = one(resource);
    registry.apply(&original, &HashSet::new(), &original, true, false);
    assert!(httpdns_handle(&registry).is_ready());
    for case in ["group", "material-namespace", "missing-material"] {
        let mut malformed = original.clone();
        let SystemConfig::HttpDns(config) = &mut malformed.get_mut("test/link").unwrap().spec.config else {
            unreachable!()
        };
        let tls = config.connection.as_mut().unwrap().tls.as_mut().unwrap();
        match case {
            "group" => {
                tls.outbound
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs
                    .as_mut()
                    .unwrap()[0]
                    .group = Some("other.example".into())
            }
            "material-namespace" => {
                tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
            }
            "missing-material" => tls.resolved_ca_certificates = None,
            _ => unreachable!(),
        }
        assert_eq!(
            original["test/link"].metadata.resource_version,
            malformed["test/link"].metadata.resource_version
        );
        let ready = httpdns_handle(&registry);
        registry.apply(&malformed, &HashSet::new(), &malformed, false, false);
        let failed = httpdns_handle(&registry);
        assert!(!failed.is_ready() && !Arc::ptr_eq(&ready, &failed), "{case}");
        registry.apply(&malformed, &HashSet::new(), &malformed, false, false);
        assert!(Arc::ptr_eq(&failed, &httpdns_handle(&registry)), "{case}");
        registry.apply(&original, &HashSet::new(), &original, false, false);
        assert!(httpdns_handle(&registry).is_ready(), "{case}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_relist_reuses_but_changes_even_with_same_rv_publish_latest_fallback() {
    let registry = RuntimeRegistry::new();
    let original = one(httpdns_resource(100));
    registry.apply(&original, &HashSet::new(), &original, true, false);
    let first = httpdns_handle(&registry);
    assert!(first.is_ready());
    registry.apply(&original, &HashSet::new(), &original, false, false);
    assert!(Arc::ptr_eq(&first, &httpdns_handle(&registry)));
    let invalid = one(httpdns_resource(0));
    registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
    let failed = httpdns_handle(&registry);
    assert!(!failed.is_ready());
    assert!(!Arc::ptr_eq(&first, &failed));
    assert!(
        edgion_common::dns::resolve(&edgion_common::dns::ResolverSpec::HttpDns(failed.clone()), "localhost")
            .await
            .is_err()
    );
    registry.apply(&invalid, &HashSet::new(), &invalid, true, false);
    assert!(
        Arc::ptr_eq(&failed, &httpdns_handle(&registry)),
        "failed preparation is reused across relists"
    );
    registry.apply(&invalid, &HashSet::new(), &invalid, true, true);
    assert!(
        !Arc::ptr_eq(&failed, &httpdns_handle(&registry)),
        "forced inherited-policy rebuild replaces failed state too"
    );
    registry.apply(&original, &HashSet::new(), &original, false, false);
    let recovered = httpdns_handle(&registry);
    assert!(recovered.is_ready());
    registry.apply(&original, &HashSet::new(), &original, true, true);
    assert!(!Arc::ptr_eq(&recovered, &httpdns_handle(&registry)));
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_delete_kind_change_and_full_omission_have_one_current_owner() {
    let registry = RuntimeRegistry::new();
    let original = one(httpdns_resource(100));
    registry.apply(&original, &HashSet::new(), &original, true, false);
    let old = registry.get("test/link").unwrap();
    let webhook = one(create_test_webhook_linksys("test", "link"));
    registry.apply(&webhook, &HashSet::new(), &webhook, false, false);
    assert!(old.stopped.load(Ordering::Acquire));
    assert_eq!(registry.count(ProviderKind::HttpDns), 0);
    assert_eq!(registry.count(ProviderKind::Webhook), 1);
    registry.apply(&original, &HashSet::new(), &original, false, false);
    assert_eq!(registry.entries.load().len(), 1);
    registry.apply(&LinkSysMap::new(), &HashSet::new(), &LinkSysMap::new(), true, false);
    assert!(registry.entries.load().is_empty());
    old.stop();
    assert!(
        registry.entries.load().is_empty(),
        "retired owners cannot recreate omission"
    );
    registry.apply(&original, &HashSet::new(), &original, false, false);
    registry.apply(
        &LinkSysMap::new(),
        &HashSet::from(["test/link".into()]),
        &LinkSysMap::new(),
        false,
        false,
    );
    assert!(registry.entries.load().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn httpdns_secret_material_rotation_replaces_client_and_preserves_address_identity() {
    use k8s_openapi::{api::core::v1::Secret, ByteString};
    let registry = RuntimeRegistry::new();
    let mut resource = httpdns_resource(100);
    let SystemConfig::HttpDns(config) = &mut resource.spec.config else {
        unreachable!()
    };
    config.connection.as_mut().unwrap().tls = Some(edgion_resources::resources::common::TlsConfig {
        enabled: true,
        outbound: serde_json::from_value(serde_json::json!({"verify":false,"validation":{
            "caCertificateRefs":[{"kind":"Secret","name":"ca"}]
        }}))
        .unwrap(),
        resolved_ca_certificates: Some(vec![Secret {
            metadata: serde_json::from_value(serde_json::json!({"name":"ca","namespace":"test"})).unwrap(),
            data: Some(std::collections::BTreeMap::from([(
                "tls.ca.crt".into(),
                ByteString(b"old-material".to_vec()),
            )])),
            ..Default::default()
        }]),
        ..Default::default()
    });
    let data = one(resource.clone());
    registry.apply(&data, &HashSet::new(), &data, true, false);
    let first = httpdns_handle(&registry);
    assert!(first.is_ready());
    let fingerprint = edgion_common::dns::ResolverSpec::HttpDns(first.clone()).fingerprint();
    let SystemConfig::HttpDns(config) = &mut resource.spec.config else {
        unreachable!()
    };
    config
        .connection
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .resolved_ca_certificates
        .as_mut()
        .unwrap()[0]
        .data
        .as_mut()
        .unwrap()
        .insert("tls.ca.crt".into(), ByteString(b"new-material".to_vec()));
    let data = one(resource);
    registry.apply(&data, &HashSet::new(), &data, false, false);
    let second = httpdns_handle(&registry);
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(
        fingerprint,
        edgion_common::dns::ResolverSpec::HttpDns(second).fingerprint()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_provider_type_change_has_one_current_owner() {
    for old_resource in resources(InjectedSecret::Valid) {
        for new_resource in resources(InjectedSecret::Valid) {
            let registry = RuntimeRegistry::new();
            let old_data = one(old_resource.clone());
            registry.apply(&old_data, &HashSet::new(), &old_data, true, false);
            let old = registry.get("test/link").unwrap();
            let new_data = one(new_resource);
            registry.apply(&new_data, &HashSet::new(), &new_data, false, false);
            let current = registry.get("test/link").unwrap();
            assert_eq!(registry.entries.load().len(), 1);
            assert!(Some(current.kind()) == ProviderKind::of(&new_data["test/link"].spec.config));
            if old.kind() == current.kind() {
                assert!(Arc::ptr_eq(&old, &current), "same kind reuses its owner");
            } else {
                assert!(old.stopped.load(Ordering::Acquire), "old owner is cancelled");
                assert!(!Arc::ptr_eq(&old, &current));
            }
            // Drop cancels every posted desired state before yielding; these
            // registry tests never initialize real network clients.
            drop(registry);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn full_set_omission_and_delete_recreate_cannot_resurrect_old_owner() {
    for resource in resources(InjectedSecret::Valid) {
        let registry = RuntimeRegistry::new();
        let data = one(resource);
        registry.apply(&data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        registry.apply(&HashMap::new(), &HashSet::new(), &HashMap::new(), true, false);
        assert!(registry.entries.load().is_empty());
        assert!(old.stopped.load(Ordering::Acquire));
        registry.apply(&data, &HashSet::new(), &data, false, false);
        let new = registry.get("test/link").unwrap();
        assert!(!Arc::ptr_eq(&old, &new), "recreation is immediate and independent");
        old.stop();
        assert!(Arc::ptr_eq(&new, &registry.get("test/link").unwrap()));
        registry.apply(
            &HashMap::new(),
            &HashSet::from(["test/link".to_string()]),
            &HashMap::new(),
            false,
            false,
        );
        assert!(registry.entries.load().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn metadata_only_kind_change_removes_previous_runtime() {
    let configurations = [
        SystemConfig::HttpDns(
            serde_json::from_value(serde_json::json!({
                "preset": "tencent"
            }))
            .unwrap(),
        ),
        SystemConfig::CredentialSource(
            serde_json::from_value(serde_json::json!({
                "provider": {
                    "type": "oauth2ClientCredentials",
                    "tokenEndpoint": "https://issuer.example/token",
                    "clientAuthentication": { "activeSecretRef": { "name": "bootstrap" } }
                }
            }))
            .unwrap(),
        ),
    ];
    for config in configurations {
        let registry = RuntimeRegistry::new();
        let old_data = one(create_test_webhook_linksys("test", "link"));
        registry.apply(&old_data, &HashSet::new(), &old_data, true, false);
        let mut resource = old_data["test/link"].clone();
        let expects_httpdns = matches!(&config, SystemConfig::HttpDns(_));
        resource.spec.config = config;
        let data = one(resource);
        registry.apply(&data, &HashSet::new(), &data, false, false);
        if expects_httpdns {
            assert!(matches!(
                registry.get("test/link").as_deref().map(|entry| &entry.provider),
                Some(ProviderRuntime::HttpDns(_))
            ));
            assert_eq!(registry.count(ProviderKind::Webhook), 0);
        } else {
            assert!(registry.get("test/link").is_none());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pending_and_invalid_credentials_never_admit_any_persistent_provider() {
    for auth in [InjectedSecret::Missing, InjectedSecret::MalformedForInvalid] {
        for resource in resources(auth).into_iter().take(5) {
            for full in [true, false] {
                let registry = RuntimeRegistry::new();
                let data = one(resource.clone());
                registry.apply(&data, &HashSet::new(), &data, full, false);
                assert!(registry.entries.load().is_empty());
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pending_same_kind_retains_owner_but_changed_kind_removes_it() {
    let ready = resources(InjectedSecret::Valid);
    let pending = resources(InjectedSecret::Missing);
    for (index, resource) in ready.iter().take(5).enumerate() {
        let registry = RuntimeRegistry::new();
        let data = one(resource.clone());
        registry.apply(&data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        let pending_same = one(pending[index].clone());
        registry.apply(&pending_same, &HashSet::new(), &pending_same, false, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        let changed = one(pending[(index + 1) % 5].clone());
        registry.apply(&changed, &HashSet::new(), &changed, false, false);
        assert!(registry.get("test/link").is_none());
        assert!(old.stopped.load(Ordering::Acquire));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mixed_provider_admission_counts_are_independent_and_recomputed_after_removal() {
    let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
    let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
    let registry = RuntimeRegistry::new();
    let resources = resources(InjectedSecret::Valid);
    let mut snapshot = HashMap::new();
    for (index, resource) in resources.iter().enumerate() {
        snapshot.insert(format!("test/first-{index}"), resource.clone());
        snapshot.insert(format!("test/second-{index}"), resource.clone());
    }
    registry.apply(&snapshot, &HashSet::new(), &snapshot, true, false);
    let mut remove = HashSet::new();
    for resource in &resources {
        let kind = ProviderKind::of(&resource.spec.config).unwrap();
        assert_eq!(registry.count(kind), if kind == ProviderKind::Webhook { 2 } else { 1 });
        if kind != ProviderKind::Webhook {
            remove.extend(registry.keys(kind));
        }
    }
    for key in &remove {
        snapshot.remove(key);
    }
    registry.apply(&HashMap::new(), &remove, &snapshot, false, false);
    assert_eq!(registry.entries.load().len(), 7);
    for key in &remove {
        assert!(registry.get(key).is_none());
    }
    for resource in &resources {
        let kind = ProviderKind::of(&resource.spec.config).unwrap();
        assert_eq!(registry.count(kind), if kind == ProviderKind::Webhook { 2 } else { 1 });
    }
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
}

#[tokio::test(flavor = "current_thread")]
async fn capacity_release_retries_latest_unadmitted_configuration() {
    let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
    let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
    for resource in resources(InjectedSecret::Valid).into_iter().take(5) {
        let registry = RuntimeRegistry::new();
        let first = HashMap::from([("test/first".to_string(), resource.clone())]);
        registry.apply(&first, &HashSet::new(), &first, true, false);
        let changed = HashMap::from([("test/second".to_string(), resource.clone())]);
        let mut snapshot = first;
        snapshot.extend(changed.clone());
        registry.apply(&changed, &HashSet::new(), &snapshot, false, false);
        assert!(registry.get("test/first").is_some());
        assert!(registry.get("test/second").is_none());
        let mut latest = resource;
        latest.metadata.resource_version = Some("latest".to_string());
        match &mut latest.spec.config {
            SystemConfig::Redis(config) => config.endpoints = vec!["redis://127.0.0.1:6380".to_string()],
            SystemConfig::Etcd(config) => config.endpoints = vec!["http://127.0.0.1:2378".to_string()],
            SystemConfig::Elasticsearch(config) => config.endpoints = vec!["http://127.0.0.1:9201".to_string()],
            SystemConfig::Kafka(config) => config.brokers = vec!["127.0.0.1:9093".to_string()],
            SystemConfig::Otlp(config) => config.endpoint = "http://192.0.2.1:4318".to_string(),
            _ => panic!("test covers persistent providers only"),
        }
        let latest_delta = HashMap::from([("test/second".to_string(), latest.clone())]);
        snapshot.extend(latest_delta.clone());
        registry.apply(&latest_delta, &HashSet::new(), &snapshot, false, false);
        assert!(registry.get("test/second").is_none());
        snapshot.remove("test/first");
        registry.apply(
            &HashMap::new(),
            &HashSet::from(["test/first".to_string()]),
            &snapshot,
            false,
            false,
        );
        assert!(registry.get("test/first").is_none());
        assert!(
            registry.get("test/second").is_some(),
            "capacity release must recover without another apply"
        );
        let admitted = registry.get("test/second").unwrap();
        let desired = match &admitted.provider {
            ProviderRuntime::Redis(runner) => runner.desired_link_for_test(),
            ProviderRuntime::Etcd(runner) => runner.desired_link_for_test(),
            ProviderRuntime::Elasticsearch(runner) => runner.desired_link_for_test(),
            ProviderRuntime::Kafka(runner) => runner.desired_link_for_test(),
            ProviderRuntime::Otlp(runner) => runner.desired_link_for_test(),
            ProviderRuntime::Webhook(_) | ProviderRuntime::HttpDns(_) => {
                panic!("test covers persistent providers only")
            }
        }
        .unwrap();
        assert_eq!(desired.metadata.resource_version.as_deref(), Some("latest"));
        assert_eq!(
            serde_json::to_value(&desired.spec).unwrap(),
            serde_json::to_value(&latest.spec).unwrap()
        );
    }
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
}

#[tokio::test(flavor = "current_thread")]
async fn capacity_increase_retries_without_reposting_existing_runner() {
    let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
    let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
    let registry = RuntimeRegistry::new();
    let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
    let first = HashMap::from([("test/first".to_string(), resource.clone())]);
    registry.apply(&first, &HashSet::new(), &first, true, false);
    let old = registry.get("test/first").unwrap();
    let mut snapshot = first;
    snapshot.insert("test/second".to_string(), resource);
    registry.retry_admission(&snapshot);
    assert!(registry.get("test/second").is_none());
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(2, Ordering::Relaxed);
    registry.retry_admission(&snapshot);
    assert!(registry.get("test/second").is_some());
    assert!(Arc::ptr_eq(&old, &registry.get("test/first").unwrap()));
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
}

#[tokio::test(flavor = "current_thread")]
async fn omitted_capacity_waiter_is_not_recreated_after_release() {
    let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
    let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
    let registry = RuntimeRegistry::new();
    let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
    let first = HashMap::from([("test/first".to_string(), resource.clone())]);
    registry.apply(&first, &HashSet::new(), &first, true, false);
    let mut snapshot = first;
    snapshot.insert("test/second".to_string(), resource);
    registry.retry_admission(&snapshot);
    registry.apply(&HashMap::new(), &HashSet::new(), &HashMap::new(), true, false);
    registry.retry_admission(&HashMap::new());
    assert!(registry.entries.load().is_empty());
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
}

#[tokio::test(flavor = "current_thread")]
async fn webhook_relist_reuses_and_material_update_replaces_current_client() {
    let registry = RuntimeRegistry::new();
    let data = one(create_test_webhook_linksys("test", "link"));
    registry.apply(&data, &HashSet::new(), &data, true, false);
    let old = registry.get("test/link").unwrap();
    registry.apply(&data, &HashSet::new(), &data, true, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    let changed = one(create_test_webhook_linksys_with_port("test", "link", 9090));
    registry.apply(&changed, &HashSet::new(), &changed, false, false);
    assert!(old.stopped.load(Ordering::Acquire));
    assert_eq!(webhook_port(&registry, "test/link"), 9090);
    let current = registry.get("test/link").unwrap();
    registry.apply(&changed, &HashSet::new(), &changed, true, true);
    assert!(!Arc::ptr_eq(&current, &registry.get("test/link").unwrap()));
}

#[tokio::test(flavor = "current_thread")]
async fn webhook_prepare_failure_preserves_same_kind_but_not_foreign_kind() {
    let registry = RuntimeRegistry::new();
    let data = one(create_test_webhook_linksys("test", "link"));
    registry.apply(&data, &HashSet::new(), &data, true, false);
    let old = registry.get("test/link").unwrap();
    let mut invalid = data["test/link"].clone();
    let SystemConfig::Webhook(config) = &mut invalid.spec.config else {
        panic!("expected webhook")
    };
    config.conn.target.service.port = None;
    let invalid = one(invalid);
    registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
    assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    let redis = one(create_redis_linksys("test", "link", InjectedSecret::Valid));
    registry.apply(&redis, &HashSet::new(), &redis, false, false);
    registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
    assert!(registry.get("test/link").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn source_events_serialize_full_sets_deltas_rebuilds_and_admission() {
    use std::sync::mpsc;
    use std::time::Duration;

    // The executor stays on this thread while scoped threads prepare owners,
    // so persistent provider tasks cannot perform network IO in this test.
    for first_full in [true, false] {
        for operation in ["delta", "full", "rebuild", "admission"] {
            let source = LinkSysStore::new();
            let registry = RuntimeRegistry::new();
            let source_lock = Mutex::new(());
            let (prepared_tx, prepared_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (attempt_tx, attempt_rx) = mpsc::channel();
            let source = &source;
            let registry = &registry;
            let source_lock = &source_lock;
            let handle = tokio::runtime::Handle::current();
            std::thread::scope(|scope| {
                let first_handle = handle.clone();
                let first = scope.spawn(move || {
                    let _runtime = first_handle.enter();
                    let data =
                        HashMap::from([("test/first".to_string(), create_test_webhook_linksys("test", "first"))]);
                    let dispatch = |snapshot: Arc<LinkSysMap>| {
                        assert!(source_lock.try_lock().is_none());
                        registry.apply(&snapshot, &HashSet::new(), &snapshot, first_full, false);
                        prepared_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    };
                    if first_full {
                        source.replace_all_with_dispatch(data, source_lock, dispatch);
                    } else {
                        source.update_with_dispatch(data, &HashSet::new(), source_lock, |_, _, snapshot| {
                            dispatch(snapshot);
                        });
                    }
                });
                prepared_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let successor = scope.spawn(move || {
                    let _runtime = handle.enter();
                    attempt_tx.send(()).unwrap();
                    if operation == "rebuild" || operation == "admission" {
                        source.rebuild_clients_with_dispatch(source_lock, |snapshot| {
                            assert!(source_lock.try_lock().is_none());
                            assert!(Arc::ptr_eq(&snapshot, &source.resources.load_full()));
                            assert!(snapshot.contains_key("test/first"));
                            let old = registry.get("test/first").unwrap();
                            if operation == "rebuild" {
                                registry.apply(&snapshot, &HashSet::new(), &snapshot, true, true);
                                assert!(!Arc::ptr_eq(&old, &registry.get("test/first").unwrap()));
                            } else {
                                registry.retry_admission(&snapshot);
                                assert!(Arc::ptr_eq(&old, &registry.get("test/first").unwrap()));
                            }
                        });
                    } else {
                        let mut data =
                            HashMap::from([("test/second".to_string(), create_test_webhook_linksys("test", "second"))]);
                        let dispatch = |changed: Arc<LinkSysMap>, remove, snapshot: Arc<LinkSysMap>| {
                            assert!(source_lock.try_lock().is_none());
                            assert!(source.contains("test/first"));
                            assert!(snapshot.contains_key("test/first"));
                            assert!(!source.contains("test/second"));
                            registry.apply(&changed, &remove, &snapshot, operation == "full", false);
                            assert!(registry.get("test/second").is_some());
                        };
                        if operation == "full" {
                            data.insert("test/first".into(), create_test_webhook_linksys("test", "first"));
                            source.replace_all_with_dispatch(data, source_lock, |snapshot| {
                                dispatch(snapshot.clone(), HashSet::new(), snapshot);
                            });
                        } else {
                            source.update_with_dispatch(data, &HashSet::new(), source_lock, dispatch);
                        }
                    }
                });
                attempt_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert_eq!(source.count(), 0, "raw publication must wait for preparation");
                assert!(registry.get("test/first").is_some());
                assert!(registry.get("test/second").is_none());
                release_tx.send(()).unwrap();
                first.join().unwrap();
                successor.join().unwrap();
            });
            let expected = if operation == "delta" || operation == "full" {
                2
            } else {
                1
            };
            assert_eq!(source.count(), expected);
            assert_eq!(registry.entries.load().len(), expected);
        }
    }
}

#[test]
fn otlp_shutdown_serializes_with_source_updates() {
    const CHILD: &str = "EDGION_OTLP_SOURCE_ORDER_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "link_sys::runtime::store::tests::otlp_shutdown_serializes_with_source_updates",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "shutdown child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Shutdown is terminal process state. No runner can poll before cancellation:
    // this current-thread executor remains blocked until both writers finish.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = get_global_link_sys_store();
            let resource = create_otlp_linksys("test", "link", InjectedSecret::Valid);
            store.replace_all(one(resource.clone()));
            assert_eq!(otlp_client_count(), 1);
            let source_guard = LINK_SYS_SOURCE_EVENT_LOCK.lock();
            let handle = tokio::runtime::Handle::current();
            std::thread::scope(|scope| {
                let shutdown_handle = handle.clone();
                let shutdown = scope.spawn(move || {
                    let _runtime = shutdown_handle.enter();
                    assert!(shutdown_otlp_clients().now_or_never().is_none());
                });
                let started = std::time::Instant::now();
                while OTLP_SHUTDOWN.try_lock().is_some() && !OTLP_STOPPING.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now().saturating_duration_since(started)
                            < std::time::Duration::from_secs(5)
                    );
                    std::thread::yield_now();
                }
                assert!(!OTLP_STOPPING.load(Ordering::Acquire));
                assert_eq!(otlp_client_count(), 1, "shutdown must wait for the source lock");
                let update = scope.spawn(move || {
                    let _runtime = handle.enter();
                    store.update(
                        HashMap::from([
                            ("test/link".into(), resource),
                            (
                                "test/unrelated".into(),
                                create_test_webhook_linksys("test", "unrelated"),
                            ),
                        ]),
                        &HashSet::new(),
                    );
                });
                drop(source_guard);
                shutdown.join().unwrap();
                update.join().unwrap();
            });
            shutdown_otlp_clients().await;
            assert_eq!(otlp_client_count(), 0);
            assert_eq!(otlp_retiring_client_count(), 0);
            assert!(get_webhook_client("test/unrelated").is_some());
            get_global_link_sys_store().update(
                one(create_otlp_linksys("test", "link", InjectedSecret::Valid)),
                &HashSet::new(),
            );
            assert_eq!(otlp_client_count(), 0);
            shutdown_otlp_clients().await;
            assert!(get_webhook_client("test/unrelated").is_some());
        });
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_dispatch_publishes_before_source_and_preserves_unrelated_deltas() {
    let source = LinkSysStore::new();
    let registry = RuntimeRegistry::new();
    let source_lock = Mutex::new(());
    for name in ["first", "second"] {
        let key = format!("test/{name}");
        let changed = HashMap::from([(key.clone(), create_test_webhook_linksys("test", name))]);
        source.update_with_dispatch(changed, &HashSet::new(), &source_lock, |changed, remove, snapshot| {
            assert!(!source.contains(&key), "raw source must not outrun preparation");
            registry.apply(&changed, &remove, &snapshot, false, false);
            assert!(
                registry.get(&key).is_some(),
                "dispatch is complete before source publication"
            );
        });
        assert!(source.contains(&key));
    }
    assert_eq!(registry.entries.load().len(), 2);
    source.replace_all_with_dispatch(HashMap::new(), &source_lock, |snapshot| {
        registry.apply(&snapshot, &HashSet::new(), &snapshot, true, false);
        assert!(registry.entries.load().is_empty());
        assert_eq!(source.count(), 2);
    });
    assert_eq!(source.count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn published_etcd_client_is_sampled_without_network_init() {
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    let registry = RuntimeRegistry::new();
    let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
    let client = EtcdApply::build("test/link", &EtcdApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client);
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
    );
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || record_registry_health(&registry));
    assert!(snapshotter.snapshot().into_vec().iter().any(|(key, _, _, value)| {
        key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
            && matches!(value, DebugValue::Gauge(level) if *level == 0.0)
    }));
    drop(registry);
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn last_registry_owner_drop_stops_runner_without_waiting_for_arc_drop() {
    let registry = RuntimeRegistry::new();
    let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
    let client = EtcdApply::build("test/link", &EtcdApply::prepare(Arc::new(resource.clone())).unwrap()).unwrap();
    let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client);
    registry.install(
        "test/link",
        Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
    );
    assert!(runner.current().is_some());
    drop(registry);
    assert!(runner.current().is_none());
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_extension_retires_every_provider_and_recovers_full_and_partial() {
    for full in [true, false] {
        for resource in resources(InjectedSecret::Valid) {
            let registry = RuntimeRegistry::new();
            let original = one(resource.clone());
            registry.apply(&original, &HashSet::new(), &original, full, false);
            let old = registry.get("test/link").unwrap();
            let mut unsupported = resource;
            unsupported.spec.config = SystemConfig::Unsupported;
            let unknown = one(unsupported);
            registry.apply(&unknown, &HashSet::new(), &unknown, full, false);
            assert!(registry.get("test/link").is_none());
            assert!(registry.entries.load().is_empty());
            assert!(old.stopped.load(Ordering::Acquire));
            registry.apply(&original, &HashSet::new(), &original, full, false);
            let current = registry.get("test/link").unwrap();
            assert!(!Arc::ptr_eq(&old, &current));
            assert!(current.kind() == old.kind());
            old.stop();
            assert!(Arc::ptr_eq(&current, &registry.get("test/link").unwrap()));
            // No yield before cancellation: no provider performs network IO.
            drop(registry);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn webhook_forced_tls_rebuild_revokes_unusable_generation_and_recovers() {
    for unresolved_identity in [false, true] {
        let registry = RuntimeRegistry::new();
        let data = one(create_test_webhook_linksys("test", "link"));
        registry.apply(&data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        let mut invalid = data["test/link"].clone();
        let SystemConfig::Webhook(config) = &mut invalid.spec.config else {
            panic!("expected webhook")
        };
        if unresolved_identity {
            config.conn.tls = Some(
                serde_json::from_value(serde_json::json!({
                    "enabled": true, "clientCertificateRef": {"name": "missing"}
                }))
                .unwrap(),
            );
        } else {
            config.conn.target.service.port = None;
        }
        let invalid = one(invalid);
        registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        registry.apply(&invalid, &HashSet::new(), &invalid, true, true);
        assert!(registry.get("test/link").is_none());
        assert!(old.stopped.load(Ordering::Acquire));
        assert_eq!(registry.count(ProviderKind::Webhook), 0);
        registry.apply(&invalid, &HashSet::new(), &invalid, true, false);
        assert!(registry.get("test/link").is_none());
        registry.apply(&data, &HashSet::new(), &data, true, true);
        let recovered = registry.get("test/link").unwrap();
        assert!(!Arc::ptr_eq(&old, &recovered));
        assert!(!recovered.stopped.load(Ordering::Acquire));
        assert_eq!(registry.count(ProviderKind::Webhook), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn webhook_same_rv_invalid_owner_preserves_last_good_but_forced_rebuild_withdraws() {
    for source in ["name", "namespace"] {
        let registry = RuntimeRegistry::new();
        let mut valid = create_test_webhook_linksys("test", "link");
        valid.metadata.resource_version = Some("42".into());
        let data = one(valid.clone());
        registry.apply(&data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        let mut invalid = valid;
        if source == "name" {
            invalid.metadata.name = None;
        } else {
            invalid.metadata.namespace = Some("".into());
        }
        let ProviderRuntime::Webhook(client) = &old.provider else {
            unreachable!()
        };
        assert!(!WebhookCandidate::from_link_sys(Arc::new(invalid.clone()))
            .unwrap()
            .can_reuse(client, false));
        let invalid = one(invalid);
        for _ in 0..2 {
            registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
            assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
            assert!(!old.stopped.load(Ordering::Acquire));
        }
        registry.apply(&invalid, &HashSet::new(), &invalid, true, true);
        assert!(old.stopped.load(Ordering::Acquire));
        assert!(registry.get("test/link").is_none());
        assert_eq!(registry.count(ProviderKind::Webhook), 0);
        registry.apply(&data, &HashSet::new(), &data, false, false);
        assert!(!Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert_eq!(registry.count(ProviderKind::Webhook), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn webhook_same_rv_active_reference_failure_obeys_ordinary_and_forced_policy() {
    for failure in ["group", "material-namespace", "missing-material"] {
        let registry = RuntimeRegistry::new();
        let mut valid = create_test_webhook_linksys("test", "link");
        valid.metadata.resource_version = Some("42".into());
        let SystemConfig::Webhook(config) = &mut valid.spec.config else {
            unreachable!()
        };
        config.conn.tls = Some(serde_json::from_value(serde_json::json!({
            "enabled":true,"verify":false,
            "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"test"},"data":{"ca.crt":"Y2VydGlmaWNhdGU="}}]
        })).unwrap());
        let data = one(valid.clone());
        registry.apply(&data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        let mut invalid = valid;
        let SystemConfig::Webhook(config) = &mut invalid.spec.config else {
            unreachable!()
        };
        let tls = config.conn.tls.as_mut().unwrap();
        match failure {
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
            "material-namespace" => {
                tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
            }
            _ => tls.resolved_ca_certificates = None,
        }
        let ProviderRuntime::Webhook(client) = &old.provider else {
            unreachable!()
        };
        assert!(!WebhookCandidate::from_link_sys(Arc::new(invalid.clone()))
            .unwrap()
            .can_reuse(client, false));
        let invalid = one(invalid);
        registry.apply(&invalid, &HashSet::new(), &invalid, false, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        registry.apply(&invalid, &HashSet::new(), &invalid, true, true);
        assert!(registry.get("test/link").is_none());
        assert!(old.stopped.load(Ordering::Acquire));
        registry.apply(&data, &HashSet::new(), &data, false, false);
        assert_eq!(registry.count(ProviderKind::Webhook), 1);
        assert!(!Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
    }
}
