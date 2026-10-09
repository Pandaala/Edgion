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

thread_local! {
    static BOUND_READS: std::cell::RefCell<Option<Vec<&'static str>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn record_bound_read(kind: &'static str) {
    BOUND_READS.with(|reads| {
        if let Some(reads) = reads.borrow_mut().as_mut() {
            reads.push(kind);
        }
    });
}

/// Observe synchronous canonical boundary calls, not internal Grant-store reads.
pub(crate) fn observe_bound_reads<T>(operation: impl FnOnce() -> T) -> (T, Vec<&'static str>) {
    struct Restore(Option<Vec<&'static str>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BOUND_READS.with(|reads| *reads.borrow_mut() = self.0.take());
        }
    }
    let restore = Restore(BOUND_READS.with(|reads| reads.replace(Some(Vec::new()))));
    let result = operation();
    let reads = BOUND_READS.with(|reads| reads.borrow_mut().take().unwrap());
    drop(restore);
    (result, reads)
}

macro_rules! bound_read {
    ($boundary:literal, $operation:expr) => {{
        $crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::tests::record_bound_read($boundary);
        $operation
    }};
}
pub(super) use bound_read;

fn gateway_tls_owner(reference: serde_json::Value, frontend: bool) -> edgion_resources::Gateway {
    let mut value = serde_json::json!({"metadata":{"name":"edge","namespace":"source"},"spec":{"gatewayClassName":"edge",
        "listeners":[{"name":"https","protocol":"HTTPS","port":443,"tls":{"certificateRefs":[]}}]}});
    if frontend {
        value["spec"]["tls"] =
            serde_json::json!({"frontend":{"default":{"validation":{"caCertificateRefs":[reference]}}}});
    } else {
        value["spec"]["listeners"][0]["tls"]["certificateRefs"] = serde_json::json!([reference]);
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn native_gateway_invalid_fields_never_authorize_read_or_register_decoys() {
    use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
    let owner = ResourceRef::new(ResourceKind::Gateway, Some("source".into()), "edge".into());
    for frontend in [false, true] {
        for (key, invalid) in [
            ("group", serde_json::json!("core")),
            ("group", serde_json::json!("other.example")),
            ("kind", serde_json::json!("Service")),
            ("kind", serde_json::json!("")),
            ("name", serde_json::json!("")),
            ("name", serde_json::json!("é".repeat(254))),
            ("namespace", serde_json::json!("")),
            ("namespace", serde_json::json!("Invalid")),
            ("ownerName", serde_json::json!("bad/name")),
            ("ownerNamespace", serde_json::json!("")),
            ("dependency", serde_json::json!("unrelated")),
        ] {
            let (secret_key, secret) = make_secret_with_type("target", "key", None, &[("ca.crt", b"decoy")]);
            let (ctx, runtime) = make_ctx(HashMap::from([(secret_key, secret)]));
            let mut reference = serde_json::json!({"group":"","kind":"Secret","name":"key","namespace":"target"});
            if !frontend && !key.starts_with("owner") && key != "dependency" {
                reference[key] = invalid.clone();
            }
            let mut gateway = gateway_tls_owner(reference, frontend);
            if frontend && !key.starts_with("owner") && key != "dependency" {
                let field = &mut gateway
                    .spec
                    .tls
                    .as_mut()
                    .unwrap()
                    .frontend
                    .as_mut()
                    .unwrap()
                    .default
                    .validation
                    .as_mut()
                    .unwrap()
                    .ca_certificate_refs[0];
                let invalid = invalid.as_str().unwrap().to_owned();
                match key {
                    "group" => field.group = invalid,
                    "kind" => field.kind = invalid,
                    "name" => field.name = invalid,
                    "namespace" => field.namespace = Some(invalid),
                    _ => unreachable!(),
                }
            }

            if key == "ownerName" {
                gateway.metadata.name = Some("bad/name".into());
            }
            if key == "ownerNamespace" {
                gateway.metadata.namespace = Some("".into());
            }
            let dependency = if key == "dependency" {
                ResourceRef::new(ResourceKind::Gateway, Some("source".into()), "unrelated".into())
            } else {
                owner.clone()
            };
            let refs = GatewayTlsRefs::from_gateway(&gateway);
            let field = if frontend {
                refs.frontend_ca(0, 0).unwrap()
            } else {
                refs.serving(0, 0).unwrap()
            };
            let (result, reads) = observe_bound_reads(|| fetch_bound_secret(&ctx, &dependency, field.token()));
            assert!(matches!(result, BoundFetchOutcome::Invalid(_)), "{key}");
            assert!(reads.is_empty(), "{key}: {reads:?}");
            assert!(runtime
                .secret_ref_manager
                .get_dependencies(&dependency.key())
                .is_empty());
            assert!(runtime
                .frontend_ca_ref_manager
                .get_dependencies(&dependency.key())
                .is_empty());
            assert!(runtime.cross_ns_ref_mgr.get_refs("target").is_empty());
        }
    }
}

#[test]
fn native_gateway_ca_kind_routing_pending_edges_and_readonly_fallback_are_exact() {
    use crate::conf_mgr::sync_runtime::resource_processor::frontend_ca_ref::{
        FrontendCaSourceKey, FrontendCaSourceKind,
    };
    use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
    let owner = ResourceRef::new(ResourceKind::Gateway, Some("source".into()), "edge".into());
    for kind in ["Secret", "ConfigMap"] {
        for namespace in ["source", "target"] {
            let (key, secret) = make_secret_with_type("source", "same", None, &[("ca.crt", b"secret-ca")]);
            let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
            runtime.configmap_store.replace_all(HashMap::from([(
                "source/same".into(),
                ConfigMap {
                    metadata: ObjectMeta {
                        name: Some("same".into()),
                        namespace: Some("source".into()),
                        ..Default::default()
                    },
                    data: Some(BTreeMap::from([("ca.crt".into(), "configmap-ca".into())])),
                    ..Default::default()
                },
            )]));
            let gateway = gateway_tls_owner(
                serde_json::json!({"group":"","kind":kind,"name":"same","namespace":namespace}),
                true,
            );
            let refs = GatewayTlsRefs::from_gateway(&gateway);
            let field = refs.frontend_ca(0, 0).unwrap();
            let (fallback, reads) =
                observe_bound_reads(|| authorize_bound_gateway_frontend_ca(&ctx, &owner, field.token()));
            assert_eq!(reads, ["authorization"]);
            assert_eq!(matches!(fallback, BoundFetchOutcome::Ok(())), namespace == "source");
            let typed = if kind == "Secret" {
                FrontendCaSourceKind::Secret
            } else {
                FrontendCaSourceKind::ConfigMap
            };
            assert_eq!(
                runtime.frontend_ca_ref_manager.get_dependencies(&owner.key()),
                [FrontendCaSourceKey::new(typed, namespace, "same").index_key()]
            );
            assert!(runtime.secret_ref_manager.get_dependencies(&owner.key()).is_empty());
            assert_eq!(
                runtime.cross_ns_ref_mgr.get_refs("target").contains(&owner),
                namespace == "target"
            );
            let (material, reads) = observe_bound_reads(|| {
                if kind == "Secret" {
                    match fetch_bound_secret(&ctx, &owner, field.token()) {
                        BoundFetchOutcome::Ok(value) => {
                            Some(String::from_utf8(value.secret.data.unwrap()["ca.crt"].0.clone()).unwrap())
                        }
                        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing { .. }) => None,
                        _ => panic!("unexpected Secret outcome"),
                    }
                } else {
                    match fetch_bound_configmap(&ctx, &owner, field.token()) {
                        BoundFetchOutcome::Ok(value) => Some(value.data.unwrap()["ca.crt"].clone()),
                        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing { .. }) => None,
                        _ => panic!("unexpected ConfigMap outcome"),
                    }
                }
            });
            if namespace == "source" {
                assert_eq!(
                    material.as_deref(),
                    Some(if kind == "Secret" { "secret-ca" } else { "configmap-ca" })
                );
                assert_eq!(
                    reads,
                    ["authorization", if kind == "Secret" { "secret" } else { "configmap" }]
                );
            } else {
                assert!(material.is_none());
                assert_eq!(reads, ["authorization"]);
            }
        }
    }
    let (ctx, runtime) = make_ctx(HashMap::new());
    let gateway = gateway_tls_owner(serde_json::json!({"name":"missing"}), false);
    let refs = GatewayTlsRefs::from_gateway(&gateway);
    let field = refs.serving(0, 0).unwrap();
    assert!(matches!(
        fetch_bound_secret(&ctx, &owner, field.token()),
        BoundFetchOutcome::Pending(PendingReason::SecretNotFound { .. })
    ));
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&owner.key()),
        ["source/missing"]
    );
    assert!(runtime
        .frontend_ca_ref_manager
        .get_dependencies(&owner.key())
        .is_empty());
}

