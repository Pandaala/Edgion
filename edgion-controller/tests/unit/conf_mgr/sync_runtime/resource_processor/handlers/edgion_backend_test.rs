use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use edgion_resources::constants::secret_keys::ai::API_KEY;
use edgion_resources::resources::common::{Condition, SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_backend::{
    AiBackendDefaults, AiBackendSpec, AiCredential, AiCredentialPool, AiModel, AiProvider, EdgionBackendSpec,
};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;

fn context(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn secret(namespace: &str, name: &str, type_: Option<&str>, value: Option<&[u8]>) -> (String, Secret) {
    let data = value.map(|value| BTreeMap::from([(API_KEY.to_string(), ByteString(value.to_vec()))]));
    (
        format!("{namespace}/{name}"),
        Secret {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            data,
            type_: type_.map(str::to_string),
            ..Default::default()
        },
    )
}

fn backend(secret_name: &str, secret_namespace: Option<&str>) -> EdgionBackend {
    service_with_credentials(vec![credential("primary", secret_name, secret_namespace)])
}

fn credential(name: &str, secret_name: &str, secret_namespace: Option<&str>) -> AiCredential {
    AiCredential {
        name: name.to_string(),
        weight: 1,
        credential: SecretSlot {
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: secret_name.to_string(),
                namespace: secret_namespace.map(str::to_string),
            },
            secret: None,
        },
        limits: None,
    }
}

fn service_with_credentials(credentials: Vec<AiCredential>) -> EdgionBackend {
    EdgionBackend {
        metadata: ObjectMeta {
            namespace: Some("tenant-a".to_string()),
            name: Some("provider".to_string()),
            generation: Some(3),
            ..Default::default()
        },
        spec: EdgionBackendSpec {
            current_status: None,
            ai: AiBackendSpec {
                provider: AiProvider::OpenAI,
                endpoint: None,
                allow_insecure_http: false,
                credential_pool: AiCredentialPool {
                    redis_ref: None,
                    on_redis_failure: OnRedisFailure::Deny,
                    credentials,
                },
                models: vec![AiModel {
                    name: "model-a".to_string(),
                    aliases: Vec::new(),
                    public: true,
                    pricing: None,
                }],
                defaults: AiBackendDefaults::default(),
            },
            reconcile_outcome: None,
        },
        status: None,
    }
}

fn condition<'a>(backend: &'a EdgionBackend, type_: &str) -> &'a Condition {
    backend
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|condition| condition.type_ == type_)
        .unwrap()
}

async fn parse(handler: &EdgionBackendHandler, backend: EdgionBackend, ctx: &HandlerContext) -> EdgionBackend {
    match handler.parse(backend, ctx).await {
        ProcessResult::Continue(mut backend) => {
            handler.update_status(&mut backend, ctx, &[]);
            backend
        }
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    }
}

#[tokio::test]
async fn resolves_valid_secret_and_registers_reference() {
    let (key, credential) = secret("tenant-a", "provider-key", Some("Opaque"), Some(b"private-value"));
    let (ctx, _) = context(HashMap::from([(key, credential)]));
    let handler = EdgionBackendHandler::new();
    let parsed = parse(&handler, backend("provider-key", None), &ctx).await;

    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());
    assert_eq!(condition(&parsed, condition_types::RESOLVED_REFS).status, "True");
    let partially_invalid = condition(&parsed, condition_types::PARTIALLY_INVALID);
    assert_eq!(partially_invalid.status, "False");
    assert_eq!(partially_invalid.reason, condition_reasons::ALL_RULES_VALID);
    let owner = ResourceRef::new(
        ResourceKind::EdgionBackend,
        Some("tenant-a".to_string()),
        "provider".to_string(),
    );
    assert!(ctx
        .secret_ref_manager()
        .get_refs("tenant-a/provider-key")
        .contains(&owner));
}

