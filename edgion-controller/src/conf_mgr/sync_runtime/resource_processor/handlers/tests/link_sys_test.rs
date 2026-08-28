//! Unit tests for `link_sys.rs` (see `super`).
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::condition_reasons;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::credentials::resolvers::{BasicAuthResolver, EsApiKeyResolver};
use edgion_resources::resources::common::{Condition, SecretSlot};
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchAuth;
use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
use edgion_resources::resources::link_sys::LinkSysSpec;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

fn find_cond<'a>(ls: &'a LinkSys, type_: &str) -> Option<&'a Condition> {
    ls.status.as_ref()?.conditions.iter().find(|c| c.type_ == type_)
}

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
    let key = format!("{}/{}", namespace, name);
    (key, secret)
}

fn make_redis_link_sys(ns: &str, name: &str, secret_name: &str, secret_ns: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://localhost:6379".to_string()],
                auth: Some(RedisAuth {
                    slot: edgion_resources::resources::common::SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: secret_name.to_string(),
                            namespace: secret_ns.map(|s| s.to_string()),
                        },
                        secret: None,
                    },
                }),
                db: None,
                timeout: None,
                pool: None,
                retry: None,
                topology: None,
                tls: None,
                observability: None,
            }),
        },
        status: None,
    }
}

fn make_es_link_sys_apikey(ns: &str, name: &str, secret_name: &str) -> LinkSys {
    use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
    LinkSys {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                endpoints: vec!["http://localhost:9200".to_string()],
                auth: Some(ElasticsearchAuth::ApiKey {
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: secret_name.to_string(),
                            namespace: None,
                        },
                        secret: None,
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

fn make_credential_source(ns: &str, name: &str, secret_name: &str) -> LinkSys {
    let spec: LinkSysSpec = serde_yaml::from_str(&format!(
        "type: credentialSource\nconfig:\n  provider:\n    type: oauth2ClientCredentials\n    tokenEndpoint: https://issuer.example.com/token\n    clientAuthentication:\n      activeSecretRef:\n        name: {secret_name}\n"
    ))
    .expect("credentialSource spec");
    LinkSys {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            uid: Some("source-uid".to_string()),
            generation: Some(4),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

#[tokio::test]
async fn credential_source_registers_only_non_secret_desired_state() {
    let (key, mut secret) = make_secret(
        "ops",
        "oauth-bootstrap",
        &[("clientId", b"client"), ("clientSecret", b"secret")],
    );
    secret.metadata.uid = Some("bootstrap-uid".to_string());
    secret.metadata.resource_version = Some("22".to_string());
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));

    let result = LinkSysHandler::new()
        .parse(make_credential_source("ops", "provider", "oauth-bootstrap"), &ctx)
        .await;
    let ProcessResult::Continue(link_sys) = result else {
        panic!("credentialSource must continue");
    };
    let desired = runtime
        .credential_source_registry
        .source("ops/provider")
        .expect("desired source");
    assert_eq!(desired.uid, "source-uid");
    assert_eq!(desired.secret_inputs.len(), 1);
    assert_eq!(desired.secret_inputs[0].resource_version.as_deref(), Some("22"));
    let SystemConfig::CredentialSource(source) = link_sys.spec.config else {
        panic!("credentialSource expected");
    };
    let CredentialSourceProvider::OAuth2ClientCredentials(provider) = source.provider;
    assert!(provider.tls.is_none());
}

#[test]
fn credential_source_reports_unsupported_non_kubernetes_center_reason() {
    let (ctx, runtime) = make_ctx(HashMap::new());
    runtime.set_managed_credentials_supported(false);
    let handler = LinkSysHandler::new();
    let mut link_sys = make_credential_source("ops", "provider", "oauth-bootstrap");
    let errors = handler.preparse(&mut link_sys, &ctx);
    assert!(!errors.is_empty());
    handler.update_status(&mut link_sys, &ctx, &errors);
    let accepted = link_sys
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::ACCEPTED)
        .unwrap();
    assert_eq!(accepted.reason, "UnsupportedConfigurationCenter");
}

#[tokio::test]
async fn persist_off_is_rejected_when_not_single_publisher() {
    let (key, mut secret) = make_secret(
        "ops",
        "bootstrap",
        &[("clientId", b"client"), ("clientSecret", b"secret")],
    );
    secret.metadata.uid = Some("bootstrap-uid".to_string());
    secret.metadata.resource_version = Some("22".to_string());
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    runtime.set_single_publisher(false);
    let handler = LinkSysHandler::new();
    let mut link_sys = make_credential_source("ops", "src", "bootstrap");
    if let SystemConfig::CredentialSource(source) = &mut link_sys.spec.config {
        source.publication.persist = false;
    }
    let errors = handler.preparse(&mut link_sys, &ctx);
    assert!(!errors.is_empty());
    handler.update_status(&mut link_sys, &ctx, &errors);
    let accepted = link_sys
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::ACCEPTED)
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "UnsupportedHaMode");

    let result = handler.parse(link_sys, &ctx).await;
    let ProcessResult::Continue(_) = result else {
        panic!("credentialSource must continue");
    };
    assert!(
        runtime.credential_source_registry.source("ops/src").is_none(),
        "persist=false under all-serve must not register a desired source"
    );
}

