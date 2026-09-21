//! Controller handler for `EdgionBackend`.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, HandlerContext, ProcessResult,
    ProcessorHandler, ResourceRef,
};
use edgion_resources::credentials::AiApiKeyResolver;
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::resources::common::Condition;
use edgion_resources::resources::edgion_backend::{
    AiBackendReconcileOutcome, AiCredentialResolutionClass, AiCredentialResolutionError, EdgionBackendReconcileOutcome,
    EdgionBackendStatus, AI_CREDENTIAL_ALIAS_MAX_BYTES,
};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::ResourceKind;

const STATUS_MESSAGE_MAX_BYTES: usize = 1024;
const STATUS_TRUNCATION_SUFFIX: &str = "... [truncated]";

pub struct EdgionBackendHandler;

impl EdgionBackendHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionBackendHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn outcome_to_class(
    outcome: ResolveOutcome<k8s_openapi::api::core::v1::Secret>,
) -> Result<k8s_openapi::api::core::v1::Secret, AiCredentialResolutionClass> {
    match outcome {
        ResolveOutcome::Ok(secret) => Ok(secret),
        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
            Err(AiCredentialResolutionClass::SecretNotFound)
        }
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing { .. }) => {
            Err(AiCredentialResolutionClass::RefNotPermitted)
        }
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(_)) => {
            Err(AiCredentialResolutionClass::MissingKeys)
        }
        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { .. }) => {
            Err(AiCredentialResolutionClass::DecodeError)
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidValue { .. }) => {
            Err(AiCredentialResolutionClass::DecodeError)
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { .. }) => {
            Err(AiCredentialResolutionClass::InvalidSecretType)
        }
    }
}

fn requires_secret_edit(class: &AiCredentialResolutionClass) -> bool {
    matches!(
        class,
        AiCredentialResolutionClass::MissingKeys
            | AiCredentialResolutionClass::DecodeError
            | AiCredentialResolutionClass::InvalidSecretType
    )
}

async fn requeue_dependents(backend: &EdgionBackend, ctx: &HandlerContext) {
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: backend.metadata.namespace.as_deref().unwrap_or("default").to_string(),
        name: backend.metadata.name.clone().unwrap_or_default(),
    };
    for dependent in ctx.runtime().backend_ref_mgr.get_refs(&target.to_string()) {
        ctx.requeue(dependent.kind_str(), dependent.resource_key()).await;
    }
}

fn bounded_status_alias(name: &str, index: usize) -> String {
    let valid = !name.is_empty()
        && name.len() <= AI_CREDENTIAL_ALIAS_MAX_BYTES
        && name
            .bytes()
            .enumerate()
            .all(|(offset, byte)| byte.is_ascii_alphanumeric() || (offset > 0 && matches!(byte, b'.' | b'_' | b'-')));
    if valid {
        name.to_string()
    } else {
        format!("credential[{index}]")
    }
}

fn truncate_status_message(mut message: String) -> String {
    if message.len() <= STATUS_MESSAGE_MAX_BYTES {
        return message;
    }
    let keep = STATUS_MESSAGE_MAX_BYTES.saturating_sub(STATUS_TRUNCATION_SUFFIX.len());
    let mut boundary = keep.min(message.len());
    while boundary > 0 && !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message.push_str(STATUS_TRUNCATION_SUFFIX);
    message
}

fn format_partial_credential_errors(errors: &[AiCredentialResolutionError]) -> String {
    let entries = errors
        .iter()
        .map(|error| format!("{}({})", error.alias, error.class.reason()))
        .collect::<Vec<_>>()
        .join(", ");
    truncate_status_message(format!("Omitted credentials ({}): {entries}", errors.len()))
}