#[tokio::test]
async fn partially_resolved_pool_embeds_good_credentials_and_reports_partial_invalid() {
    let (key, secret) = secret("tenant-a", "primary-key", Some("Opaque"), Some(b"private-value"));
    let (ctx, _) = context(HashMap::from([(key, secret)]));
    let parsed = parse(
        &EdgionBackendHandler::new(),
        service_with_credentials(vec![
            credential("primary", "primary-key", None),
            credential("standby", "missing-key", None),
        ]),
        &ctx,
    )
    .await;

    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());
    assert!(parsed.spec.ai.credential_pool.credentials[1]
        .credential
        .secret
        .is_none());
    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::SECRET_NOT_FOUND);
    let partially_invalid = condition(&parsed, condition_types::PARTIALLY_INVALID);
    assert_eq!(partially_invalid.status, "True");
    assert_eq!(partially_invalid.reason, condition_reasons::INVALID_CREDENTIALS);
    assert_eq!(
        partially_invalid.message,
        "Omitted credentials (1): standby(SecretNotFound)"
    );
    assert!(!partially_invalid.message.contains("tenant-a/missing-key"));
    assert!(!partially_invalid.message.contains("private-value"));
    assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/primary-key").len(), 1);
    assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/missing-key").len(), 1);
}

#[tokio::test]
async fn partially_resolved_pool_reports_cross_namespace_denial() {
    let (local_key, local_secret) = secret("tenant-a", "primary-key", Some("Opaque"), Some(b"private-value"));
    let (shared_key, shared_secret) = secret("shared", "standby-key", Some("Opaque"), Some(b"standby-value"));
    let (ctx, runtime) = context(HashMap::from([(local_key, local_secret), (shared_key, shared_secret)]));
    let parsed = parse(
        &EdgionBackendHandler::new(),
        service_with_credentials(vec![
            credential("primary", "primary-key", None),
            credential("standby", "standby-key", Some("shared")),
        ]),
        &ctx,
    )
    .await;

    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());
    assert!(parsed.spec.ai.credential_pool.credentials[1]
        .credential
        .secret
        .is_none());
    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::REF_NOT_PERMITTED);
    let partially_invalid = condition(&parsed, condition_types::PARTIALLY_INVALID);
    assert_eq!(partially_invalid.status, "True");
    assert_eq!(partially_invalid.reason, condition_reasons::INVALID_CREDENTIALS);
    assert_eq!(
        partially_invalid.message,
        "Omitted credentials (1): standby(RefNotPermitted)"
    );
    assert_eq!(ctx.secret_ref_manager().get_refs("shared/standby-key").len(), 1);
    assert!(runtime.cross_ns_ref_mgr.get_refs("shared").contains(&ResourceRef::new(
        ResourceKind::EdgionBackend,
        Some("tenant-a".to_string()),
        "provider".to_string(),
    )));
    EdgionBackendHandler::new().on_delete(&parsed, &ctx).await;
    assert!(runtime.cross_ns_ref_mgr.get_refs("shared").is_empty());
}

#[tokio::test]
async fn completely_unresolved_pool_fails_closed_with_resolved_refs_false() {
    let (key, invalid_secret) = secret("tenant-a", "invalid-key", Some("Opaque"), None);
    let (ctx, _) = context(HashMap::from([(key, invalid_secret)]));
    let parsed = parse(
        &EdgionBackendHandler::new(),
        service_with_credentials(vec![
            credential("primary", "missing-key", None),
            credential("standby", "invalid-key", None),
        ]),
        &ctx,
    )
    .await;

    assert!(parsed
        .spec
        .ai
        .credential_pool
        .credentials
        .iter()
        .all(|credential| credential.credential.secret.is_none()));
    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::SECRET_NOT_FOUND);
    let partially_invalid = condition(&parsed, condition_types::PARTIALLY_INVALID);
    assert_eq!(partially_invalid.status, "False");
    assert_eq!(partially_invalid.reason, condition_reasons::ALL_RULES_VALID);
}

#[tokio::test]
async fn structurally_empty_pool_never_reports_resolved_refs_true() {
    let (ctx, _) = context(HashMap::new());
    let parsed = parse(&EdgionBackendHandler::new(), service_with_credentials(Vec::new()), &ctx).await;

    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::INVALID);
    assert_eq!(resolved.message, "No usable credentials");
}

