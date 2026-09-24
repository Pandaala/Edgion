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
