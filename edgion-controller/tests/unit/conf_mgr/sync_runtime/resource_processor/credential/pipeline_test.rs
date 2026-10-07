use super::*;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::credentials::BasicAuthResolver;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
use edgion_resources::ResourceKind;

fn bound_plugins(reference: serde_json::Value) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
        "metadata":{"name":"auth", "namespace":"source"},
        "spec":{"requestPlugins":[{"type":"Csrf", "config":{"secretRef": reference}}]}
    }))
    .unwrap()
}

#[test]
fn authorized_secret_debug_masks_direct_nested_and_alternate_material() {
    let authorized = AuthorizedSecret {
        secret: Secret {
            data: Some(BTreeMap::from([(
                "key".to_string(),
                ByteString(b"private-material".to_vec()),
            )])),
            string_data: Some(BTreeMap::from([("key".to_string(), "private-string-data".to_string())])),
            ..Default::default()
        },
        authorization_revision: "private-authority".to_string(),
    };
    for debug in [
        format!("{authorized:?}"),
        format!("{authorized:#?}"),
        format!("{:?}", BoundFetchOutcome::Ok(&authorized)),
        format!("{:#?}", BoundFetchOutcome::Ok(&authorized)),
    ] {
        assert!(!debug.contains("private-"));
        assert!(!debug.contains("data"));
        assert!(!debug.contains("cHJpdmF0Z"));
    }
}

#[test]
fn bound_ca_configmap_uses_its_exact_kind_and_separate_dependency_key() {
    for (reference, expected) in [
        (
            serde_json::json!({"group":"core", "kind":"ConfigMap", "name":"key"}),
            "ok",
        ),
        (
            serde_json::json!({"group":"foreign.example", "kind":"ConfigMap", "name":"key"}),
            "invalid",
        ),
        (
            serde_json::json!({"kind":"ConfigMap", "namespace":"", "name":"key"}),
            "invalid",
        ),
        (
            serde_json::json!({"kind":"ConfigMap", "namespace":"target", "name":"key"}),
            "denied",
        ),
    ] {
        let (key, secret) = make_secret_with_type("source", "key", None, &[("ca.crt", b"secret-ca")]);
        let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
        runtime.configmap_store.replace_all(HashMap::from([(
            "source/key".to_string(),
            ConfigMap {
                metadata: ObjectMeta {
                    name: Some("key".to_string()),
                    namespace: Some("source".to_string()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([("ca.crt".to_string(), "configmap-ca".to_string())])),
                ..Default::default()
            },
        )]));
        let mut owner = bound_plugins(serde_json::json!({"name":"unused"}));
        owner.spec.request_plugins = serde_json::from_value(serde_json::json!([{
            "type":"LdapAuth", "config":{"ldapHost":"ldap.example", "baseDn":"dc=example", "attribute":"uid",
                "tls":{"validation":{"caCertificateRefs":[reference]}}}
        }]))
        .unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let tls = references.tls().unwrap();
        let token = tls.ca_certificates().next().unwrap();
        let dependency = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("source".to_string()),
            "auth".to_string(),
        );
        let readonly = fetch_bound_configmap_readonly(&ctx, &dependency, token);
        assert!(runtime
            .secret_ref_manager
            .get_dependencies(&dependency.key())
            .is_empty());
        let result = fetch_bound_configmap(&ctx, &dependency, token);
        match expected {
            "ok" => {
                assert!(matches!(readonly, BoundFetchOutcome::Ok(_)));
                let BoundFetchOutcome::Ok(configmap) = result else {
                    panic!("expected ConfigMap");
                };
                assert_eq!(configmap.data.unwrap()["ca.crt"], "configmap-ca");
                assert_eq!(
                    runtime.secret_ref_manager.get_dependencies(&dependency.key()),
                    ["ConfigMap/source/key"]
                );
                assert!(matches!(
                    fetch_bound_secret(&ctx, &dependency, token),
                    BoundFetchOutcome::Invalid(ReferenceError::UnsupportedTarget)
                ));
            }
            "invalid" => {
                assert!(matches!(result, BoundFetchOutcome::Invalid(_)));
                assert!(runtime
                    .secret_ref_manager
                    .get_dependencies(&dependency.key())
                    .is_empty());
            }
            _ => {
                assert!(matches!(
                    result,
                    BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing { .. })
                ));
                assert_eq!(
                    runtime.secret_ref_manager.get_dependencies(&dependency.key()),
                    ["ConfigMap/target/key"]
                );
                assert!(runtime.cross_ns_ref_mgr.get_refs("target").contains(&dependency));
            }
        }
    }
}

#[test]
fn bound_invalid_identity_never_registers_or_uses_same_name_secret() {
    let (key, secret) = make_secret_with_type("source", "key", None, &[("key", b"private")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let dependency = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("source".to_string()),
        "auth".to_string(),
    );
    for reference in [
        serde_json::json!({"group":"foreign.example", "name":"key"}),
        serde_json::json!({"kind":"ConfigMap", "name":"key"}),
        serde_json::json!({"kind":"", "name":"key"}),
        serde_json::json!({"namespace":"", "name":"key"}),
        serde_json::json!({"name":"invalid/name"}),
    ] {
        let owner = bound_plugins(reference);
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let token = references.csrf_secret().unwrap();
        assert!(matches!(
            fetch_bound_secret(&ctx, &dependency, token),
            BoundFetchOutcome::Invalid(_)
        ));
        assert!(runtime
            .secret_ref_manager
            .get_dependencies(&dependency.key())
            .is_empty());
    }
}

#[test]
fn bound_custom_core_alias_uses_canonical_secret_and_actual_dependency_owner() {
    let (key, secret) = make_secret_with_type("source", "key", None, &[("key", b"private")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let owner = bound_plugins(serde_json::json!({"group":"core", "kind":"Secret", "name":"key"}));
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let token = references.csrf_secret().unwrap();
    let wrong = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("source".to_string()),
        "other".to_string(),
    );
    assert!(matches!(
        fetch_bound_secret(&ctx, &wrong, token),
        BoundFetchOutcome::Invalid(ReferenceError::StaleBinding)
    ));
    assert!(runtime.secret_ref_manager.get_dependencies(&wrong.key()).is_empty());
    let dependency = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("source".to_string()),
        "auth".to_string(),
    );
    let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(&ctx, &dependency, token) else {
        panic!("expected canonical Secret lookup");
    };
    assert_eq!(authorized.authorization_revision, "same-namespace");
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&dependency.key()),
        ["source/key"]
    );
}

#[test]
fn denied_bound_reference_retains_recovery_edge_but_has_no_authority_revision() {
    let (ctx, runtime) = make_ctx(HashMap::new());
    let owner = bound_plugins(serde_json::json!({"namespace":"target", "name":"key"}));
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let token = references.csrf_secret().unwrap();
    let dependency = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        Some("source".to_string()),
        "auth".to_string(),
    );
    assert!(matches!(
        fetch_bound_secret(&ctx, &dependency, token),
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing { .. })
    ));
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&dependency.key()),
        ["target/key"]
    );
    assert_eq!(
        runtime
            .ref_grant_store
            .query_authorization_revision(token.validated().unwrap().grant_query()),
        None
    );
}

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