#[tokio::test]
async fn missing_secret_stays_pending_and_registered() {
    let (ctx, _) = context(HashMap::new());
    let parsed = parse(&EdgionBackendHandler::new(), backend("missing", None), &ctx).await;
    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::SECRET_NOT_FOUND);
    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/missing").len(), 1);
}

#[tokio::test]
async fn invalid_secret_shapes_are_reported_without_values() {
    for (credential, expected_reason) in [
        (
            secret("tenant-a", "missing-key", Some("Opaque"), None).1,
            condition_reasons::MISSING_KEYS,
        ),
        (
            secret("tenant-a", "bad-utf8", Some("Opaque"), Some(&[0xff])).1,
            condition_reasons::DECODE_ERROR,
        ),
        (
            secret(
                "tenant-a",
                "wrong-type",
                Some("kubernetes.io/tls"),
                Some(b"private-value"),
            )
            .1,
            condition_reasons::INVALID_SECRET_TYPE,
        ),
    ] {
        let name = credential.metadata.name.clone().unwrap();
        let (ctx, _) = context(HashMap::from([(format!("tenant-a/{name}"), credential)]));
        let parsed = parse(&EdgionBackendHandler::new(), backend(&name, None), &ctx).await;
        let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.reason, expected_reason);
        assert!(!resolved.message.contains("private-value"));
        assert!(parsed.spec.ai.credential_pool.credentials[0]
            .credential
            .secret
            .is_none());
    }
}

#[tokio::test]
async fn cross_namespace_denial_fails_closed_and_keeps_dependency() {
    let (key, credential) = secret("shared", "provider-key", None, Some(b"private-value"));
    let (ctx, _) = context(HashMap::from([(key, credential)]));
    let parsed = parse(
        &EdgionBackendHandler::new(),
        backend("provider-key", Some("shared")),
        &ctx,
    )
    .await;
    let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
    assert_eq!(resolved.reason, condition_reasons::REF_NOT_PERMITTED);
    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    assert_eq!(ctx.secret_ref_manager().get_refs("shared/provider-key").len(), 1);
}

#[tokio::test]
async fn reconcile_clears_stale_material_and_dropped_reference() {
    let (old_key, old_secret) = secret("tenant-a", "old-key", None, Some(b"old-private"));
    let (ctx, _) = context(HashMap::from([(old_key, old_secret)]));
    let handler = EdgionBackendHandler::new();
    let mut parsed = parse(&handler, backend("old-key", None), &ctx).await;
    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());

    parsed.spec.ai.credential_pool.credentials[0].credential.secret_ref.name = "new-missing".to_string();
    let reparsed = parse(&handler, parsed, &ctx).await;
    assert!(reparsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    assert!(ctx.secret_ref_manager().get_refs("tenant-a/old-key").is_empty());
    assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/new-missing").len(), 1);
}

#[tokio::test]
async fn secret_repair_transitions_resolved_refs_false_to_true() {
    let (ctx, runtime) = context(HashMap::new());
    let handler = EdgionBackendHandler::new();
    let pending = parse(&handler, backend("provider-key", None), &ctx).await;
    assert_eq!(condition(&pending, condition_types::RESOLVED_REFS).status, "False");

    let (key, credential) = secret("tenant-a", "provider-key", None, Some(b"repaired-private"));
    runtime.secret_store.replace_all(HashMap::from([(key, credential)]));
    let repaired = parse(&handler, pending, &ctx).await;
    assert_eq!(condition(&repaired, condition_types::RESOLVED_REFS).status, "True");
    assert!(repaired.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());
}

#[tokio::test]
async fn delete_clears_reference() {
    let (ctx, _) = context(HashMap::new());
    let handler = EdgionBackendHandler::new();
    let backend = parse(&handler, backend("provider-key", None), &ctx).await;
    assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/provider-key").len(), 1);
    handler.on_delete(&backend, &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("tenant-a/provider-key").is_empty());
}