#[test]
fn native_gateway_disabled_grant_policy_is_finite_and_keeps_recovery() {
    const CHILD: &str = "EDGION_R8A_NATIVE_TLS_DISABLED_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "conf_mgr::sync_runtime::resource_processor::credential::pipeline::tests::native_gateway_disabled_grant_policy_is_finite_and_keeps_recovery", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let mut config = crate::config::EdgionControllerConfig::default();
    config.validation.enable_reference_grant_validation = false;
    crate::config::init_controller_config(config);
    use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
    let (key, secret) = make_secret_with_type("target", "key", None, &[("ca.crt", b"material")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let dependency = ResourceRef::new(ResourceKind::Gateway, Some("source".into()), "edge".into());
    let gateway = gateway_tls_owner(serde_json::json!({"name":"key","namespace":"target"}), false);
    let refs = GatewayTlsRefs::from_gateway(&gateway);
    let field = refs.serving(0, 0).unwrap();
    let (result, reads) = observe_bound_reads(|| fetch_bound_secret(&ctx, &dependency, field.token()));
    assert!(matches!(result, BoundFetchOutcome::Ok(_)));
    assert_eq!(reads, ["secret"]);
    assert!(runtime.cross_ns_ref_mgr.get_refs("target").contains(&dependency));
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&dependency.key()),
        ["target/key"]
    );
    native_gateway_invalid_fields_never_authorize_read_or_register_decoys();
    let plugins = bound_plugins(serde_json::json!({"name":"key","namespace":"target"}));
    let prepared = PreparedPluginReferences::from_plugins(&plugins);
    let binding = prepared.bind(&plugins).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let plugin_dependency = ResourceRef::new(ResourceKind::EdgionPlugins, Some("source".into()), "auth".into());
    assert!(matches!(
        fetch_bound_secret(&ctx, &plugin_dependency, references.csrf_secret().unwrap()),
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing { .. })
    ));
}

