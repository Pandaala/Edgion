use super::*;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::ResourceKind;

fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
    let mut bmap = BTreeMap::new();
    for (k, v) in data {
        bmap.insert(k.to_string(), ByteString(v.to_vec()));
    }
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        data: Some(bmap),
        ..Default::default()
    };
    (format!("{}/{}", namespace, name), secret)
}

fn owner() -> ResourceRef {
    ResourceRef::new(
        ResourceKind::LinkSys,
        Some("prod".to_string()),
        "my-webhook".to_string(),
    )
}

/// Build a `HandlerContext` pre-seeded with a single Secret.
fn test_ctx_with_secret(ns_name: &str, key: &str, value: &str) -> HandlerContext {
    let (ns, name) = ns_name.split_once('/').expect("ns/name expected");
    let (secret_key, secret) = make_secret(ns, name, &[(key, value.as_bytes())]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(secret_key, secret)]));
    ctx
}

#[tokio::test]
async fn template_secret_dependencies_survive_missing_and_rotation() {
    let (ctx, runtime) = make_ctx(HashMap::new());
    let owner = owner();
    let refs = ["prod/token:key".to_string()];
    let (values, errors) = resolve_request_secrets(&ctx, &owner, ResourceKind::LinkSys, &refs).await;
    assert!(values.is_empty());
    assert!(matches!(errors.as_slice(), [ResolvedRefsError::SecretNotFound { .. }]));
    assert!(ctx.secret_ref_manager().get_refs("prod/token").contains(&owner));

    for value in ["first", "rotated"] {
        let (key, secret) = make_secret("prod", "token", &[("key", value.as_bytes())]);
        runtime.secret_store.replace_all(HashMap::from([(key, secret)]));
        let (values, errors) = resolve_request_secrets(&ctx, &owner, ResourceKind::LinkSys, &refs).await;
        assert!(errors.is_empty());
        assert_eq!(values["prod/token:key"], value);
        assert!(request_secret_errors_readonly(&ctx, &owner, ResourceKind::LinkSys, &refs).is_empty());
    }
    runtime.secret_store.replace_all(HashMap::new());
    assert!(matches!(
        request_secret_errors_readonly(&ctx, &owner, ResourceKind::LinkSys, &refs).as_slice(),
        [ResolvedRefsError::SecretNotFound { .. }]
    ));
}

#[test]
fn template_secret_status_read_does_not_register_dependencies() {
    let (ctx, _) = make_ctx(HashMap::new());
    let owner = owner();
    let refs = ["prod/missing:key".to_string()];
    assert!(!request_secret_errors_readonly(&ctx, &owner, ResourceKind::LinkSys, &refs).is_empty());
    assert!(!ctx.secret_ref_manager().get_refs("prod/missing").contains(&owner));
}

/// A missing key in a present Secret surfaces as `DecodeError` (the Secret
/// object exists; `read_secret_key` returns `Err` for the absent key).
/// A missing Secret object surfaces as `SecretNotFound`.
#[tokio::test]
async fn resolve_request_secrets_reports_missing() {
    let ctx = test_ctx_with_secret("prod/s", "key", "v");
    let (map, errs) = resolve_request_secrets(
        &ctx,
        &owner(),
        ResourceKind::LinkSys,
        &["prod/s:key".to_string(), "prod/s:absent".to_string()],
    )
    .await;

    // The present key must resolve correctly.
    assert_eq!(map.get("prod/s:key").map(String::as_str), Some("v"));

    // The absent key surfaces as a DecodeError (key not found in secret).
    assert!(
        errs.iter().any(|e| matches!(e, ResolvedRefsError::DecodeError { .. })),
        "expected a DecodeError for the absent key, got: {errs:?}"
    );
}

#[tokio::test]
async fn resolve_request_secrets_happy_path_two_keys() {
    let (key_a, secret_a) = make_secret("prod", "creds", &[("token", b"abc123"), ("secret", b"xyz789")]);
    let (ctx, _) = make_ctx(HashMap::from([(key_a, secret_a)]));

    let (map, errs) = resolve_request_secrets(
        &ctx,
        &owner(),
        ResourceKind::LinkSys,
        &["prod/creds:token".to_string(), "prod/creds:secret".to_string()],
    )
    .await;

    assert!(errs.is_empty(), "expected no errors, got: {errs:?}");
    assert_eq!(map.get("prod/creds:token").map(String::as_str), Some("abc123"));
    assert_eq!(map.get("prod/creds:secret").map(String::as_str), Some("xyz789"));
}

#[tokio::test]
async fn resolve_request_secrets_missing_secret_returns_secret_not_found() {
    let (ctx, _) = make_ctx(HashMap::new());

    let (map, errs) =
        resolve_request_secrets(&ctx, &owner(), ResourceKind::LinkSys, &["prod/missing:key".to_string()]).await;

    assert!(map.is_empty());
    assert!(
        errs.iter()
            .any(|e| matches!(e, ResolvedRefsError::SecretNotFound { .. })),
        "expected SecretNotFound, got: {errs:?}"
    );
}

#[tokio::test]
async fn resolve_request_secrets_malformed_ref_returns_decode_error() {
    let (ctx, _) = make_ctx(HashMap::new());

    // "nons:key" is malformed — missing the "namespace/" prefix required
    // by parse_secret_ref.
    let (map, errs) = resolve_request_secrets(&ctx, &owner(), ResourceKind::LinkSys, &["nons:key".to_string()]).await;

    assert!(map.is_empty());
    assert!(
        errs.iter()
            .any(|e| matches!(e, ResolvedRefsError::DecodeError { key: "secretRef", .. })),
        "expected DecodeError for malformed ref, got: {errs:?}"
    );
}