#[test]
fn preparse_and_status_own_accepted_condition() {
    let (ctx, _) = context(HashMap::new());
    let handler = EdgionBackendHandler::new();
    let mut backend = backend("provider-key", None);
    assert!(handler.preparse(&mut backend, &ctx).is_empty());
    handler.update_status(&mut backend, &ctx, &[]);
    assert_eq!(condition(&backend, condition_types::ACCEPTED).status, "True");

    backend.spec.ai.models.clear();
    let errors = handler.preparse(&mut backend, &ctx);
    handler.update_status(&mut backend, &ctx, &errors);
    let accepted = condition(&backend, condition_types::ACCEPTED);
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
}

#[test]
fn status_rebuild_is_ordered_destructive_and_tuple_stable() {
    const SENTINEL: &str = "1970-01-01T00:00:00Z";

    let (ctx, _) = context(HashMap::new());
    let handler = EdgionBackendHandler::new();
    let mut backend = backend("provider-key", None);
    backend.spec.reconcile_outcome = Some(EdgionBackendReconcileOutcome::Ai(AiBackendReconcileOutcome {
        resolved_count: 1,
        total_count: 1,
        credential_errors: Vec::new(),
    }));
    handler.update_status(&mut backend, &ctx, &[]);
    backend
        .status
        .as_mut()
        .unwrap()
        .as_local_mut()
        .expect("native test status")
        .conditions
        .push(condition_true("StaleCondition", "Stale", "stale", Some(1)));
    for condition in &mut backend
        .status
        .as_mut()
        .unwrap()
        .as_local_mut()
        .expect("native test status")
        .conditions
    {
        condition.last_transition_time = SENTINEL.to_string();
    }

    handler.update_status(&mut backend, &ctx, &[]);
    let conditions = &backend
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .expect("native test status")
        .conditions;
    assert_eq!(conditions.len(), 3);
    assert_eq!(conditions[0].type_, condition_types::ACCEPTED);
    assert_eq!(conditions[1].type_, condition_types::RESOLVED_REFS);
    assert_eq!(conditions[2].type_, condition_types::PARTIALLY_INVALID);
    assert!(conditions
        .iter()
        .all(|condition| condition.last_transition_time == SENTINEL));

    backend.spec.reconcile_outcome = Some(EdgionBackendReconcileOutcome::Ai(AiBackendReconcileOutcome {
        resolved_count: 0,
        total_count: 1,
        credential_errors: vec![AiCredentialResolutionError {
            alias: "primary".to_string(),
            class: AiCredentialResolutionClass::SecretNotFound,
        }],
    }));
    handler.update_status(&mut backend, &ctx, &[]);
    let conditions = &backend
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .expect("native test status")
        .conditions;
    assert_eq!(conditions[0].last_transition_time, SENTINEL);
    assert_ne!(conditions[1].last_transition_time, SENTINEL);
    assert_eq!(conditions[2].last_transition_time, SENTINEL);
}

#[test]
fn bounded_status_messages_are_utf8_safe() {
    let message = truncate_status_message("é".repeat(800));
    assert!(message.len() <= STATUS_MESSAGE_MAX_BYTES);
    assert!(message.is_char_boundary(message.len()));
    assert!(message.ends_with(STATUS_TRUNCATION_SUFFIX));
}

fn observed_parse(backend: EdgionBackend, ctx: &HandlerContext) -> (EdgionBackend, Vec<&'static str>) {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::observe_bound_reads;
    use futures::FutureExt;
    observe_bound_reads(|| {
        parse(&EdgionBackendHandler::new(), backend, ctx)
            .now_or_never()
            .expect("Backend Secret resolution must remain synchronous")
    })
}

