//! Controller handler for `EdgionAIService`.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::partially_invalid_condition;
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_reasons, condition_true, condition_types, resolved_refs_condition,
    update_condition, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use edgion_resources::credentials::AiApiKeyResolver;
use edgion_resources::prelude_resources::EdgionAIService;
use edgion_resources::resources::edgion_ai_service::{EdgionAIServiceStatus, AI_CREDENTIAL_ALIAS_MAX_BYTES};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::ResourceKind;

pub struct EdgionAIServiceHandler;

impl EdgionAIServiceHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionAIServiceHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn outcome_to_error(
    outcome: ResolveOutcome<k8s_openapi::api::core::v1::Secret>,
) -> Result<k8s_openapi::api::core::v1::Secret, ResolvedRefsError> {
    match outcome {
        ResolveOutcome::Ok(secret) => Ok(secret),
        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            Err(ResolvedRefsError::SecretNotFound { namespace, name })
        }
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: ResourceKind::EdgionAIService.as_str(),
            target_namespace: to_ns,
            target_name: secret_name,
        }),
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
            Err(ResolvedRefsError::MissingKeys { keys })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
            Err(ResolvedRefsError::DecodeError { key, detail })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
            Err(ResolvedRefsError::InvalidSecretType { got, allowed })
        }
    }
}

fn requires_secret_edit(error: &ResolvedRefsError) -> bool {
    matches!(
        error,
        ResolvedRefsError::MissingKeys { .. }
            | ResolvedRefsError::DecodeError { .. }
            | ResolvedRefsError::InvalidSecretType { .. }
    )
}

