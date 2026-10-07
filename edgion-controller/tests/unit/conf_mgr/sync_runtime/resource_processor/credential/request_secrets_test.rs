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

fn with_templates<R>(
    owner: &ResourceRef,
    names: &[String],
    run: impl FnOnce(&edgion_resources::resources::refs::link_sys_fields::LinkSysReferences<'_>) -> R,
) -> R {
    let request = names
        .iter()
        .map(|name| format!("${{secretRef:{name}}}"))
        .collect::<Vec<_>>()
        .join(" ");
    let resource: edgion_resources::LinkSys = serde_json::from_value(serde_json::json!({
        "metadata":{"name":owner.name,"namespace":owner.namespace},
        "spec":{"type":"webhook","config":{"target":{"url":"https://hook.example"},"request":{"path":{"template":request}}}}
    })).unwrap();
    let prepared =
        edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences::from_link_sys(&resource);
    let binding = prepared.bind(&resource).unwrap();
    let references = binding.references().unwrap();
    run(&references)
}

fn resolve_templates(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    names: &[String],
) -> (BTreeMap<String, String>, Vec<ResolvedRefsError>) {
    with_templates(owner, names, |references| {
        resolve_request_secrets(ctx, owner, references.request_secrets())
    })
}

fn template_errors(ctx: &HandlerContext, owner: &ResourceRef, names: &[String]) -> Vec<ResolvedRefsError> {
    with_templates(owner, names, |references| {
        request_secret_errors_readonly(ctx, owner, references.request_secrets())
    })
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
    let (values, errors) = resolve_templates(&ctx, &owner, &refs);
    assert!(values.is_empty());
    assert!(matches!(errors.as_slice(), [ResolvedRefsError::SecretNotFound { .. }]));
    assert!(ctx.secret_ref_manager().get_refs("prod/token").contains(&owner));

    for value in ["first", "rotated"] {
        let (key, secret) = make_secret("prod", "token", &[("key", value.as_bytes())]);
        runtime.secret_store.replace_all(HashMap::from([(key, secret)]));
        let (values, errors) = resolve_templates(&ctx, &owner, &refs);
        assert!(errors.is_empty());
        assert_eq!(values["prod/token:key"], value);
        assert!(template_errors(&ctx, &owner, &refs).is_empty());
    }
    runtime.secret_store.replace_all(HashMap::new());
    assert!(matches!(
        template_errors(&ctx, &owner, &refs).as_slice(),
        [ResolvedRefsError::SecretNotFound { .. }]
    ));
}

#[test]
fn template_secret_status_read_does_not_register_dependencies() {
    let (ctx, _) = make_ctx(HashMap::new());
    let owner = owner();
    let refs = ["prod/missing:key".to_string()];
    assert!(!template_errors(&ctx, &owner, &refs).is_empty());
    assert!(!ctx.secret_ref_manager().get_refs("prod/missing").contains(&owner));
}

/// A missing key in a present Secret surfaces as `DecodeError` (the Secret
/// object exists; `read_secret_key` returns `Err` for the absent key).
/// A missing Secret object surfaces as `SecretNotFound`.
#[tokio::test]
async fn resolve_request_secrets_reports_missing() {
    let ctx = test_ctx_with_secret("prod/s", "key", "v");
    let (map, errs) = resolve_templates(&ctx, &owner(), &["prod/s:key".to_string(), "prod/s:absent".to_string()]);

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

    let (map, errs) = resolve_templates(
        &ctx,
        &owner(),
        &["prod/creds:token".to_string(), "prod/creds:secret".to_string()],
    );

    assert!(errs.is_empty(), "expected no errors, got: {errs:?}");
    assert_eq!(map.get("prod/creds:token").map(String::as_str), Some("abc123"));
    assert_eq!(map.get("prod/creds:secret").map(String::as_str), Some("xyz789"));
}

#[tokio::test]
async fn resolve_request_secrets_missing_secret_returns_secret_not_found() {
    let (ctx, _) = make_ctx(HashMap::new());

    let (map, errs) = resolve_templates(&ctx, &owner(), &["prod/missing:key".to_string()]);

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
    let (map, errs) = resolve_templates(&ctx, &owner(), &["nons:key".to_string()]);

    assert!(map.is_empty());
    assert!(
        errs.iter()
            .any(|e| matches!(e, ResolvedRefsError::DecodeError { key: "secretRef", .. })),
        "expected DecodeError for malformed ref, got: {errs:?}"
    );
}

#[test]
fn owning_template_preserves_empty_string_data_and_lexical_components() {
    let (key, mut secret) = make_secret("prod", "nested/token:part", &[("empty", b""), ("bad", &[255])]);
    secret.string_data = Some(BTreeMap::from([("fallback".to_string(), "value".to_string())]));
    let (ctx, _) = make_ctx(HashMap::from([(key, secret)]));
    let names = [
        "prod/nested/token:part:empty",
        "prod/nested/token:part:fallback",
        "prod/nested/token:part:bad",
    ]
    .map(str::to_string);
    let (values, errors) = resolve_templates(&ctx, &owner(), &names);
    assert_eq!(values[&names[0]], "");
    assert_eq!(values[&names[1]], "value");
    assert!(matches!(errors.as_slice(), [ResolvedRefsError::DecodeError { .. }]));
    assert_eq!(template_errors(&ctx, &owner(), &names).len(), 1);
    assert!(ctx
        .secret_ref_manager()
        .get_refs("prod/nested/token:part")
        .contains(&owner()));
}

#[test]
fn malformed_and_stale_owner_templates_never_register_recovery_edges() {
    let (ctx, runtime) = make_ctx(HashMap::new());
    let names = ["token:key", "/token:key", "prod/:key", "prod/token:"].map(str::to_string);
    let (_, errors) = resolve_templates(&ctx, &owner(), &names);
    assert_eq!(errors.len(), 4);
    assert!(errors
        .iter()
        .all(|error| matches!(error, ResolvedRefsError::DecodeError { .. })));
    assert!(runtime.secret_ref_manager.get_dependencies(&owner().key()).is_empty());
    let wrong_owner = ResourceRef::new(ResourceKind::EdgionPlugins, Some("prod".into()), "my-webhook".into());
    let names = ["prod/token:key".to_string()];
    let (_, errors) = with_templates(&owner(), &names, |references| {
        resolve_request_secrets(&ctx, &wrong_owner, references.request_secrets())
    });
    assert!(matches!(
        errors.as_slice(),
        [ResolvedRefsError::InvalidReference { .. }]
    ));
    assert!(runtime
        .secret_ref_manager
        .get_dependencies(&wrong_owner.key())
        .is_empty());
}

#[test]
fn template_denial_grant_recovery_and_revocation_use_actual_source() {
    let (key, secret) = make_secret("policy", "token", &[("key", b"value")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let names = ["policy/token:key".to_string()];
    let (_, errors) = resolve_templates(&ctx, &owner(), &names);
    assert!(matches!(
        errors.as_slice(),
        [ResolvedRefsError::RefNotPermitted {
            from_kind: "LinkSys",
            ..
        }]
    ));
    assert!(ctx.secret_ref_manager().get_refs("policy/token").contains(&owner()));
    let grant = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"allow","namespace":"policy"},"spec":{
            "from":[{"group":"edgion.io","kind":"LinkSys","namespace":"prod"}],
            "to":[{"group":"","kind":"Secret","name":"token"}]}}))
    .unwrap();
    runtime
        .ref_grant_store
        .upsert("policy/allow".to_string(), Arc::new(grant));
    let (values, errors) = resolve_templates(&ctx, &owner(), &names);
    assert!(errors.is_empty());
    assert_eq!(values[&names[0]], "value");
    runtime.ref_grant_store.remove("policy/allow");
    assert!(matches!(
        template_errors(&ctx, &owner(), &names).as_slice(),
        [ResolvedRefsError::RefNotPermitted { .. }]
    ));
}