#[test]
fn actual_backend_secret_identity_precedes_reads_and_dependency_registration() {
    for (namespace, name) in [
        (Some(""), Some("provider")),
        (Some("bad/ns"), Some("provider")),
        (Some("tenant-a"), None),
        (Some("tenant-a"), Some("")),
        (Some("tenant-a"), Some("bad/name")),
    ] {
        let mut input = backend("provider-key", None);
        input.metadata.namespace = namespace.map(str::to_owned);
        input.metadata.name = name.map(str::to_owned);
        input.metadata.uid = Some("valid-uid".into());
        let (key, material) = secret(
            namespace.unwrap_or("default"),
            "provider-key",
            Some("Opaque"),
            Some(b"private-value"),
        );
        input.spec.ai.credential_pool.credentials[0].credential.secret = Some(material.clone());
        let (ctx, runtime) = context(HashMap::from([(key.clone(), material)]));
        let (parsed, reads) = observed_parse(input, &ctx);
        assert!(reads.is_empty());
        assert!(ctx.secret_ref_manager().get_refs(&key).is_empty());
        assert!(runtime
            .cross_ns_ref_mgr
            .get_refs(namespace.unwrap_or("default"))
            .is_empty());
        assert!(parsed.spec.ai.credential_pool.credentials[0]
            .credential
            .secret
            .is_none());
        assert_eq!(
            condition(&parsed, condition_types::RESOLVED_REFS).reason,
            "InvalidReference"
        );
        assert!(!condition(&parsed, condition_types::RESOLVED_REFS)
            .message
            .contains("private-value"));
    }
    for (group, kind, namespace, name) in [
        (Some("other.io"), None, None, "provider-key"),
        (None, Some("ConfigMap"), None, "provider-key"),
        (None, None, Some(""), "provider-key"),
        (None, None, None, ""),
        (None, None, None, "bad/name"),
    ] {
        let mut input = backend(name, namespace);
        let wire = &mut input.spec.ai.credential_pool.credentials[0].credential.secret_ref;
        wire.group = group.map(str::to_owned);
        wire.kind = kind.map(str::to_owned);
        let (key, material) = secret(
            namespace.unwrap_or("tenant-a"),
            name,
            Some("Opaque"),
            Some(b"private-value"),
        );
        let (ctx, runtime) = context(HashMap::from([(key.clone(), material)]));
        let (parsed, reads) = observed_parse(input, &ctx);
        assert!(reads.is_empty());
        assert!(ctx.secret_ref_manager().get_refs(&key).is_empty());
        assert!(runtime
            .cross_ns_ref_mgr
            .get_refs(namespace.unwrap_or("tenant-a"))
            .is_empty());
        assert!(parsed.spec.ai.credential_pool.credentials[0]
            .credential
            .secret
            .is_none());
        assert_eq!(
            condition(&parsed, condition_types::RESOLVED_REFS).reason,
            "InvalidReference"
        );
    }
    for namespace in [None, Some("tenant-a")] {
        let mut input = backend("provider-key", None);
        input.metadata.namespace = namespace.map(str::to_owned);
        input.spec.ai.credential_pool.credentials[0].credential.secret_ref.group = Some("core".into());
        let (key, material) = secret(
            namespace.unwrap_or("default"),
            "provider-key",
            Some("Opaque"),
            Some(b"private-value"),
        );
        let (ctx, _) = context(HashMap::from([(key.clone(), material)]));
        let (parsed, reads) = observed_parse(input, &ctx);
        assert_eq!(reads, ["authorization", "secret"]);
        assert_eq!(
            parsed.spec.ai.credential_pool.credentials[0]
                .credential
                .secret
                .as_ref()
                .unwrap()
                .data
                .as_ref()
                .unwrap()[API_KEY]
                .0,
            b"private-value"
        );
        assert_eq!(
            ctx.secret_ref_manager().get_refs(&key),
            vec![ResourceRef::new(
                ResourceKind::EdgionBackend,
                namespace.map(str::to_owned),
                "provider".into()
            )]
        );
    }
    assert!(!requires_secret_edit(&AiCredentialResolutionClass::InvalidReference));
}