async fn requeue_dependents(service: &EdgionAIService, ctx: &HandlerContext) {
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionAIService,
        namespace: service.metadata.namespace.as_deref().unwrap_or("default").to_string(),
        name: service.metadata.name.clone().unwrap_or_default(),
    };
    for dependent in ctx.runtime().ai_service_ref_mgr.get_refs(&target.to_string()) {
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

fn format_partial_credential_errors(errors: &[(String, ResolvedRefsError)]) -> String {
    let entries = errors
        .iter()
        .map(|(name, error)| format!("{name}({})", error.reason()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Omitted credentials ({}): {entries}", errors.len())
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAIService> for EdgionAIServiceHandler {
    fn preparse(&self, service: &mut EdgionAIService, _ctx: &HandlerContext) -> Vec<String> {
        service.spec.validate().err().unwrap_or_default()
    }

    async fn parse(&self, mut service: EdgionAIService, ctx: &HandlerContext) -> ProcessResult<EdgionAIService> {
        let owner = ResourceRef::new(
            ResourceKind::EdgionAIService,
            service.metadata.namespace.clone(),
            service.metadata.name.clone().unwrap_or_default(),
        );

        // A reconcile is a full replacement: remove every previous dependency
        // edge and embedded credential before rebuilding the pool. This is
        // required for delete-create coalescing in the per-kind workqueue.
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        for credential in &mut service.spec.credential_pool.credentials {
            credential.credential.secret = None;
        }

        let total_count = service.spec.credential_pool.credentials.len();
        let mut resolved_count = 0usize;
        let mut invalid_count = 0usize;
        let mut errors = Vec::new();
        for (index, credential) in service.spec.credential_pool.credentials.iter_mut().enumerate() {
            let outcome = resolve_secret(
                ctx,
                &owner,
                "edgion.io",
                ResourceKind::EdgionAIService.as_str(),
                &credential.credential.secret_ref,
                &AiApiKeyResolver,
            )
            .await;
            match outcome_to_error(outcome) {
                Ok(secret) => {
                    credential.credential.secret = Some(secret);
                    resolved_count += 1;
                }
                Err(error) => {
                    if requires_secret_edit(&error) {
                        invalid_count += 1;
                    }
                    errors.push((bounded_status_alias(&credential.name, index), error));
                }
            }
        }

        if !errors.is_empty() {
            if invalid_count > 0 {
                tracing::warn!(
                    component = "edgion_ai_service_handler",
                    resolved_count,
                    failed_count = errors.len(),
                    invalid_count,
                    total_count,
                    "Some AI provider credentials are invalid or unresolved"
                );
            } else {
                tracing::info!(
                    component = "edgion_ai_service_handler",
                    resolved_count,
                    failed_count = errors.len(),
                    total_count,
                    "Some AI provider credential resolutions are pending"
                );
            }
        }

        let generation = service.metadata.generation;
        let status = service.status.get_or_insert_with(EdgionAIServiceStatus::default);
        let resolved_errors = errors.iter().map(|(_, error)| error.clone()).collect::<Vec<_>>();
        let resolved_refs = if resolved_errors.is_empty() && resolved_count == 0 {
            condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::INVALID,
                "No usable credentials",
                generation,
            )
        } else {
            resolved_refs_condition(&resolved_errors, generation)
        };
        update_condition(&mut status.conditions, resolved_refs);
        let partially_invalid = if resolved_count > 0 && !errors.is_empty() {
            condition_true(
                condition_types::PARTIALLY_INVALID,
                condition_reasons::INVALID_CREDENTIALS,
                format_partial_credential_errors(&errors),
                generation,
            )
        } else {
            partially_invalid_condition(None, generation)
        };
        update_condition(&mut status.conditions, partially_invalid);

        // Invalid and pending resources remain in the cache/conf_sync stream so
        // their status is visible and Secret/ReferenceGrant changes can wake them.
        ProcessResult::Continue(service)
    }

    async fn on_change(&self, service: &EdgionAIService, ctx: &HandlerContext) {
        requeue_dependents(service, ctx).await;
    }

    async fn on_delete(&self, service: &EdgionAIService, ctx: &HandlerContext) {
        let owner = ResourceRef::new(
            ResourceKind::EdgionAIService,
            service.metadata.namespace.clone(),
            service.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        requeue_dependents(service, ctx).await;
        tracing::debug!(
            component = "edgion_ai_service_handler",
            edgion_ai_service = %owner.key(),
            "Cleared Secret references on EdgionAIService delete"
        );
    }

    fn update_status(&self, service: &mut EdgionAIService, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = service.metadata.generation;
        let status = service.status.get_or_insert_with(EdgionAIServiceStatus::default);
        let accepted = if validation_errors.is_empty() {
            accepted_condition(generation)
        } else {
            condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                validation_errors.join("; "),
                generation,
            )
        };
        update_condition(&mut status.conditions, accepted);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use edgion_resources::constants::secret_keys::ai::API_KEY;
    use edgion_resources::resources::common::{Condition, SecretObjectReference, SecretSlot};
    use edgion_resources::resources::edgion_ai_service::{
        AiCredential, AiCredentialPool, AiModel, AiProvider, AiServiceDefaults, EdgionAIServiceSpec,
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

    fn service(secret_name: &str, secret_namespace: Option<&str>) -> EdgionAIService {
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

    fn service_with_credentials(credentials: Vec<AiCredential>) -> EdgionAIService {
        EdgionAIService {
            metadata: ObjectMeta {
                namespace: Some("tenant-a".to_string()),
                name: Some("provider".to_string()),
                generation: Some(3),
                ..Default::default()
            },
            spec: EdgionAIServiceSpec {
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
                    pricing: None,
                }],
                defaults: AiServiceDefaults::default(),
            },
            status: None,
        }
    }

    fn condition<'a>(service: &'a EdgionAIService, type_: &str) -> &'a Condition {
        service
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == type_)
            .unwrap()
    }

    async fn parse(
        handler: &EdgionAIServiceHandler,
        service: EdgionAIService,
        ctx: &HandlerContext,
    ) -> EdgionAIService {
        match handler.parse(service, ctx).await {
            ProcessResult::Continue(service) => service,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        }
    }

    #[tokio::test]
    async fn resolves_valid_secret_and_registers_reference() {
        let (key, credential) = secret("tenant-a", "provider-key", Some("Opaque"), Some(b"private-value"));
        let (ctx, _) = context(HashMap::from([(key, credential)]));
        let handler = EdgionAIServiceHandler::new();
        let parsed = parse(&handler, service("provider-key", None), &ctx).await;

        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_some());
        assert_eq!(condition(&parsed, condition_types::RESOLVED_REFS).status, "True");
        let partially_invalid = condition(&parsed, condition_types::PARTIALLY_INVALID);
        assert_eq!(partially_invalid.status, "False");
        assert_eq!(partially_invalid.reason, condition_reasons::ALL_RULES_VALID);
        let owner = ResourceRef::new(
            ResourceKind::EdgionAIService,
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
            &EdgionAIServiceHandler::new(),
            service_with_credentials(vec![
                credential("primary", "primary-key", None),
                credential("standby", "missing-key", None),
            ]),
            &ctx,
        )
        .await;

        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_some());
        assert!(parsed.spec.credential_pool.credentials[1].credential.secret.is_none());
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
        let (ctx, _) = context(HashMap::from([(local_key, local_secret), (shared_key, shared_secret)]));
        let parsed = parse(
            &EdgionAIServiceHandler::new(),
            service_with_credentials(vec![
                credential("primary", "primary-key", None),
                credential("standby", "standby-key", Some("shared")),
            ]),
            &ctx,
        )
        .await;

        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_some());
        assert!(parsed.spec.credential_pool.credentials[1].credential.secret.is_none());
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
    }

    #[tokio::test]
    async fn completely_unresolved_pool_fails_closed_with_resolved_refs_false() {
        let (key, invalid_secret) = secret("tenant-a", "invalid-key", Some("Opaque"), None);
        let (ctx, _) = context(HashMap::from([(key, invalid_secret)]));
        let parsed = parse(
            &EdgionAIServiceHandler::new(),
            service_with_credentials(vec![
                credential("primary", "missing-key", None),
                credential("standby", "invalid-key", None),
            ]),
            &ctx,
        )
        .await;

        assert!(parsed
            .spec
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
        let parsed = parse(
            &EdgionAIServiceHandler::new(),
            service_with_credentials(Vec::new()),
            &ctx,
        )
        .await;

        let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, condition_reasons::INVALID);
        assert_eq!(resolved.message, "No usable credentials");
    }

    #[tokio::test]
    async fn missing_secret_stays_pending_and_registered() {
        let (ctx, _) = context(HashMap::new());
        let parsed = parse(&EdgionAIServiceHandler::new(), service("missing", None), &ctx).await;
        let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, condition_reasons::SECRET_NOT_FOUND);
        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_none());
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
            let parsed = parse(&EdgionAIServiceHandler::new(), service(&name, None), &ctx).await;
            let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
            assert_eq!(resolved.reason, expected_reason);
            assert!(!resolved.message.contains("private-value"));
            assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_none());
        }
    }

    #[tokio::test]
    async fn cross_namespace_denial_fails_closed_and_keeps_dependency() {
        let (key, credential) = secret("shared", "provider-key", None, Some(b"private-value"));
        let (ctx, _) = context(HashMap::from([(key, credential)]));
        let parsed = parse(
            &EdgionAIServiceHandler::new(),
            service("provider-key", Some("shared")),
            &ctx,
        )
        .await;
        let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.reason, condition_reasons::REF_NOT_PERMITTED);
        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_none());
        assert_eq!(ctx.secret_ref_manager().get_refs("shared/provider-key").len(), 1);
    }

    #[tokio::test]
    async fn reconcile_clears_stale_material_and_dropped_reference() {
        let (old_key, old_secret) = secret("tenant-a", "old-key", None, Some(b"old-private"));
        let (ctx, _) = context(HashMap::from([(old_key, old_secret)]));
        let handler = EdgionAIServiceHandler::new();
        let mut parsed = parse(&handler, service("old-key", None), &ctx).await;
        assert!(parsed.spec.credential_pool.credentials[0].credential.secret.is_some());

        parsed.spec.credential_pool.credentials[0].credential.secret_ref.name = "new-missing".to_string();
        let reparsed = parse(&handler, parsed, &ctx).await;
        assert!(reparsed.spec.credential_pool.credentials[0].credential.secret.is_none());
        assert!(ctx.secret_ref_manager().get_refs("tenant-a/old-key").is_empty());
        assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/new-missing").len(), 1);
    }

    #[tokio::test]
    async fn secret_repair_transitions_resolved_refs_false_to_true() {
        let (ctx, runtime) = context(HashMap::new());
        let handler = EdgionAIServiceHandler::new();
        let pending = parse(&handler, service("provider-key", None), &ctx).await;
        assert_eq!(condition(&pending, condition_types::RESOLVED_REFS).status, "False");

        let (key, credential) = secret("tenant-a", "provider-key", None, Some(b"repaired-private"));
        runtime.secret_store.replace_all(HashMap::from([(key, credential)]));
        let repaired = parse(&handler, pending, &ctx).await;
        assert_eq!(condition(&repaired, condition_types::RESOLVED_REFS).status, "True");
        assert!(repaired.spec.credential_pool.credentials[0].credential.secret.is_some());
    }

    #[tokio::test]
    async fn delete_clears_reference() {
        let (ctx, _) = context(HashMap::new());
        let handler = EdgionAIServiceHandler::new();
        let service = parse(&handler, service("provider-key", None), &ctx).await;
        assert_eq!(ctx.secret_ref_manager().get_refs("tenant-a/provider-key").len(), 1);
        handler.on_delete(&service, &ctx).await;
        assert!(ctx.secret_ref_manager().get_refs("tenant-a/provider-key").is_empty());
    }

    #[test]
    fn preparse_and_status_own_accepted_condition() {
        let (ctx, _) = context(HashMap::new());
        let handler = EdgionAIServiceHandler::new();
        let mut service = service("provider-key", None);
        assert!(handler.preparse(&mut service, &ctx).is_empty());
        handler.update_status(&mut service, &ctx, &[]);
        assert_eq!(condition(&service, condition_types::ACCEPTED).status, "True");

        service.spec.models.clear();
        let errors = handler.preparse(&mut service, &ctx);
        handler.update_status(&mut service, &ctx, &errors);
        let accepted = condition(&service, condition_types::ACCEPTED);
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "Invalid");
    }
}
