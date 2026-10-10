use super::{normalize_string_data, SecretHandler};
use crate::conf_mgr::sync_runtime::resource_processor::{
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

struct PluginsNoop;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for PluginsNoop {}

async fn assert_plugins_requeued(processor: &ResourceProcessor<EdgionPlugins>) {
    let item = tokio::time::timeout(Duration::from_secs(1), processor.workqueue().dequeue())
        .await
        .expect("dependent plugin should be requeued")
        .expect("work item should exist");
    assert_eq!(item.key, "default/policy");
    processor.workqueue().done(&item.key);
}

#[test]
fn test_has_non_empty_secret_field_reads_data() {
    let mut secret = Secret::default();
    let mut data = BTreeMap::new();
    data.insert("clientSecret".to_string(), ByteString(b"abc".to_vec()));
    secret.data = Some(data);

    assert!(SecretHandler::has_non_empty_secret_field(
        &secret,
        &["clientSecret", "client_secret"]
    ));
}

#[test]
fn test_has_non_empty_secret_field_reads_string_data() {
    let mut secret = Secret::default();
    let mut string_data = BTreeMap::new();
    string_data.insert("session_secret".to_string(), "v".to_string());
    secret.string_data = Some(string_data);

    assert!(SecretHandler::has_non_empty_secret_field(
        &secret,
        &["sessionSecret", "session_secret"]
    ));
}

#[test]
fn test_has_non_empty_secret_field_false_when_missing() {
    let secret = Secret::default();
    assert!(!SecretHandler::has_non_empty_secret_field(
        &secret,
        &["clientSecret", "client_secret", "secret"]
    ));
}

#[test]
fn normalize_string_data_copies_into_data_and_clears() {
    let mut secret = Secret::default();
    let mut sd = BTreeMap::new();
    sd.insert("password".to_string(), "hunter2".to_string());
    sd.insert("username".to_string(), "alice".to_string());
    secret.string_data = Some(sd);

    normalize_string_data(&mut secret);

    assert!(secret.string_data.is_none(), "stringData must be cleared");
    let data = secret.data.expect("data must be present");
    assert_eq!(data.get("password").unwrap().0, b"hunter2".to_vec());
    assert_eq!(data.get("username").unwrap().0, b"alice".to_vec());
}

#[test]
fn normalize_string_data_overrides_existing_data_entry() {
    let mut secret = Secret::default();
    let mut data = BTreeMap::new();
    data.insert("password".to_string(), ByteString(b"old".to_vec()));
    secret.data = Some(data);
    let mut sd = BTreeMap::new();
    sd.insert("password".to_string(), "new".to_string());
    secret.string_data = Some(sd);

    normalize_string_data(&mut secret);

    let data = secret.data.unwrap();
    assert_eq!(
        data.get("password").unwrap().0,
        b"new".to_vec(),
        "stringData must win over data for duplicate keys"
    );
}

#[test]
fn normalize_string_data_noop_when_absent() {
    let mut secret = Secret::default();
    let mut data = BTreeMap::new();
    data.insert("key".to_string(), ByteString(b"val".to_vec()));
    secret.data = Some(data);

    normalize_string_data(&mut secret);

    assert!(secret.string_data.is_none());
    assert_eq!(secret.data.unwrap().get("key").unwrap().0, b"val".to_vec());
}

#[tokio::test]
async fn secret_apply_and_delete_requeue_nested_request_access_policy_dependents() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        100,
        Arc::new(PluginsNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let secret = Secret {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("origin-token".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let dependent = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("default".to_string()),
        "policy".to_string(),
    );

    runtime
        .secret_ref_manager
        .add_ref("default/origin-token".to_string(), dependent.clone());
    SecretHandler::new().on_change(&secret, &ctx).await;
    assert_plugins_requeued(&processor).await;

    runtime
        .secret_ref_manager
        .add_ref("default/origin-token".to_string(), dependent);
    SecretHandler::new().on_delete(&secret, &ctx).await;
    assert_plugins_requeued(&processor).await;
    assert_eq!(runtime.secret_ref_manager.get_refs("default/origin-token").len(), 1);

    SecretHandler::new().on_change(&secret, &ctx).await;
    assert_plugins_requeued(&processor).await;
}

#[tokio::test]
async fn frontend_ca_source_delete_preserves_typed_edges_and_kind_isolation() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let dependent = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );
    let secret_key = FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "default", "same-name");
    let configmap_key = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "same-name");
    runtime
        .frontend_ca_ref_manager
        .add_ref(secret_key.index_key(), dependent.clone());
    runtime
        .frontend_ca_ref_manager
        .add_ref(configmap_key.index_key(), dependent);

    let secret = Secret {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("same-name".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    SecretHandler::new().on_delete(&secret, &ctx).await;

    assert_eq!(
        runtime.frontend_ca_ref_manager.get_refs(&secret_key.index_key()).len(),
        1
    );
    assert_eq!(
        runtime
            .frontend_ca_ref_manager
            .get_refs(&configmap_key.index_key())
            .len(),
        1
    );
}

#[tokio::test]
async fn native_gateway_secret_events_recover_missing_ambiguous_and_deleted_material() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::resources::gateway::FrontendCAResolutionError;
    use edgion_resources::{EdgionPlugins, Gateway};
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap, HashSet};
    struct RawDependent;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for RawDependent {}
    macro_rules! work {
        ($processor:expr, $raw:expr, $key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let source = $raw.get(&item.key).map(|value| (*value).clone());
            $processor
                .process_work_item(&item.key, source, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let pem = params.self_signed(&key).unwrap().pem();
    for owner_namespace in [None, Some("default")] {
        let runtime = Arc::new(ControllerRuntime::default());
        let sources = Arc::new(ResourceProcessor::<Secret>::new(
            "Secret",
            32,
            Arc::new(SecretHandler::new()),
            runtime.clone(),
        ));
        let source_raw = Arc::new(LocalRawStore::new());
        sources.set_raw_store(source_raw.clone());
        runtime.processor_registry.register(sources.clone());
        let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            32,
            Arc::new(GatewayHandler::new(None, None)),
            runtime.clone(),
        ));
        let gateway_raw = Arc::new(LocalRawStore::new());
        gateways.set_raw_store(gateway_raw.clone());
        runtime.processor_registry.register(gateways.clone());
        let plugins = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            32,
            Arc::new(RawDependent),
            runtime.clone(),
        ));
        runtime.processor_registry.register(plugins.clone());
        let owner_key = if owner_namespace.is_some() {
            "default/edge"
        } else {
            "edge"
        };
        let mut value = serde_json::json!({"metadata":{"name":"edge","namespace":owner_namespace},"spec":{"gatewayClassName":"edge",
            "listeners":[{"name":"https","protocol":"HTTPS","port":443,"tls":{"mode":"Terminate"}}],
            "tls":{"frontend":{"default":{"validation":{"caCertificateRefs":[{"group":"","kind":"Secret","name":"ca"}]}}}}}});
        value["spec"]["listeners"][0]["tls"]["certificateRefs"] = serde_json::json!([{"name":"ca"}]);
        value["spec"]["tls"]["backend"] = serde_json::json!({"clientCertificateRef":{"name":"ca"}});
        let gateway: Gateway = serde_json::from_value(value).unwrap();
        gateway_raw.upsert(owner_key.into(), Arc::new(gateway));
        gateways.on_event_key(owner_key.into()).await;
        work!(gateways, gateway_raw, owner_key);
        let first = gateways.get(owner_key).unwrap();
        assert_eq!(
            first.spec.listeners.as_ref().unwrap()[0]
                .tls
                .as_ref()
                .unwrap()
                .resolved_frontend_ca_refs
                .as_ref()
                .unwrap()[0]
                .error,
            Some(FrontendCAResolutionError::NotFound)
        );
        let dependent = ResourceRef::new(ResourceKind::Gateway, owner_namespace.map(str::to_owned), "edge".into());
        let typed = FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "default", "ca").index_key();
        assert_eq!(
            runtime.frontend_ca_ref_manager.get_refs(&typed),
            std::slice::from_ref(&dependent)
        );
        let wrong = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "ca").index_key();
        assert!(runtime.frontend_ca_ref_manager.get_refs(&wrong).is_empty());
        // A different material kind with the same namespace/name is not a candidate.
        runtime.configmap_store.update(
            HashMap::from([("default/ca".into(), k8s_openapi::api::core::v1::ConfigMap::default())]),
            &HashSet::new(),
        );
        for (step, raw_key, present, error) in [
            (1, "ca", true, None),
            (2, "default/ca", true, Some(FrontendCAResolutionError::InvalidReference)),
            (3, "ca", false, None),
            (4, "default/ca", false, Some(FrontendCAResolutionError::NotFound)),
            (5, "ca", true, None),
        ] {
            if present {
                let material = Secret {
                    metadata: ObjectMeta {
                        name: Some("ca".into()),
                        namespace: raw_key.contains('/').then(|| "default".into()),
                        uid: Some(format!("material-{step}")),
                        resource_version: Some(step.to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([
                        ("ca.crt".into(), ByteString(pem.as_bytes().to_vec())),
                        ("tls.crt".into(), ByteString(pem.as_bytes().to_vec())),
                        ("tls.key".into(), ByteString(key.serialize_pem().into_bytes())),
                    ])),
                    ..Default::default()
                };
                source_raw.upsert(raw_key.into(), Arc::new(material));
            } else {
                source_raw.remove(raw_key);
            }
            // Original raw-key observers remain notified; semantic-only non-Gateway observers do not gain a bare alias.
            let raw_dependent = ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "raw".into());
            let semantic_dependent =
                ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "semantic".into());
            runtime
                .secret_ref_manager
                .add_ref(raw_key.to_string(), raw_dependent.clone());
            if raw_key == "ca" {
                runtime
                    .secret_ref_manager
                    .add_ref("default/ca".into(), semantic_dependent.clone());
            }
            sources.on_event_key(raw_key.into()).await;
            work!(sources, source_raw, raw_key);
            work!(gateways, gateway_raw, owner_key);
            let published = gateways.get(owner_key).unwrap();
            assert_eq!(published.metadata.namespace.as_deref(), owner_namespace);
            let tls = published.spec.listeners.as_ref().unwrap()[0].tls.as_ref().unwrap();
            let outcome = &tls.resolved_frontend_ca_refs.as_ref().unwrap()[0];
            assert_eq!(outcome.error, error, "step {step}");
            assert_eq!(outcome.ca_certificate.is_some(), error.is_none());
            assert_eq!(outcome.source.namespace, "default");
            let serving = &tls.resolved_certificate_refs.as_ref().unwrap()[0];
            assert_eq!(serving.secret.is_some(), error.is_none());
            let backend = published.spec.tls.as_ref().unwrap().backend.as_ref().unwrap();
            assert_eq!(backend.resolved_client_certificate.is_some(), error.is_none());
            if let Some(secret) = serving.secret.as_ref() {
                assert_eq!(
                    secret.metadata.namespace.as_deref(),
                    if step == 3 { Some("default") } else { None }
                );
                assert_eq!(
                    secret.metadata.uid.as_deref(),
                    Some(if step == 3 {
                        "material-2"
                    } else if step == 5 {
                        "material-5"
                    } else {
                        "material-1"
                    })
                );
            }
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), plugins.workqueue().dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, "default/raw");
            plugins.workqueue().done(&item.key);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), plugins.workqueue().dequeue())
                    .await
                    .is_err()
            );
            runtime.secret_ref_manager.clear_value_refs(&raw_dependent);
            runtime.secret_ref_manager.clear_value_refs(&semantic_dependent);
        }
        assert!(runtime.frontend_ca_ref_manager.get_refs(&typed).contains(&dependent));
    }
}