#[tokio::test]
async fn invalid_publication_cap_never_registers_a_credential_source() {
    for (persist, memory_max_keys) in [(false, 0), (false, 10_001), (true, 0)] {
        let (key, mut secret) = make_secret(
            "ops",
            "bootstrap",
            &[("clientId", b"client"), ("clientSecret", b"secret")],
        );
        secret.metadata.uid = Some("bootstrap-uid".to_string());
        secret.metadata.resource_version = Some("22".to_string());
        let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = LinkSysHandler::new();
        let mut link_sys = make_credential_source("ops", "src", "bootstrap");
        let SystemConfig::CredentialSource(source) = &mut link_sys.spec.config else {
            panic!("credentialSource expected");
        };
        source.publication.persist = persist;
        source.publication.memory_max_keys = memory_max_keys;

        let errors = handler.preparse(&mut link_sys, &ctx);
        assert!(
            errors.iter().any(|error| error.contains("memoryMaxKeys")),
            "invalid cap must fail admission: {errors:?}"
        );
        let result = handler.parse(link_sys, &ctx).await;
        let ProcessResult::Continue(_) = result else {
            panic!("lenient admission must continue");
        };
        assert!(
            runtime.credential_source_registry.source("ops/src").is_none(),
            "Accepted=False source must not register: persist={persist}, memoryMaxKeys={memory_max_keys}"
        );
    }
}

#[test]
fn persist_off_is_allowed_on_non_kubernetes_single_publisher() {
    let (ctx, runtime) = make_ctx(HashMap::new());
    runtime.set_managed_credentials_supported(false);
    runtime.set_single_publisher(true);
    let handler = LinkSysHandler::new();
    let mut link_sys = make_credential_source("ops", "src", "bootstrap");
    if let SystemConfig::CredentialSource(source) = &mut link_sys.spec.config {
        source.publication.persist = false;
    }
    let errors = handler.preparse(&mut link_sys, &ctx);
    assert!(
        !errors
            .iter()
            .any(|error| error.contains("UnsupportedConfigurationCenter")),
        "persist=false on a non-k8s single publisher must not be mapped as UnsupportedConfigurationCenter: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| {
            error.contains("credentialSource.publication.persist=false is unsupported when ha_mode is all-serve")
        }),
        "persist-off all-serve error must be absent: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| {
            error.contains("credentialSource.publication.persist=true requires the Kubernetes configuration center")
        }),
        "persist-on Kubernetes-only error must be absent: {errors:?}"
    );
    handler.update_status(&mut link_sys, &ctx, &errors);
    let accepted = link_sys
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::ACCEPTED)
        .unwrap();
    assert_ne!(accepted.reason, "UnsupportedConfigurationCenter");
}

fn json_has_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => map.contains_key(key) || map.values().any(|v| json_has_key(v, key)),
        serde_json::Value::Array(items) => items.iter().any(|v| json_has_key(v, key)),
        _ => false,
    }
}

#[tokio::test]
async fn persist_off_link_sys_conf_sync_projection_has_no_generated_map() {
    const TOKEN: &str = "ephemeral-generated-secret-must-not-serialize-xx";
    let (key, mut secret) = make_secret(
        "ops",
        "oauth-bootstrap",
        &[("clientId", b"client"), ("clientSecret", b"secret")],
    );
    secret.metadata.uid = Some("bootstrap-uid".to_string());
    secret.metadata.resource_version = Some("22".to_string());
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));

    runtime
        .credential_memory
        .insert_oauth_publication(
            "ops/provider/source-uid",
            crate::services::credential_manager::memory::GeneratedSecretEntry {
                key: "pub-1".to_string(),
                value: zeroize::Zeroizing::new(TOKEN.as_bytes().to_vec()),
                issued_at: "2026-08-27T00:00:00Z".to_string(),
                valid_until: "2099-08-27T12:00:00Z".to_string(),
                valid_until_mono: std::time::Instant::now() + std::time::Duration::from_secs(3_600),
            },
            10_000,
        )
        .expect("memory insert");

    let handler = LinkSysHandler::new();
    let mut link_sys = make_credential_source("ops", "provider", "oauth-bootstrap");
    if let SystemConfig::CredentialSource(source) = &mut link_sys.spec.config {
        source.publication.persist = false;
    }
    let result = handler.parse(link_sys, &ctx).await;
    let ProcessResult::Continue(mut ls) = result else {
        panic!("credentialSource must continue");
    };
    handler.update_status(&mut ls, &ctx, &[]);

    let value = serde_json::to_value(&ls).expect("LinkSys must serialize");
    let config = value.pointer("/spec/config").expect("spec.config");
    assert!(
        !json_has_key(config, "entries") && !json_has_key(config, "generated") && !json_has_key(config, "value"),
        "spec.config must not carry a generated secret map: {config}"
    );

    let status = value.get("status").unwrap_or(&serde_json::Value::Null);
    assert!(
        !status.to_string().contains(TOKEN),
        "status must not contain the memory token"
    );
}

