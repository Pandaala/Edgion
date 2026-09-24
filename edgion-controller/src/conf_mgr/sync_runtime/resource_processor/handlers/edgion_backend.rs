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
        ctx.secret_ref_manager().clear_value_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&owner);
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
        ctx.secret_ref_manager().clear_value_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&owner);
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_test.rs"]
mod tests;