fn preserve_transition_time(old: &[Condition], condition: &mut Condition) {
    if let Some(previous) = old.iter().find(|old| {
        old.type_ == condition.type_
            && old.status == condition.status
            && old.reason == condition.reason
            && old.message == condition.message
    }) {
        condition.last_transition_time = previous.last_transition_time.clone();
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionBackend> for EdgionBackendHandler {
    fn preparse(&self, backend: &mut EdgionBackend, _ctx: &HandlerContext) -> Vec<String> {
        backend.spec.validate().err().unwrap_or_default()
    }

    async fn parse(&self, mut backend: EdgionBackend, ctx: &HandlerContext) -> ProcessResult<EdgionBackend> {
        let owner = ResourceRef::new(
            ResourceKind::EdgionBackend,
            backend.metadata.namespace.clone(),
            backend.metadata.name.clone().unwrap_or_default(),
        );

        backend.spec.reconcile_outcome = None;
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&owner);
        let ai = &mut backend.spec.ai;

        for credential in &mut ai.credential_pool.credentials {
            credential.credential.secret = None;
        }

        let total_count = ai.credential_pool.credentials.len();
        let mut resolved_count = 0usize;
        let mut invalid_count = 0usize;
        let mut errors = Vec::new();
        let owner_ns = owner.namespace.as_deref().unwrap_or("default");
        for (index, credential) in ai.credential_pool.credentials.iter_mut().enumerate() {
            let secret_ns = credential.credential.secret_ref.effective_namespace(owner_ns);
            if secret_ns != owner_ns {
                ctx.runtime()
                    .cross_ns_ref_mgr
                    .add_ref(secret_ns.to_string(), owner.clone());
            }
            let outcome = resolve_secret(
                ctx,
                &owner,
                "edgion.io",
                ResourceKind::EdgionBackend.as_str(),
                &credential.credential.secret_ref,
                &AiApiKeyResolver,
            )
            .await;
            match outcome_to_class(outcome) {
                Ok(secret) => {
                    credential.credential.secret = Some(secret);
                    resolved_count += 1;
                }
                Err(class) => {
                    if requires_secret_edit(&class) {
                        invalid_count += 1;
                    }
                    errors.push(AiCredentialResolutionError {
                        alias: bounded_status_alias(&credential.name, index),
                        class,
                    });
                }
            }
        }

        if !errors.is_empty() {
            if invalid_count > 0 {
                tracing::warn!(
                    component = "edgion_backend_handler",
                    resolved_count,
                    failed_count = errors.len(),
                    invalid_count,
                    total_count,
                    "Some AI provider credentials are invalid or unresolved"
                );
            } else {
                tracing::info!(
                    component = "edgion_backend_handler",
                    resolved_count,
                    failed_count = errors.len(),
                    total_count,
                    "Some AI provider credential resolutions are pending"
                );
            }
        }

        backend.spec.reconcile_outcome = Some(EdgionBackendReconcileOutcome::Ai(AiBackendReconcileOutcome {
            resolved_count,
            total_count,
            credential_errors: errors,
        }));

        ProcessResult::Continue(backend)
    }

    async fn on_change(&self, backend: &EdgionBackend, ctx: &HandlerContext) {
        requeue_dependents(backend, ctx).await;
    }

    async fn on_delete(&self, backend: &EdgionBackend, ctx: &HandlerContext) {
        let owner = ResourceRef::new(
            ResourceKind::EdgionBackend,
            backend.metadata.namespace.clone(),
            backend.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&owner);
        requeue_dependents(backend, ctx).await;
        tracing::debug!(
            component = "edgion_backend_handler",
            backend = %owner.key(),
            "Cleared Secret references on EdgionBackend delete"
        );
    }

    fn update_status(&self, backend: &mut EdgionBackend, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = backend.metadata.generation;
        let outcome = backend.spec.reconcile_outcome.as_ref();
        let structural_valid = validation_errors.is_empty();

        let (mut accepted, mut resolved_refs, mut partially_invalid) = if !structural_valid {
            (
                condition_false(
                    condition_types::ACCEPTED,
                    condition_reasons::INVALID,
                    "Backend configuration is invalid",
                    generation,
                ),
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID,
                    "Backend configuration is invalid",
                    generation,
                ),
                condition_false(
                    condition_types::PARTIALLY_INVALID,
                    condition_reasons::ALL_RULES_VALID,
                    "No partially valid credential pool",
                    generation,
                ),
            )
        } else {
            let outcome = match outcome {
                Some(EdgionBackendReconcileOutcome::Ai(outcome)) => outcome.clone(),
                _ => AiBackendReconcileOutcome::default(),
            };
            let first_error = outcome.credential_errors.first();
            let resolved = match first_error {
                None if outcome.resolved_count == outcome.total_count && outcome.total_count > 0 => condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "All credential references resolved",
                    generation,
                ),
                Some(error) => condition_false(
                    condition_types::RESOLVED_REFS,
                    error.class.reason(),
                    truncate_status_message(format!(
                        "Credential '{}' could not be resolved ({})",
                        error.alias,
                        error.class.reason()
                    )),
                    generation,
                ),
                None => condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID,
                    "No usable credentials",
                    generation,
                ),
            };
            let partial = if outcome.resolved_count > 0 && !outcome.credential_errors.is_empty() {
                condition_true(
                    condition_types::PARTIALLY_INVALID,
                    condition_reasons::INVALID_CREDENTIALS,
                    format_partial_credential_errors(&outcome.credential_errors),
                    generation,
                )
            } else {
                condition_false(
                    condition_types::PARTIALLY_INVALID,
                    condition_reasons::ALL_RULES_VALID,
                    "All credential rules are valid",
                    generation,
                )
            };
            (
                condition_true(
                    condition_types::ACCEPTED,
                    condition_reasons::ACCEPTED,
                    "Resource accepted",
                    generation,
                ),
                resolved,
                partial,
            )
        };

        let Some(status) = backend
            .status
            .get_or_insert_with(EdgionBackendStatus::default)
            .as_local_mut()
        else {
            tracing::warn!(
                component = "status",
                kind = "EdgionBackend",
                "Unprojected status cannot be updated"
            );
            return;
        };
        let old = std::mem::take(&mut status.conditions);
        preserve_transition_time(&old, &mut accepted);
        preserve_transition_time(&old, &mut resolved_refs);
        preserve_transition_time(&old, &mut partially_invalid);
        status.conditions = vec![accepted, resolved_refs, partially_invalid];
    }
}

#[cfg(test)]
mod tests {
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
}