#[tokio::test]
async fn native_gateway_bare_secret_events_wake_ordinary_only_tls_dependencies() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::Gateway;
    macro_rules! work {
        ($processor:expr, $raw:expr, $key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let source = $raw.get(&item.key).map(|value| (*value).clone());
            $processor
                .process_work_item(&item.key, source, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    let runtime = Arc::new(ControllerRuntime::default());
    let sources = Arc::new(ResourceProcessor::<Secret>::new(
        "Secret",
        32,
        Arc::new(SecretHandler::new()),
        runtime.clone(),
    ));
    let source_raw = Arc::new(LocalRawStore::new());
    sources.set_raw_store(source_raw.clone());
    runtime.processor_registry.register(sources.clone());
    let gateways = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        32,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let gateway_raw = Arc::new(LocalRawStore::new());
    gateways.set_raw_store(gateway_raw.clone());
    runtime.processor_registry.register(gateways.clone());
    let gateway: Gateway = serde_json::from_value(serde_json::json!({"metadata":{"name":"ordinary-edge"},"spec":{"gatewayClassName":"edge",
        "listeners":[{"name":"https","protocol":"HTTPS","port":443,"tls":{"certificateRefs":[{"name":"ordinary-cert"}]}}],
        "tls":{"backend":{"clientCertificateRef":{"name":"ordinary-cert"}}}}})).unwrap();
    gateway_raw.upsert("ordinary-edge".into(), Arc::new(gateway));
    gateways.on_event_key("ordinary-edge".into()).await;
    work!(gateways, gateway_raw, "ordinary-edge");
    let owner = ResourceRef::new(ResourceKind::Gateway, None, "ordinary-edge".into());
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&owner.key()),
        ["default/ordinary-cert"]
    );
    assert!(runtime
        .frontend_ca_ref_manager
        .get_dependencies(&owner.key())
        .is_empty());
    let first = gateways.get("ordinary-edge").unwrap();
    assert!(first.spec.tls.as_ref().unwrap().frontend.is_none());
    assert!(first
        .spec
        .tls
        .as_ref()
        .unwrap()
        .backend
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_none());
    assert!(first.spec.listeners.as_ref().unwrap()[0]
        .tls
        .as_ref()
        .unwrap()
        .resolved_certificate_refs
        .as_ref()
        .unwrap()[0]
        .secret
        .is_none());
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
    let pem = params.self_signed(&key).unwrap().pem();
    for (generation, present) in [(1, true), (2, false), (3, true)] {
        if present {
            source_raw.upsert(
                "ordinary-cert".into(),
                Arc::new(Secret {
                    metadata: ObjectMeta {
                        name: Some("ordinary-cert".into()),
                        namespace: None,
                        uid: Some(format!("actual-{generation}")),
                        resource_version: Some(generation.to_string()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([
                        ("tls.crt".into(), ByteString(pem.as_bytes().to_vec())),
                        ("tls.key".into(), ByteString(key.serialize_pem().into_bytes())),
                    ])),
                    ..Default::default()
                }),
            );
        } else {
            source_raw.remove("ordinary-cert");
        }
        sources.on_event_key("ordinary-cert".into()).await;
        work!(sources, source_raw, "ordinary-cert");
        // Only the ordinary semantic dependency can produce this Gateway work item.
        work!(gateways, gateway_raw, "ordinary-edge");
        let published = gateways.get("ordinary-edge").unwrap();
        assert!(published.metadata.namespace.is_none());
        assert!(published.spec.tls.as_ref().unwrap().frontend.is_none());
        let backend = published
            .spec
            .tls
            .as_ref()
            .unwrap()
            .backend
            .as_ref()
            .unwrap()
            .resolved_client_certificate
            .as_ref();
        let serving = published.spec.listeners.as_ref().unwrap()[0]
            .tls
            .as_ref()
            .unwrap()
            .resolved_certificate_refs
            .as_ref()
            .unwrap()[0]
            .secret
            .as_ref();
        assert_eq!(backend.is_some(), present);
        assert_eq!(serving.is_some(), present);
        for material in [backend, serving].into_iter().flatten() {
            assert!(material.metadata.namespace.is_none());
            assert_eq!(
                material.metadata.uid.as_deref(),
                Some(format!("actual-{generation}").as_str())
            );
        }
        assert_eq!(
            runtime.secret_ref_manager.get_dependencies(&owner.key()),
            ["default/ordinary-cert"]
        );
        assert!(runtime
            .frontend_ca_ref_manager
            .get_dependencies(&owner.key())
            .is_empty());
    }
}

#[tokio::test]
async fn backend_tls_secret_events_recover_bare_material_and_ambiguity_without_typed_ca_edges() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::BackendTlsPolicyHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::{BackendTLSPolicy, EdgionPlugins};
    use std::collections::BTreeMap;
    struct Observer;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for Observer {}
    macro_rules! work {
        ($processor:expr,$raw:expr,$key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let value = $raw.get(&item.key).map(|v| (*v).clone());
            $processor
                .process_work_item(&item.key, value, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    for namespace in [None, Some("default")] {
        let runtime = Arc::new(ControllerRuntime::default());
        let sources = Arc::new(ResourceProcessor::<Secret>::new(
            "Secret",
            32,
            Arc::new(SecretHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        sources.set_raw_store(raw.clone());
        runtime.processor_registry.register(sources.clone());
        let policies = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
            "BackendTLSPolicy",
            32,
            Arc::new(BackendTlsPolicyHandler::default()),
            runtime.clone(),
        ));
        let owners = Arc::new(LocalRawStore::new());
        policies.set_raw_store(owners.clone());
        runtime.processor_registry.register(policies.clone());
        let observers = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            32,
            Arc::new(Observer),
            runtime.clone(),
        ));
        runtime.processor_registry.register(observers.clone());
        let key = if namespace.is_some() {
            "default/policy"
        } else {
            "policy"
        };
        let mut policy:BackendTLSPolicy=serde_json::from_value(serde_json::json!({"metadata":{"name":"policy","namespace":namespace},"spec":{
            "targetRefs":[{"group":"","kind":"Service","name":"backend"}],"validation":{"hostname":"backend.example","caCertificateRefs":[{"group":"","kind":"Secret","name":"ca"}]}}})).unwrap();
        policy.spec.options = Some([("edgion.io/client-certificate-ref".into(), "ca".into())].into());
        owners.upsert(key.into(), Arc::new(policy.clone()));
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        let dependent = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            namespace.map(str::to_owned),
            "policy".into(),
        );
        assert_eq!(
            runtime.secret_ref_manager.get_dependencies(&dependent.key()),
            ["default/ca"]
        );
        assert!(runtime
            .frontend_ca_ref_manager
            .get_dependencies(&dependent.key())
            .is_empty());
        for (generation, (source_key, present, resolved)) in [
            ("ca", true, true),
            ("ca", false, false),
            ("ca", true, true),
            ("default/ca", true, false),
            ("default/ca", false, true),
        ]
        .into_iter()
        .enumerate()
        {
            if present {
                raw.upsert(
                    source_key.into(),
                    Arc::new(Secret {
                        metadata: ObjectMeta {
                            name: Some("ca".into()),
                            namespace: source_key.contains('/').then(|| "default".into()),
                            uid: Some(format!("actual-{generation}")),
                            ..Default::default()
                        },
                        data: Some(BTreeMap::from([
                            ("ca.crt".into(), k8s_openapi::ByteString(b"ca".to_vec())),
                            ("tls.crt".into(), k8s_openapi::ByteString(b"crt".to_vec())),
                            ("tls.key".into(), k8s_openapi::ByteString(b"key".to_vec())),
                        ])),
                        ..Default::default()
                    }),
                );
            } else {
                raw.remove(source_key);
            }
            let raw_observer = ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "raw".into());
            let semantic_observer =
                ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "semantic".into());
            runtime
                .secret_ref_manager
                .add_ref(source_key.to_string(), raw_observer.clone());
            if source_key == "ca" {
                runtime
                    .secret_ref_manager
                    .add_ref("default/ca".into(), semantic_observer.clone());
            }
            sources.on_event_key(source_key.into()).await;
            work!(sources, raw, source_key);
            // This work item must originate from the ordinary material notification.
            work!(policies, owners, key);
            let actual = policies.get(key).unwrap();
            assert_eq!(actual.spec.resolved_ca_certificates.is_some(), resolved);
            assert_eq!(actual.spec.resolved_target_class.is_some(), resolved);
            assert_eq!(actual.spec.resolved_client_certificate.is_some(), resolved);
            if let Some(material) = actual.spec.resolved_ca_certificates.as_ref().and_then(|m| m.first()) {
                assert!(material.metadata.namespace.is_none());
            }
            assert!(runtime
                .frontend_ca_ref_manager
                .get_dependencies(&dependent.key())
                .is_empty());
            let queue = observers.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, "default/raw");
            queue.done(&item.key);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), queue.dequeue())
                    .await
                    .is_err()
            );
            runtime.secret_ref_manager.clear_value_refs(&raw_observer);
            runtime.secret_ref_manager.clear_value_refs(&semantic_observer);
        }
        policy.spec.validation.ca_certificate_refs.as_mut().unwrap()[0].name = "replacement".into();
        policy.spec.options = None;
        owners.upsert(key.into(), Arc::new(policy));
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        assert_eq!(
            runtime.secret_ref_manager.get_dependencies(&dependent.key()),
            ["default/replacement"]
        );
        raw.remove("ca");
        sources.on_event_key("ca".into()).await;
        work!(sources, raw, "ca");
        // Observe beyond the configured coalescing delay before any owner delete can mask it.
        let quiet_window = std::time::Duration::from_secs(3);
        assert!(
            quiet_window > crate::conf_mgr::sync_runtime::workqueue::WorkqueueConfig::default().default_requeue_delay
        );
        assert!(tokio::time::timeout(quiet_window, policies.workqueue().dequeue())
            .await
            .is_err());
        owners.remove(key);
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        assert!(runtime.secret_ref_manager.get_dependencies(&dependent.key()).is_empty());
    }
}