#[tokio::test]
async fn test_redis_auth_happy_path() {
    let (key, secret) = make_secret(
        "default",
        "redis-creds",
        &[("username", b"admin"), ("password", b"s3cr3t")],
    );
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                _ => panic!("expected Redis"),
            };
            let secret = auth.slot.secret.as_ref().expect("secret should be attached");
            // parse material from embedded secret via resolver
            let data = secret.data.as_ref().expect("secret data");
            let m = BasicAuthResolver.parse(data).expect("parse material");
            assert_eq!(m.username.as_deref(), Some("admin"));
            assert_eq!(m.password, "s3cr3t");
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "True");
            assert_eq!(rr.reason, condition_reasons::RESOLVED_REFS);
        }
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    }
}

#[tokio::test]
async fn test_secret_not_found_clears_injected_secret_and_returns_pending() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let handler = LinkSysHandler::new();
    let mut ls = make_redis_link_sys("default", "my-redis", "missing-secret", None);
    let (_, injected) = make_secret("default", "injected", &[("password", b"operator-value")]);
    let SystemConfig::Redis(redis) = &mut ls.spec.config else {
        panic!("expected Redis");
    };
    redis.auth.as_mut().expect("auth").slot.secret = Some(injected);

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                _ => panic!("expected Redis"),
            };
            assert!(
                auth.slot.secret.is_none(),
                "operator-injected or stale Secret must be cleared while resolution is pending"
            );
            let refs = ctx.secret_ref_manager().get_refs("default/missing-secret");
            let owner = ResourceRef::new(
                ResourceKind::LinkSys,
                Some("default".to_string()),
                "my-redis".to_string(),
            );
            assert!(refs.contains(&owner), "SecretRefManager should contain owner");
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "False");
            assert_eq!(rr.reason, condition_reasons::SECRET_NOT_FOUND);
        }
        ProcessResult::Skip { reason } => panic!("expected Continue (pending), got Skip: {reason}"),
    }
}

#[tokio::test]
async fn test_es_apikey_auth() {
    let (key, secret) = make_secret(
        "default",
        "es-apikey",
        &[("apiKeyId", b"abc123"), ("apiKeySecret", b"secretXYZ")],
    );
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_es_link_sys_apikey("default", "my-es", "es-apikey");

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let auth = match &ls.spec.config {
                SystemConfig::Elasticsearch(es) => es.auth.as_ref().expect("auth"),
                _ => panic!("expected Elasticsearch"),
            };
            let ElasticsearchAuth::ApiKey { slot } = auth else {
                panic!("expected ApiKey variant");
            };
            let data = slot.secret.as_ref().expect("secret").data.as_ref().expect("data");
            let m = EsApiKeyResolver.parse(data).expect("parse");
            assert_eq!(m.api_key_id, "abc123");
            assert_eq!(m.api_key_secret, "secretXYZ");
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "True");
        }
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    }
}

#[tokio::test]
async fn test_on_delete_clears_secret_ref_manager() {
    let (key, secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let result = handler.parse(ls.clone(), &ctx).await;
    assert!(matches!(result, ProcessResult::Continue(_)));

    let refs = ctx.secret_ref_manager().get_refs("default/redis-creds");
    let owner = ResourceRef::new(
        ResourceKind::LinkSys,
        Some("default".to_string()),
        "my-redis".to_string(),
    );
    assert!(refs.contains(&owner), "reference should be registered after parse");

    handler.on_delete(&ls, &ctx).await;

    let refs = ctx.secret_ref_manager().get_refs("default/redis-creds");
    assert!(!refs.contains(&owner), "reference should be cleared after on_delete");
}

#[tokio::test]
async fn test_cross_ns_without_grant_sets_ref_not_permitted() {
    let (key, secret) = make_secret("shared", "redis-creds", &[("password", b"s3cr3t")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    // Owner is in `app`, Secret is in `shared` — no ReferenceGrant configured.
    let ls = make_redis_link_sys("app", "my-redis", "redis-creds", Some("shared"));

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth should be present"),
                _ => panic!("expected Redis"),
            };
            assert!(
                auth.slot.secret.is_none(),
                "secret must not be embedded while cross-ns reference is not permitted"
            );
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "False");
            // Cross-ns denial collapses onto the Gateway-API standard
            // `RefNotPermitted` reason — the pipeline cannot distinguish
            // "no grant yet" from "grant denies", and the upstream spec
            // explicitly uses a single reason for both.
            assert_eq!(rr.reason, condition_reasons::REF_NOT_PERMITTED);
            // Message must still name both endpoints so operators can
            // diagnose which grant to create.
            assert!(
                rr.message.contains("shared/redis-creds"),
                "message must identify target 'shared/redis-creds', got: {}",
                rr.message
            );
            assert!(
                rr.message.contains("ReferenceGrant"),
                "message must name the missing mechanism, got: {}",
                rr.message
            );
            // Even while denied, the owner must be registered so a later
            // ReferenceGrant / Secret event can wake this resource.
            let refs = ctx.secret_ref_manager().get_refs("shared/redis-creds");
            let owner = ResourceRef::new(ResourceKind::LinkSys, Some("app".to_string()), "my-redis".to_string());
            assert!(refs.contains(&owner), "SecretRefManager should register cross-ns owner");
        }
        ProcessResult::Skip { reason } => panic!("expected Continue (pending), got Skip: {reason}"),
    }
}