#[test]
fn actual_backend_secret_update_clears_old_material_and_edges_and_recovers() {
    use edgion_resources::ReferenceGrant;
    let (key, material) = secret("shared", "provider-key", Some("Opaque"), Some(b"private-value"));
    let (ctx, runtime) = context(HashMap::from([(key.clone(), material.clone())]));
    let grant: Arc<ReferenceGrant> = Arc::new(
        serde_json::from_value(serde_json::json!({
            "metadata":{"namespace":"shared","name":"allow"},
            "spec":{"from":[{"group":"edgion.io","kind":"EdgionBackend","namespace":"tenant-a"}],
                    "to":[{"group":"","kind":"Secret","name":"provider-key"}]}
        }))
        .unwrap(),
    );
    let input = backend("provider-key", Some("shared"));
    let (denied, reads) = observed_parse(input, &ctx);
    assert_eq!(reads, ["authorization"]);
    assert_eq!(
        condition(&denied, condition_types::RESOLVED_REFS).reason,
        "RefNotPermitted"
    );
    assert_eq!(ctx.secret_ref_manager().get_refs(&key).len(), 1);
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("shared").len(), 1);
    runtime.ref_grant_store.upsert("shared/allow".into(), grant.clone());
    let (allowed, reads) = observed_parse(denied, &ctx);
    assert_eq!(reads, ["authorization", "secret"]);
    assert!(allowed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_some());
    let mut invalid = allowed.clone();
    invalid.spec.ai.credential_pool.credentials[0]
        .credential
        .secret_ref
        .kind = Some("ConfigMap".into());
    let (invalid, reads) = observed_parse(invalid, &ctx);
    assert_eq!(invalid.metadata, allowed.metadata, "same raw owner identity is updated");
    assert!(reads.is_empty());
    assert!(ctx.secret_ref_manager().get_refs(&key).is_empty());
    assert!(runtime.cross_ns_ref_mgr.get_refs("shared").is_empty());
    assert!(invalid.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    let mut recovered = invalid;
    recovered.spec.ai.credential_pool.credentials[0]
        .credential
        .secret_ref
        .kind = None;
    let (recovered, reads) = observed_parse(recovered, &ctx);
    assert_eq!(reads, ["authorization", "secret"]);
    runtime.ref_grant_store.remove("shared/allow");
    let (denied, reads) = observed_parse(recovered, &ctx);
    assert_eq!(reads, ["authorization"]);
    assert!(denied.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    runtime.ref_grant_store.upsert("shared/allow".into(), grant);
    runtime.secret_store.replace_all(HashMap::new());
    let (missing, reads) = observed_parse(denied, &ctx);
    assert_eq!(reads, ["authorization", "secret"]);
    assert_eq!(
        condition(&missing, condition_types::RESOLVED_REFS).reason,
        "SecretNotFound"
    );
    assert_eq!(ctx.secret_ref_manager().get_refs(&key).len(), 1);
    assert_eq!(runtime.cross_ns_ref_mgr.get_refs("shared").len(), 1);
    runtime.secret_store.replace_all(HashMap::from([(key, material)]));
    let (repaired, reads) = observed_parse(missing, &ctx);
    assert_eq!(reads, ["authorization", "secret"]);
    assert_eq!(condition(&repaired, condition_types::RESOLVED_REFS).status, "True");
}

#[test]
fn actual_backend_secret_invalid_sibling_retains_partial_good_and_structural_status() {
    let (key, material) = secret("tenant-a", "provider-key", Some("Opaque"), Some(b"private-value"));
    let (ctx, _) = context(HashMap::from([(key, material)]));
    let mut input = service_with_credentials(vec![
        credential("invalid", "provider-key", None),
        credential("valid", "provider-key", None),
    ]);
    input.spec.ai.credential_pool.credentials[0].credential.secret_ref.kind = Some("ConfigMap".into());
    let errors = EdgionBackendHandler::new().preparse(&mut input, &ctx);
    assert!(!errors.is_empty());
    let (mut parsed, reads) = observed_parse(input, &ctx);
    assert_eq!(reads, ["authorization", "secret"]);
    assert!(parsed.spec.ai.credential_pool.credentials[0]
        .credential
        .secret
        .is_none());
    assert!(parsed.spec.ai.credential_pool.credentials[1]
        .credential
        .secret
        .is_some());
    assert_eq!(
        condition(&parsed, condition_types::RESOLVED_REFS).reason,
        "InvalidReference"
    );
    assert_eq!(condition(&parsed, condition_types::PARTIALLY_INVALID).status, "True");
    EdgionBackendHandler::new().update_status(&mut parsed, &ctx, &errors);
    assert_eq!(condition(&parsed, condition_types::ACCEPTED).status, "False");
    assert_eq!(condition(&parsed, condition_types::RESOLVED_REFS).reason, "Invalid");
}
