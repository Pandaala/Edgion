use super::*;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::credentials::BasicAuthResolver;
use edgion_resources::ResourceKind;

fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_secret_with_type(namespace: &str, name: &str, type_: Option<&str>, data: &[(&str, &[u8])]) -> (String, Secret) {
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
        type_: type_.map(str::to_string),
        ..Default::default()
    };
    (format!("{}/{}", namespace, name), secret)
}

fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
    make_secret_with_type(namespace, name, None, data)
}

fn owner_ref(ns: &str, name: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::LinkSys, Some(ns.to_string()), name.to_string())
}

fn secret_ref(name: &str, namespace: Option<&str>) -> SecretObjectReference {
    SecretObjectReference {
        group: None,
        kind: None,
        name: name.to_string(),
        namespace: namespace.map(|s| s.to_string()),
    }
}

#[tokio::test]
async fn test_secret_ref_manager_registration() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let _ = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    let refs = ctx.secret_ref_manager().get_refs("default/my-secret");
    assert!(refs.contains(&owner), "owner must be registered, got {refs:?}");
}

#[tokio::test]
async fn test_secret_not_found_returns_pending() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("missing-secret", None);

    let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            assert_eq!(namespace, "default");
            assert_eq!(name, "missing-secret");
        }
        other => panic!("expected Pending(SecretNotFound), got {other:?}"),
    }
}

#[tokio::test]
async fn test_missing_keys_invalid() {
    let (key, secret) = make_secret("default", "my-secret", &[("username", b"alice")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
            assert!(keys.contains(&"password"), "got {keys:?}");
        }
        other => panic!("expected Invalid(MissingKeys), got {other:?}"),
    }
}

#[tokio::test]
async fn test_happy_path_returns_ok() {
    let (key, secret) = make_secret(
        "default",
        "my-secret",
        &[("username", b"alice"), ("password", b"s3cr3t")],
    );
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Ok(m) => {
            assert_eq!(m.username.as_deref(), Some("alice"));
            assert_eq!(m.password, "s3cr3t");
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resolve_secret_returns_secret_object() {
    let (key, secret) = make_secret("default", "my-secret", &[("password", b"s3cr3t")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Ok(s) => {
            assert_eq!(s.metadata.name.as_deref(), Some("my-secret"));
            let data = s.data.expect("data");
            assert!(data.contains_key("password"));
        }
        other => panic!("expected Ok(Secret), got {other:?}"),
    }
}

#[tokio::test]
async fn test_basic_auth_accepts_k8s_basic_auth_type() {
    let (key, secret) = make_secret_with_type(
        "default",
        "my-secret",
        Some("kubernetes.io/basic-auth"),
        &[("password", b"s3cr3t")],
    );
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;
    assert!(
        matches!(outcome, ResolveOutcome::Ok(_)),
        "basic-auth type must be accepted, got {outcome:?}"
    );
}

#[tokio::test]
async fn test_basic_auth_rejects_tls_type() {
    let (key, secret) = make_secret_with_type(
        "default",
        "my-secret",
        Some("kubernetes.io/tls"),
        &[("password", b"s3cr3t")],
    );
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", None);

    let outcome = resolve_secret(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;
    match outcome {
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, .. }) => {
            assert_eq!(got, "kubernetes.io/tls");
        }
        other => panic!("expected InvalidSecretType, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cross_ns_no_grant_returns_pending() {
    let (key, secret) = make_secret("other-ns", "my-secret", &[("password", b"s3cr3t")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("my-secret", Some("other-ns"));

    let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => {
            assert_eq!(from_ns, "default");
            assert_eq!(to_ns, "other-ns");
            assert_eq!(secret_name, "my-secret");
        }
        other => panic!("expected Pending(ReferenceGrantMissing), got {other:?}"),
    }
}

#[tokio::test]
async fn test_secret_ref_manager_registered_when_cross_ns_grant_missing() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let owner = owner_ref("default", "my-linksys");
    let sref = secret_ref("creds", Some("other-ns"));

    let outcome = resolve_credential(&ctx, &owner, "edgion.io", "LinkSys", &sref, &BasicAuthResolver).await;

    match outcome {
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing { .. }) => {}
        other => panic!("expected Pending(ReferenceGrantMissing), got {other:?}"),
    }

    let refs = ctx.secret_ref_manager().get_refs("other-ns/creds");
    assert!(
        refs.contains(&owner),
        "owner must be registered for Secret wakeups, got {refs:?}"
    );
}