/// After a successful parse leaves `ResolvedRefs=True`, a subsequent parse
/// against a broken Secret must flip the condition to `False` with the
/// correct reason — not silently retain `True`.
///
/// The real flow re-parses on Secret changes (wakeup via SecretRefManager);
/// here we simulate it by swapping the Secret in the store between two
/// `parse` calls and re-using the status from the prior pass.
#[tokio::test]
async fn test_resolved_refs_transitions_true_to_false_when_secret_broken() {
    let (key, good) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key.clone(), good)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let ls = match handler.parse(ls, &ctx).await {
        ProcessResult::Continue(ls) => ls,
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    };
    let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs v1");
    assert_eq!(rr.status, "True", "initial parse must mark ResolvedRefs=True");

    // Carry the prior status forward onto a fresh spec (mirrors what the
    // informer delivers: spec without `auth.secret`, but status preserved
    // in-cluster from the previous reconcile).
    let mut next = make_redis_link_sys("default", "my-redis", "redis-creds", None);
    next.status = ls.status.clone();

    // Operator drops the required `password` key in the Secret.
    let (_, broken) = make_secret("default", "redis-creds", &[("wrong_key", b"oops")]);
    runtime.secret_store.replace_all(HashMap::from([(key, broken)]));

    let next = match handler.parse(next, &ctx).await {
        ProcessResult::Continue(ls) => ls,
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    };
    let rr = find_cond(&next, condition_types::RESOLVED_REFS).expect("ResolvedRefs v2");
    assert_eq!(rr.status, "False", "broken Secret must flip ResolvedRefs to False");
    assert_eq!(rr.reason, condition_reasons::MISSING_KEYS);
    // `update_condition` merges by type — the prior True condition must be
    // replaced in place, not duplicated.
    let rr_count = next
        .status
        .as_ref()
        .expect("status")
        .conditions
        .iter()
        .filter(|c| c.type_ == condition_types::RESOLVED_REFS)
        .count();
    assert_eq!(rr_count, 1, "conditions must be merged by type, not duplicated");
}

#[tokio::test]
async fn test_invalid_secret_data_sets_resolved_refs_false_and_continues() {
    let (key, secret) = make_secret("default", "redis-creds", &[("wrong_key", b"some_value")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "False");
            assert_eq!(rr.reason, condition_reasons::MISSING_KEYS);
            assert!(rr.message.contains("password"), "message should mention missing key");
            // Invalid data must NOT leave a stale Secret embedded on spec
            // — defence-in-depth so Gateway providers (which already gate
            // on `CredentialState::is_buildable`) never see bad material
            // even if a future change widens dispatch.
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                _ => panic!("expected Redis"),
            };
            assert!(auth.slot.secret.is_none(), "secret must not be attached on Invalid");
        }
        ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
    }
}

/// `DecodeError` variant: the Secret has the required key but the
/// value is not UTF-8, so `BasicAuthResolver::parse` surfaces
/// `ResolveError::DecodeError`, which pipeline maps to
/// `InvalidCredentialError::DecodeError`.
///
/// We assert that:
/// - `parse` returns `Continue` (not `Skip`) so status gets persisted
/// - `ResolvedRefs=False` with reason `DecodeError`
/// - The condition message pinpoints the offending key
/// - No stale Secret is attached to spec
#[tokio::test]
async fn test_decode_error_sets_resolved_refs_false_with_decode_error_reason() {
    // Invalid UTF-8 in `password` — resolver rejects before returning material.
    let (key, secret) = make_secret("default", "redis-creds", &[("password", &[0xff, 0xfe, 0xfd][..])]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "False");
            assert_eq!(rr.reason, condition_reasons::DECODE_ERROR);
            assert!(
                rr.message.contains("password"),
                "DecodeError message must name the offending key 'password', got: {}",
                rr.message
            );
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                _ => panic!("expected Redis"),
            };
            assert!(auth.slot.secret.is_none(), "secret must not be attached on DecodeError");
        }
        ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
    }
}

/// `InvalidSecretType` variant: the Secret data is fine, but its
/// `type` is not in `BasicAuthResolver::allowed_secret_types()`
/// (`kubernetes.io/basic-auth`, `Opaque`, or missing).
///
/// This check runs in `credential/pipeline.rs::check_secret_type` before
/// `resolver.parse`, so we verify that the handler surfaces it with the
/// correct reason and a message that mentions the rejected type.
#[tokio::test]
async fn test_invalid_secret_type_sets_resolved_refs_false_with_invalid_secret_type_reason() {
    let (key, mut secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
    // `kubernetes.io/ssh-auth` is a real Secret type but not accepted here.
    secret.type_ = Some("kubernetes.io/ssh-auth".to_string());
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "False");
            assert_eq!(rr.reason, condition_reasons::INVALID_SECRET_TYPE);
            assert!(
                rr.message.contains("ssh-auth"),
                "message must identify the rejected type, got: {}",
                rr.message
            );
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
                _ => panic!("expected Redis"),
            };
            assert!(
                auth.slot.secret.is_none(),
                "secret must not be attached on InvalidSecretType"
            );
        }
        ProcessResult::Skip { reason } => panic!("expected Continue so status persists, got Skip: {reason}"),
    }
}