#[tokio::test]
async fn health_tls_secret_events_recover_bare_material_and_ambiguity_without_typed_ca_edges() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, RawStore};
    use edgion_resources::{EdgionBackendTrafficPolicy, EdgionPlugins};
    use std::collections::BTreeMap;
    struct Observer;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for Observer {}
    macro_rules! work {
        ($processor:expr,$raw:expr,$key:expr) => {{
            let queue = $processor.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, $key);
            let value = $raw.get(&item.key).map(|v| (*v).clone());
            $processor
                .process_work_item(&item.key, value, None, item.trigger_chain)
                .await;
            queue.done(&item.key);
        }};
    }
    for namespace in [None, Some("default")] {
        let runtime = Arc::new(ControllerRuntime::default());
        let sources = Arc::new(ResourceProcessor::<Secret>::new(
            "Secret",
            32,
            Arc::new(SecretHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        sources.set_raw_store(raw.clone());
        runtime.processor_registry.register(sources.clone());
        let policies = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            32,
            Arc::new(EdgionBackendTrafficPolicyHandler::new(
                "test.edgion.io/controller".into(),
            )),
            runtime.clone(),
        ));
        let owners = Arc::new(LocalRawStore::new());
        policies.set_raw_store(owners.clone());
        runtime.processor_registry.register(policies.clone());
        let observers = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            32,
            Arc::new(Observer),
            runtime.clone(),
        ));
        runtime.processor_registry.register(observers.clone());
        let key = if namespace.is_some() {
            "default/policy"
        } else {
            "policy"
        };
        let mut policy: EdgionBackendTrafficPolicy = serde_json::from_value(serde_json::json!({"metadata":{"name":"policy","namespace":namespace},"spec":{
            "targetRefs":[{"group":"","kind":"Service","name":"backend"}],"healthCheck":{"active":{"type":"https","tls":{"validation":{"hostname":"backend.example","caCertificateRefs":[{"kind":"Secret","name":"ca"}]}}}}}})).unwrap();
        let pair = rcgen::generate_simple_self_signed(vec!["backend.example".into()]).unwrap();
        let pem = pair.cert.pem();
        policy
            .spec
            .health_check
            .as_mut()
            .unwrap()
            .active
            .as_mut()
            .unwrap()
            .tls
            .as_mut()
            .unwrap()
            .client_certificate_ref = Some(serde_json::from_value(serde_json::json!({"name":"ca"})).unwrap());
        owners.upsert(key.into(), Arc::new(policy.clone()));
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        let dependent = ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            namespace.map(str::to_owned),
            "policy".into(),
        );
        assert_eq!(
            runtime.secret_ref_manager.get_dependencies(&dependent.key()),
            ["default/ca"]
        );
        assert!(runtime
            .frontend_ca_ref_manager
            .get_dependencies(&dependent.key())
            .is_empty());
        for (generation, (source_key, present, resolved)) in [
            ("ca", true, true),
            ("ca", false, false),
            ("ca", true, true),
            ("default/ca", true, false),
            ("default/ca", false, true),
        ]
        .into_iter()
        .enumerate()
        {
            if present {
                raw.upsert(
                    source_key.into(),
                    Arc::new(Secret {
                        type_: Some("kubernetes.io/tls".into()),
                        metadata: ObjectMeta {
                            name: Some("ca".into()),
                            namespace: source_key.contains('/').then(|| "default".into()),
                            uid: Some(format!("actual-{generation}")),
                            ..Default::default()
                        },
                        data: Some(BTreeMap::from([
                            ("ca.crt".into(), k8s_openapi::ByteString(pem.as_bytes().to_vec())),
                            ("tls.crt".into(), k8s_openapi::ByteString(pem.as_bytes().to_vec())),
                            (
                                "tls.key".into(),
                                k8s_openapi::ByteString(pair.signing_key.serialize_pem().into_bytes()),
                            ),
                        ])),
                        ..Default::default()
                    }),
                );
            } else {
                raw.remove(source_key);
            }
            let raw_observer = ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "raw".into());
            let semantic_observer =
                ResourceRef::new(ResourceKind::EdgionPlugins, Some("default".into()), "semantic".into());
            runtime
                .secret_ref_manager
                .add_ref(source_key.to_string(), raw_observer.clone());
            if source_key == "ca" {
                runtime
                    .secret_ref_manager
                    .add_ref("default/ca".into(), semantic_observer.clone());
            }
            sources.on_event_key(source_key.into()).await;
            work!(sources, raw, source_key);
            // This work item must originate from the ordinary material notification.
            work!(policies, owners, key);
            let actual = policies.get(key).unwrap();
            assert_eq!(
                actual
                    .spec
                    .health_check
                    .as_ref()
                    .unwrap()
                    .active
                    .as_ref()
                    .unwrap()
                    .resolved_ca_certificates
                    .is_some(),
                resolved
            );
            assert_eq!(
                actual
                    .spec
                    .health_check
                    .as_ref()
                    .unwrap()
                    .active
                    .as_ref()
                    .unwrap()
                    .resolved_tls_error
                    .is_none(),
                resolved
            );
            assert_eq!(
                actual
                    .spec
                    .health_check
                    .as_ref()
                    .unwrap()
                    .active
                    .as_ref()
                    .unwrap()
                    .resolved_client_certificate
                    .is_some(),
                resolved
            );
            if let Some(material) = actual
                .spec
                .health_check
                .as_ref()
                .unwrap()
                .active
                .as_ref()
                .unwrap()
                .resolved_ca_certificates
                .as_ref()
                .and_then(|m| m.first())
            {
                assert!(material.metadata.namespace.is_none());
            }
            assert!(runtime
                .frontend_ca_ref_manager
                .get_dependencies(&dependent.key())
                .is_empty());
            let queue = observers.workqueue();
            let item = tokio::time::timeout(std::time::Duration::from_secs(3), queue.dequeue())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item.key, "default/raw");
            queue.done(&item.key);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), queue.dequeue())
                    .await
                    .is_err()
            );
            runtime.secret_ref_manager.clear_value_refs(&raw_observer);
            runtime.secret_ref_manager.clear_value_refs(&semantic_observer);
        }
        let active = policy.spec.health_check.as_mut().unwrap().active.as_mut().unwrap();
        active
            .tls
            .as_mut()
            .unwrap()
            .validation
            .as_mut()
            .unwrap()
            .ca_certificate_refs
            .as_mut()
            .unwrap()[0]
            .name = "replacement".into();
        active.tls.as_mut().unwrap().client_certificate_ref = None;
        owners.upsert(key.into(), Arc::new(policy));
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        assert_eq!(
            runtime.secret_ref_manager.get_dependencies(&dependent.key()),
            ["default/replacement"]
        );
        raw.remove("ca");
        sources.on_event_key("ca".into()).await;
        work!(sources, raw, "ca");
        // Observe beyond the configured coalescing delay before any owner delete can mask it.
        let quiet_window = std::time::Duration::from_secs(3);
        assert!(
            quiet_window > crate::conf_mgr::sync_runtime::workqueue::WorkqueueConfig::default().default_requeue_delay
        );
        assert!(tokio::time::timeout(quiet_window, policies.workqueue().dequeue())
            .await
            .is_err());
        owners.remove(key);
        policies.on_event_key(key.into()).await;
        work!(policies, owners, key);
        assert!(runtime.secret_ref_manager.get_dependencies(&dependent.key()).is_empty());
    }
}