#[test]
fn native_gateway_material_candidates_preserve_metadata_and_reject_ambiguity() {
    use edgion_resources::resources::refs::gateway_tls_fields::GatewayTlsRefs;
    for kind in ["Secret", "ConfigMap"] {
        for case in [
            "bare",
            "qualified",
            "both",
            "wrong-name",
            "wrong-namespace",
            "empty-namespace",
            "bare-nondefault",
            "qualified-nondefault",
        ] {
            let (ctx, runtime) = make_ctx(HashMap::new());
            let namespace = if case.ends_with("nondefault") {
                "apps"
            } else {
                "default"
            };
            let bare = case == "bare" || case == "both" || case == "bare-nondefault";
            let qualified = case != "bare" && case != "bare-nondefault";
            for is_bare in [true, false] {
                if (is_bare && !bare) || (!is_bare && !qualified) {
                    continue;
                }
                let raw_namespace = if is_bare { None } else { Some(namespace.to_owned()) };
                let metadata = ObjectMeta {
                    name: Some(if case == "wrong-name" { "decoy" } else { "key" }.into()),
                    namespace: if case == "wrong-namespace" {
                        Some("other".into())
                    } else if case == "empty-namespace" {
                        Some("".into())
                    } else {
                        raw_namespace
                    },
                    uid: Some("actual-uid".into()),
                    resource_version: Some("actual-rv".into()),
                    ..Default::default()
                };
                let key = if is_bare {
                    "key".into()
                } else {
                    format!("{namespace}/key")
                };
                if kind == "Secret" {
                    runtime.secret_store.update(
                        HashMap::from([(
                            key,
                            Secret {
                                metadata,
                                ..Default::default()
                            },
                        )]),
                        &Default::default(),
                    );
                } else {
                    runtime.configmap_store.update(
                        HashMap::from([(
                            key,
                            ConfigMap {
                                metadata,
                                ..Default::default()
                            },
                        )]),
                        &Default::default(),
                    );
                }
            }
            let mut gateway = gateway_tls_owner(serde_json::json!({"group":"","kind":kind,"name":"key"}), true);
            gateway.metadata.namespace = Some(namespace.into());
            let dependency = ResourceRef::new(ResourceKind::Gateway, Some(namespace.into()), "edge".into());
            let refs = GatewayTlsRefs::from_gateway(&gateway);
            let field = refs.frontend_ca(0, 0).unwrap();
            let (outcome, reads) = observe_bound_reads(|| {
                if kind == "Secret" {
                    fetch_bound_secret(&ctx, &dependency, field.token())
                        .and_then(|value| BoundFetchOutcome::Ok(value.secret.metadata))
                } else {
                    fetch_bound_configmap(&ctx, &dependency, field.token())
                        .and_then(|value| BoundFetchOutcome::Ok(value.metadata))
                }
            });
            assert_eq!(
                reads,
                if namespace == "default" {
                    vec![
                        "authorization",
                        if kind == "Secret" { "secret" } else { "configmap" },
                        if kind == "Secret" { "secret" } else { "configmap" },
                    ]
                } else {
                    vec!["authorization", if kind == "Secret" { "secret" } else { "configmap" }]
                }
            );
            match case {
                "bare" | "qualified" | "qualified-nondefault" => {
                    let BoundFetchOutcome::Ok(metadata) = outcome else {
                        panic!("{kind} {case}");
                    };
                    assert_eq!(metadata.name.as_deref(), Some("key"));
                    assert_eq!(
                        metadata.namespace.as_deref(),
                        if case == "bare" { None } else { Some(namespace) }
                    );
                    assert_eq!(metadata.uid.as_deref(), Some("actual-uid"));
                    assert_eq!(metadata.resource_version.as_deref(), Some("actual-rv"));
                }
                "bare-nondefault" => assert!(matches!(
                    outcome,
                    BoundFetchOutcome::Pending(PendingReason::SecretNotFound { .. })
                )),
                _ => assert!(matches!(outcome, BoundFetchOutcome::Invalid(_)), "{kind} {case}"),
            }
            assert_eq!(
                runtime
                    .frontend_ca_ref_manager
                    .get_dependencies(&dependency.key())
                    .len(),
                1
            );
            assert!(runtime
                .secret_ref_manager
                .get_dependencies(&dependency.key())
                .is_empty());
        }
    }
}