#[tokio::test]
async fn test_no_auth_configured() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let handler = LinkSysHandler::new();

    let ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("my-redis".to_string()),
            namespace: Some("default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://localhost:6379".to_string()],
                auth: None,
                db: None,
                timeout: None,
                pool: None,
                retry: None,
                topology: None,
                tls: None,
                observability: None,
            }),
        },
        status: None,
    };

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let auth = match &ls.spec.config {
                SystemConfig::Redis(r) => r.auth.as_ref(),
                _ => panic!("expected Redis"),
            };
            assert!(auth.is_none(), "auth should be None when not configured");
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
            assert_eq!(rr.status, "True");
        }
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    }
}

#[test]
fn test_update_status_sets_accepted_true() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let handler = LinkSysHandler::new();
    let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    handler.update_status(&mut ls, &ctx, &[]);

    let status = ls.status.expect("status should be set");
    let accepted = status
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .expect("Accepted condition should exist");
    assert_eq!(accepted.status, "True");
    assert_eq!(accepted.reason, "Accepted");
    assert!(
        status
            .conditions
            .iter()
            .all(|c| c.type_ != condition_types::RESOLVED_REFS),
        "update_status must not touch ResolvedRefs (set in parse)"
    );
}

#[test]
fn test_update_status_sets_accepted_false_on_validation_errors() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let handler = LinkSysHandler::new();
    let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let errors = vec!["Redis configuration has no endpoints".to_string()];
    handler.update_status(&mut ls, &ctx, &errors);

    let status = ls.status.expect("status should be set");
    let accepted = status
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .expect("Accepted condition should exist");
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
    assert!(accepted.message.contains("no endpoints"));
}

