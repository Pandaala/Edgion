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