#[tokio::test]
async fn test_redis_tls_secret_ref_resolution() {
    use edgion_resources::credentials::resolvers::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};

    let (key, mut secret) = make_secret(
        "default",
        "redis-tls",
        &[
            (TLS_CA_KEY, b"CA-PEM"),
            (TLS_CERT_KEY, b"CERT-PEM"),
            (TLS_KEY_KEY, b"KEY-PEM"),
        ],
    );
    secret.type_ = Some("kubernetes.io/tls".to_string());

    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();

    let mut ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);
    ls.spec.config = SystemConfig::Redis(RedisClientConfig {
        endpoints: vec!["rediss://localhost:6380".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: None,
        retry: None,
        topology: None,
        tls: Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                validation: Some(edgion_resources::resources::common::OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![
                        edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef {
                            kind: "Secret".to_string(),
                            name: "redis-tls".to_string(),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        observability: None,
    });

    let ls = match handler.parse(ls, &ctx).await {
        ProcessResult::Continue(ls) => ls,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };

    let tls = match &ls.spec.config {
        SystemConfig::Redis(r) => r.tls.as_ref().unwrap(),
        _ => panic!("expected Redis"),
    };
    let secrets = tls
        .resolved_ca_certificates
        .as_ref()
        .expect("resolved_ca_certificates injected");
    let secret = secrets.first().expect("at least one resolved CA secret");
    let data = secret.data.as_ref().expect("data");
    assert_eq!(data.get(TLS_CA_KEY).unwrap().0, b"CA-PEM");
    assert_eq!(data.get(TLS_CERT_KEY).unwrap().0, b"CERT-PEM");
    assert_eq!(data.get(TLS_KEY_KEY).unwrap().0, b"KEY-PEM");
    let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
    assert_eq!(rr.status, "True");
}

/// Multi-error regression: `resolve_for_spec` must NOT short-circuit on
/// the first auth failure — tls must also be resolved so both errors land
/// in the same `ResolvedRefs` condition.
///
/// Setup:
/// - auth.secretRef points to a non-existent Secret → `SecretNotFound`
///   (priority 5 in `pick_resolved_refs_reason`)
/// - tls.outbound.caSecretRef points to a Secret with a rejected `type`
///   (`kubernetes.io/ssh-auth`, which `TlsCertsResolver` does not allow)
///   → `InvalidSecretType` (priority 2 — higher severity)
///
/// Assertions:
/// - `errors.len() == 2` (proves we walked both fields)
/// - `reason == InvalidSecretType` (proves priority table fires)
/// - `message` joins both error texts so operators see both problems
#[tokio::test]
async fn test_auth_and_tls_both_fail_collects_both_errors() {
    use edgion_resources::credentials::resolvers::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};

    // tls Secret exists but with a rejected type — surfaces as
    // `InvalidSecretType` from the pipeline's type-check step.
    let (tls_key, mut tls_secret) = make_secret(
        "default",
        "redis-tls-bad-type",
        &[
            (TLS_CA_KEY, b"CA-PEM"),
            (TLS_CERT_KEY, b"CERT-PEM"),
            (TLS_KEY_KEY, b"KEY-PEM"),
        ],
    );
    tls_secret.type_ = Some("kubernetes.io/ssh-auth".to_string());

    let (ctx, _runtime) = make_ctx(HashMap::from([(tls_key, tls_secret)]));
    let handler = LinkSysHandler::new();

    // auth points to a Secret that doesn't exist in the store.
    let mut ls = make_redis_link_sys("default", "my-redis", "missing-auth", None);
    ls.spec.config = SystemConfig::Redis(RedisClientConfig {
        endpoints: vec!["rediss://localhost:6380".to_string()],
        auth: Some(RedisAuth {
            slot: edgion_resources::resources::common::SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "missing-auth".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        db: None,
        timeout: None,
        pool: None,
        retry: None,
        topology: None,
        tls: Some(TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                validation: Some(edgion_resources::resources::common::OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![
                        edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef {
                            kind: "Secret".to_string(),
                            name: "redis-tls-bad-type".to_string(),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        observability: None,
    });

    let ls = match handler.parse(ls, &ctx).await {
        ProcessResult::Continue(ls) => ls,
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    };

    // Spec must show both sides still at `None`, proving neither side silently succeeded.
    match &ls.spec.config {
        SystemConfig::Redis(r) => {
            assert!(
                r.auth.as_ref().unwrap().slot.secret.is_none(),
                "auth.secret must remain None when auth resolution failed"
            );
            assert!(
                r.tls.as_ref().unwrap().resolved_ca_certificates.is_none(),
                "tls.resolved_ca_certificates must remain None when tls resolution failed"
            );
        }
        _ => panic!("expected Redis"),
    }

    let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
    assert_eq!(rr.status, "False");
    // `InvalidSecretType` (priority 2) wins over `SecretNotFound`
    // (priority 5) per `pick_resolved_refs_reason`. If this flips back
    // to `SecretNotFound`, it means the Redis branch short-circuited on
    // auth again — multi-error aggregation regression.
    assert_eq!(
        rr.reason,
        condition_reasons::INVALID_SECRET_TYPE,
        "picked reason must be the highest-priority error (InvalidSecretType), got: {}",
        rr.reason,
    );
    // Both error messages must appear — `resolved_refs_condition` joins
    // them with "; ". Checking for tokens unique to each error proves
    // the join is actually happening.
    assert!(
        rr.message.contains("missing-auth"),
        "message must mention the missing auth Secret, got: {}",
        rr.message,
    );
    assert!(
        rr.message.contains("ssh-auth"),
        "message must mention the rejected tls Secret type, got: {}",
        rr.message,
    );
}

/// Verify that LinkSys.spec survives a JSON round-trip and preserves the
/// embedded Secret (mimics conf_sync over gRPC).
#[tokio::test]
async fn test_spec_serde_round_trip_preserves_secret() {
    let (key, secret) = make_secret("default", "redis-creds", &[("password", b"s3cr3t")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = LinkSysHandler::new();
    let ls = make_redis_link_sys("default", "my-redis", "redis-creds", None);

    let ls = match handler.parse(ls, &ctx).await {
        ProcessResult::Continue(ls) => ls,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };

    let json = serde_json::to_string(&ls.spec).expect("serialize spec");
    let parsed: LinkSysSpec = serde_json::from_str(&json).expect("deserialize spec");
    let auth = match &parsed.config {
        SystemConfig::Redis(r) => r.auth.as_ref().expect("auth"),
        _ => panic!("expected Redis"),
    };
    let data = auth.slot.secret.as_ref().expect("secret").data.as_ref().expect("data");
    let m = BasicAuthResolver.parse(data).expect("parse material");
    assert_eq!(m.password, "s3cr3t");
    let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs");
    assert_eq!(rr.status, "True");
}

/// After Task 3: webhook Service targets must NOT register into service_ref_mgr.
/// The gateway resolves endpoints itself from the shared store; the controller
/// no longer embeds them or registers wakeup dependencies.
#[tokio::test]
async fn webhook_service_target_registers_no_service_ref() {
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };

    let (ctx, _runtime) = make_ctx(HashMap::new());
    let owner = ResourceRef::new(ResourceKind::LinkSys, Some("ns".to_string()), "websvc".to_string());
    let mut cfg = SystemConfig::Webhook(WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("websvc".into()),
                    namespace: Some("ns".into()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    });
    let _ = resolve_for_spec(&ctx, &owner, &mut cfg).await;
    let refs = ctx.runtime().service_ref_mgr.get_refs("ns/websvc");
    assert!(
        refs.is_empty(),
        "webhook target must not register a service_ref after decouple"
    );
}

// ============================================================================
// Task D1: deprecation warnings for webhook shim fields
//
// The warning emission itself goes through `tracing::warn!`, which the test
// harness does not capture without a subscriber. We therefore assert on the
// pure detector `detect_deprecated_webhook_shim_fields`, which is the single
// source of truth that drives `warn_deprecated_webhook_shim_fields`: if a field
// is detected, the warning fires; if none are detected, none fire.
// ============================================================================

/// Build a minimal valid webhook config with no shim fields set.
fn make_webhook_config() -> edgion_resources::resources::link_sys::webhook::WebhookServiceConfig {
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };
    WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("websvc".into()),
                    namespace: Some("ns".into()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn test_deprecation_warns_on_auth_shim_field() {
    let mut cfg = make_webhook_config();
    cfg.auth = Some(serde_json::json!({ "type": "bearer" }));

    let detected = detect_deprecated_webhook_shim_fields(&cfg);
    assert_eq!(detected.len(), 1, "exactly the 'auth' shim field should warn");
    assert_eq!(detected[0].field, "auth");
    assert!(
        detected[0].migrate_to.contains("secretRef"),
        "auth migration hint should point to the secretRef template, got: {}",
        detected[0].migrate_to
    );
}

#[test]
fn test_deprecation_warns_on_default_headers_shim_field() {
    let mut cfg = make_webhook_config();
    cfg.default_headers = Some(serde_json::json!({ "X-Source": "edgion" }));

    let detected = detect_deprecated_webhook_shim_fields(&cfg);
    assert_eq!(detected.len(), 1, "exactly the 'defaultHeaders' shim field should warn");
    assert_eq!(detected[0].field, "defaultHeaders");
    assert!(
        detected[0].migrate_to.contains("source: value"),
        "defaultHeaders migration hint should point to value-sourced custom header, got: {}",
        detected[0].migrate_to
    );
}

#[test]
fn test_deprecation_warns_on_request_method_shim_field() {
    let mut cfg = make_webhook_config();
    cfg.request_method = Some(serde_json::json!("POST"));

    let detected = detect_deprecated_webhook_shim_fields(&cfg);
    assert_eq!(detected.len(), 1, "exactly the 'requestMethod' shim field should warn");
    assert_eq!(detected[0].field, "requestMethod");
    assert!(
        detected[0].migrate_to.contains("request.method.template"),
        "requestMethod migration hint should point to request.method.template, got: {}",
        detected[0].migrate_to
    );
}

#[test]
fn test_no_deprecation_warning_when_shim_fields_absent() {
    let cfg = make_webhook_config();
    assert!(cfg.auth.is_none());
    assert!(cfg.default_headers.is_none());
    assert!(cfg.request_method.is_none());

    let detected = detect_deprecated_webhook_shim_fields(&cfg);
    assert!(
        detected.is_empty(),
        "no shim fields set must produce no deprecation warnings, got: {detected:?}"
    );
}

/// All three shim fields present at once: every one must warn, in stable order.
#[test]
fn test_deprecation_warns_on_all_shim_fields_together() {
    let mut cfg = make_webhook_config();
    cfg.auth = Some(serde_json::json!({ "type": "bearer" }));
    cfg.default_headers = Some(serde_json::json!({ "X-Source": "edgion" }));
    cfg.request_method = Some(serde_json::json!("POST"));

    let detected = detect_deprecated_webhook_shim_fields(&cfg);
    let fields: Vec<&str> = detected.iter().map(|d| d.field).collect();
    assert_eq!(fields, vec!["auth", "defaultHeaders", "requestMethod"]);
}

// ============================================================================
// Task 8: webhook secretRef resolution — controller resolves ${secretRef:...}
// templates and carries the values inline in resolved_secrets.
// ============================================================================

/// Build a webhook LinkSys whose Authorization header template references a
/// secretRef, then inject the matching Secret and assert that the handler
/// populates `resolved_secrets` with the decoded value.
#[tokio::test]
async fn test_webhook_secret_ref_resolved_into_resolved_secrets() {
    use edgion_resources::resources::link_sys::webhook::{
        CustomField, CustomSource, ExternalHttpConnection, ListFields, WebhookOutboundRequest, WebhookServiceConfig,
        WebhookServiceRef, WebhookTarget,
    };

    // Secret: namespace="edgion-default", name="wh-tok", key="token" -> "my-secret-token"
    let (secret_key, secret) = make_secret("edgion-default", "wh-tok", &[("token", b"my-secret-token")]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(secret_key, secret)]));
    let handler = LinkSysHandler::new();

    let ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("webhook-ls".to_string()),
            namespace: Some("edgion-default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::link_sys::LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            kind: None,
                            name: Some("wh-svc".into()),
                            namespace: Some("edgion-default".into()),
                            port: Some(8080),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                request: WebhookOutboundRequest {
                    headers: ListFields {
                        custom: vec![CustomField {
                            name: "Authorization".to_string(),
                            // Template references the secret; the controller must resolve it.
                            source: CustomSource::Template(
                                "Bearer ${secretRef:edgion-default/wh-tok:token}".to_string(),
                            ),
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        status: None,
    };

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let w = match &ls.spec.config {
                SystemConfig::Webhook(w) => w,
                _ => panic!("expected Webhook"),
            };
            let rs = w.resolved_secrets.as_ref().expect("resolved_secrets must be populated");
            // The map key is the raw var_name: "edgion-default/wh-tok:token"
            let value =
                rs.0.get("edgion-default/wh-tok:token")
                    .expect("resolved_secrets must contain the secretRef key");
            assert_eq!(value, "my-secret-token", "resolved value must match Secret data");

            // ResolvedRefs must be True — all refs resolved.
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs condition");
            assert_eq!(
                rr.status, "True",
                "ResolvedRefs must be True when all secretRefs resolve"
            );
        }
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    }
}

/// When the Secret referenced by a webhook template is absent, the handler
/// must set ResolvedRefs=False with reason SecretNotFound and leave
/// resolved_secrets as None (fail-closed).
#[tokio::test]
async fn test_webhook_secret_ref_missing_secret_sets_resolved_refs_false() {
    use edgion_resources::resources::link_sys::webhook::{
        CustomField, CustomSource, ExternalHttpConnection, ListFields, WebhookOutboundRequest, WebhookServiceConfig,
        WebhookServiceRef, WebhookTarget,
    };

    // No secrets in the store — the referenced secret does not exist.
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let handler = LinkSysHandler::new();

    let ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("webhook-ls".to_string()),
            namespace: Some("edgion-default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::link_sys::LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            kind: None,
                            name: Some("wh-svc".into()),
                            namespace: Some("edgion-default".into()),
                            port: Some(8080),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                request: WebhookOutboundRequest {
                    headers: ListFields {
                        custom: vec![CustomField {
                            name: "Authorization".to_string(),
                            source: CustomSource::Template(
                                "Bearer ${secretRef:edgion-default/wh-tok:token}".to_string(),
                            ),
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        status: None,
    };

    let result = handler.parse(ls, &ctx).await;

    match result {
        ProcessResult::Continue(ls) => {
            let w = match &ls.spec.config {
                SystemConfig::Webhook(w) => w,
                _ => panic!("expected Webhook"),
            };
            // resolved_secrets must remain None while the Secret is absent.
            assert!(
                w.resolved_secrets.is_none(),
                "resolved_secrets must be None while Secret is missing"
            );
            let rr = find_cond(&ls, condition_types::RESOLVED_REFS).expect("ResolvedRefs condition");
            assert_eq!(rr.status, "False", "ResolvedRefs must be False when Secret is missing");
            assert_eq!(
                rr.reason,
                condition_reasons::SECRET_NOT_FOUND,
                "reason must be SecretNotFound, got: {}",
                rr.reason,
            );
        }
        ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
    }
}

// ── Phase 1 SAN guard tests ───────────────────────────────────────────────────

/// Redis LinkSys with `tls.validation.subjectAltNames` must produce an
/// Accepted=False preparse error in Phase 1.
#[tokio::test]
async fn redis_with_san_rejected_phase1() {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, TlsConfig};
    use edgion_resources::resources::link_sys::redis::RedisClientConfig;

    let (ctx, _runtime) = make_ctx(HashMap::new());

    let mut ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("redis-san".to_string()),
            namespace: Some("default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://localhost:6379".to_string()],
                tls: Some(TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            subject_alt_names: Some(vec![SubjectAltName {
                                san_type: SubjectAltNameType::Hostname,
                                hostname: Some("redis.example.com".to_string()),
                                uri: None,
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                auth: None,
                db: None,
                timeout: None,
                pool: None,
                retry: None,
                topology: None,
                observability: None,
            }),
        },
        status: None,
    };

    let handler = LinkSysHandler::new();
    let errors = handler.preparse(&mut ls, &ctx);

    assert!(
        !errors.is_empty(),
        "expected preparse error for Redis subjectAltNames in Phase 1"
    );
    assert!(
        errors.iter().any(|e| e.contains("Redis")),
        "error must identify the provider as Redis; got: {:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("SAN pinning") || e.contains("subjectAltNames")),
        "error must mention SAN pinning; got: {:?}",
        errors
    );
}

/// Etcd LinkSys with `tls.validation.hostname` must produce an
/// Accepted=False preparse error in Phase 1 (spot-check for Etcd path).
#[tokio::test]
async fn etcd_with_hostname_rejected_phase1() {
    use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, TlsConfig};
    use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

    let (ctx, _runtime) = make_ctx(HashMap::new());

    let mut ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("etcd-san".to_string()),
            namespace: Some("default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Etcd(EtcdClientConfig {
                endpoints: vec!["https://etcd:2379".to_string()],
                tls: Some(TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            hostname: Some("etcd.example.com".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                auth: None,
                timeout: None,
                keep_alive: None,
                namespace: None,
                auto_sync_interval: None,
                max_call_send_size: None,
                max_call_recv_size: None,
                user_agent: None,
                reject_old_cluster: None,
                observability: None,
            }),
        },
        status: None,
    };

    let handler = LinkSysHandler::new();
    let errors = handler.preparse(&mut ls, &ctx);

    assert!(
        !errors.is_empty(),
        "expected preparse error for Etcd hostname in Phase 1"
    );
    assert!(
        errors.iter().any(|e| e.contains("Etcd")),
        "error must identify the provider as Etcd; got: {:?}",
        errors
    );
}

/// Redis with only CA refs (no SAN fields) must NOT be rejected by the SAN guard.
#[tokio::test]
async fn redis_ca_only_no_san_accepted() {
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, TlsConfig};
    use edgion_resources::resources::link_sys::redis::RedisClientConfig;

    let (ctx, _runtime) = make_ctx(HashMap::new());

    let mut ls = LinkSys {
        metadata: ObjectMeta {
            name: Some("redis-ca".to_string()),
            namespace: Some("default".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: LinkSysSpec {
            unknown_fields: Vec::new(),
            config: SystemConfig::Redis(RedisClientConfig {
                endpoints: vec!["redis://localhost:6379".to_string()],
                tls: Some(TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                kind: "Secret".to_string(),
                                name: "redis-ca".to_string(),
                                namespace: None,
                                ..Default::default()
                            }]),
                            subject_alt_names: None,
                            hostname: None,
                            well_known_ca_certificates: None,
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                auth: None,
                db: None,
                timeout: None,
                pool: None,
                retry: None,
                topology: None,
                observability: None,
            }),
        },
        status: None,
    };

    let handler = LinkSysHandler::new();
    let errors = handler.preparse(&mut ls, &ctx);

    let san_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.contains("SAN pinning") || e.contains("subjectAltNames"))
        .collect();
    assert!(
        san_errors.is_empty(),
        "CA-only TLS config must not trigger the SAN guard; got: {:?}",
        errors
    );
}
