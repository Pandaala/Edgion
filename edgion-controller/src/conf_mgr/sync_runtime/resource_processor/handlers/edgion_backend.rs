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
    A2aBackendReconcileOutcome, A2aTargetResolutionClass, AiBackendReconcileOutcome, AiCredentialResolutionClass,
    AiCredentialResolutionError, EdgionBackendReconcileOutcome, EdgionBackendStatus, ResolvedA2aBackend,
    ResolvedA2aTarget, AI_CREDENTIAL_ALIAS_MAX_BYTES,
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
        backend.spec.resolved_a2a = None;
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&owner);

        if let Some(a2a) = backend.spec.a2a.as_ref() {
            let Some(service_ref) = a2a.target.service_ref.as_ref() else {
                if let Ok(normalized_endpoint) = a2a.normalized_endpoint() {
                    backend.spec.resolved_a2a = Some(ResolvedA2aBackend {
                        target: ResolvedA2aTarget::External { normalized_endpoint },
                    });
                }
                backend.spec.reconcile_outcome =
                    Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome::default()));
                return ProcessResult::Continue(backend);
            };

            let owner_ns = owner.namespace.as_deref().unwrap_or("default");
            let service_ns = service_ref.namespace.as_deref().unwrap_or(owner_ns);
            let service_key = format!("{service_ns}/{}", service_ref.name);
            ctx.runtime()
                .service_ref_mgr
                .add_ref(service_key.clone(), owner.clone());
            if service_ns != owner_ns {
                ctx.runtime()
                    .cross_ns_ref_mgr
                    .add_ref(service_ns.to_string(), owner.clone());
                let permitted = !crate::config::is_reference_grant_validation_enabled()
                    || ctx.runtime().ref_grant_store.check_reference_allowed(
                        owner_ns,
                        "edgion.io",
                        ResourceKind::EdgionBackend.as_str(),
                        service_ns,
                        "",
                        "Service",
                        Some(&service_ref.name),
                    );
                if !permitted {
                    backend.spec.reconcile_outcome =
                        Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome {
                            error: Some(A2aTargetResolutionClass::RefNotPermitted),
                        }));
                    return ProcessResult::Continue(backend);
                }
            }

            let service_port = ctx
                .runtime()
                .processor_registry
                .get("Service")
                .and_then(|processor| processor.get_raw_any(&service_key))
                .and_then(|raw| raw.downcast::<k8s_openapi::api::core::v1::Service>().ok())
                .and_then(|service| {
                    service
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.ports.as_ref())
                        .and_then(|ports| {
                            ports
                                .iter()
                                .find(|port| port.port == i32::from(service_ref.port))
                                .cloned()
                        })
                });
            if service_port
                .as_ref()
                .and_then(|port| port.app_protocol.as_deref())
                .is_some_and(|protocol| {
                    matches!(
                        protocol.to_ascii_lowercase().as_str(),
                        "ws" | "wss" | "kubernetes.io/ws" | "kubernetes.io/wss"
                    )
                })
            {
                backend.spec.reconcile_outcome = Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome {
                    error: Some(A2aTargetResolutionClass::UnsupportedProtocol),
                }));
            } else if service_port.is_some() {
                backend.spec.resolved_a2a = Some(ResolvedA2aBackend {
                    target: ResolvedA2aTarget::Service {
                        namespace: service_ns.to_string(),
                        name: service_ref.name.clone(),
                        port: service_ref.port,
                    },
                });
                backend.spec.reconcile_outcome =
                    Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome::default()));
            } else {
                backend.spec.reconcile_outcome = Some(EdgionBackendReconcileOutcome::A2a(A2aBackendReconcileOutcome {
                    error: Some(A2aTargetResolutionClass::TargetNotFound),
                }));
            }
            return ProcessResult::Continue(backend);
        }

        let Some(ai) = backend.spec.ai.as_mut() else {
            return ProcessResult::Continue(backend);
        };

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
        ctx.runtime().service_ref_mgr.clear_resource_refs(&owner);
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
        let structural_valid = validation_errors.is_empty()
            && usize::from(backend.spec.ai.is_some()) + usize::from(backend.spec.a2a.is_some()) == 1;

        if backend.spec.a2a.is_some() {
            let (mut accepted, mut resolved_refs) = if structural_valid {
                let error = match outcome {
                    Some(EdgionBackendReconcileOutcome::A2a(outcome)) => outcome.error,
                    _ => Some(A2aTargetResolutionClass::TargetNotFound),
                };
                (
                    condition_true(
                        condition_types::ACCEPTED,
                        condition_reasons::ACCEPTED,
                        "Resource accepted",
                        generation,
                    ),
                    match error {
                        None => condition_true(
                            condition_types::RESOLVED_REFS,
                            condition_reasons::RESOLVED_REFS,
                            "A2A target resolved",
                            generation,
                        ),
                        Some(error) => condition_false(
                            condition_types::RESOLVED_REFS,
                            error.reason(),
                            match error {
                                A2aTargetResolutionClass::RefNotPermitted => "A2A Service reference is not permitted",
                                A2aTargetResolutionClass::TargetNotFound => "A2A Service target or port was not found",
                                A2aTargetResolutionClass::UnsupportedProtocol => {
                                    "A2A Service target uses an unsupported protocol"
                                }
                            },
                            generation,
                        ),
                    },
                )
            } else {
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
                )
            };
            let status = backend.status.get_or_insert_with(EdgionBackendStatus::default);
            let old = std::mem::take(&mut status.conditions);
            preserve_transition_time(&old, &mut accepted);
            preserve_transition_time(&old, &mut resolved_refs);
            status.conditions = vec![accepted, resolved_refs];
            return;
        }

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

        let status = backend.status.get_or_insert_with(EdgionBackendStatus::default);
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
        A2aBackendSpec, A2aBackendTarget, A2aServiceReference, AiBackendDefaults, AiBackendSpec, AiCredential,
        AiCredentialPool, AiModel, AiProvider, EdgionBackendSpec,
    };
    use edgion_resources::resources::edgion_plugins::OnRedisFailure;
    use k8s_openapi::api::core::v1::{Secret, Service, ServicePort, ServiceSpec};
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

    fn install_services(runtime: &Arc<ControllerRuntime>, services: Vec<Service>) {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::ServiceHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let processor = Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            16,
            Arc::new(ServiceHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        for service in services {
            let key = format!(
                "{}/{}",
                service.metadata.namespace.as_deref().unwrap_or("default"),
                service.metadata.name.as_deref().unwrap_or_default()
            );
            raw.upsert(key, Arc::new(service));
        }
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);
    }

    fn service(namespace: &str, name: &str, ports: &[i32]) -> Service {
        Service {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                ports: Some(
                    ports
                        .iter()
                        .map(|port| ServicePort {
                            port: *port,
                            ..Default::default()
                        })
                        .collect(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn service_a2a_backend(namespace: Option<&str>, service_name: &str, port: u16) -> EdgionBackend {
        a2a_backend(
            A2aBackendTarget {
                service_ref: Some(A2aServiceReference {
                    name: service_name.to_string(),
                    namespace: namespace.map(str::to_string),
                    port,
                }),
                endpoint: None,
            },
            false,
        )
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
                ai: Some(AiBackendSpec {
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
                }),
                a2a: None,
                resolved_a2a: None,
                reconcile_outcome: None,
            },
            status: None,
        }
    }

    fn a2a_backend(target: A2aBackendTarget, allow_insecure_http: bool) -> EdgionBackend {
        EdgionBackend {
            metadata: ObjectMeta {
                namespace: Some("tenant-a".to_string()),
                name: Some("agent".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: EdgionBackendSpec {
                ai: None,
                a2a: Some(A2aBackendSpec {
                    target,
                    allow_insecure_http,
                }),
                resolved_a2a: None,
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

        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_some()
        );
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
    async fn resolves_external_a2a_without_ai_status_or_dependencies() {
        let (ctx, _) = context(HashMap::new());
        let parsed = parse(
            &EdgionBackendHandler::new(),
            a2a_backend(
                A2aBackendTarget {
                    service_ref: None,
                    endpoint: Some("https://agent.example.test/base/".to_string()),
                },
                false,
            ),
            &ctx,
        )
        .await;

        assert_eq!(
            parsed.spec.resolved_a2a,
            Some(ResolvedA2aBackend {
                target: ResolvedA2aTarget::External {
                    normalized_endpoint: "https://agent.example.test/base".to_string(),
                },
            })
        );
        assert_eq!(condition(&parsed, condition_types::ACCEPTED).status, "True");
        assert_eq!(condition(&parsed, condition_types::RESOLVED_REFS).status, "True");
        assert_eq!(parsed.status.as_ref().unwrap().conditions.len(), 2);
    }

    #[tokio::test]
    async fn missing_a2a_service_keeps_dependency_and_withdraws_resolved_target() {
        let (ctx, runtime) = context(HashMap::new());
        let parsed = parse(
            &EdgionBackendHandler::new(),
            a2a_backend(
                A2aBackendTarget {
                    service_ref: Some(A2aServiceReference {
                        name: "agent-svc".to_string(),
                        namespace: None,
                        port: 8080,
                    }),
                    endpoint: None,
                },
                false,
            ),
            &ctx,
        )
        .await;

        assert!(parsed.spec.resolved_a2a.is_none());
        assert_eq!(condition(&parsed, condition_types::ACCEPTED).status, "True");
        let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "TargetNotFound");
        let owner = ResourceRef::new(
            ResourceKind::EdgionBackend,
            Some("tenant-a".to_string()),
            "agent".to_string(),
        );
        assert!(runtime.service_ref_mgr.get_refs("tenant-a/agent-svc").contains(&owner));
    }

    #[tokio::test]
    async fn same_namespace_a2a_service_and_port_resolve() {
        let (ctx, runtime) = context(HashMap::new());
        install_services(&runtime, vec![service("tenant-a", "agent-svc", &[8080])]);
        let parsed = parse(
            &EdgionBackendHandler::new(),
            service_a2a_backend(None, "agent-svc", 8080),
            &ctx,
        )
        .await;

        assert_eq!(
            parsed.spec.resolved_a2a,
            Some(ResolvedA2aBackend {
                target: ResolvedA2aTarget::Service {
                    namespace: "tenant-a".to_string(),
                    name: "agent-svc".to_string(),
                    port: 8080,
                },
            })
        );
        assert_eq!(condition(&parsed, condition_types::RESOLVED_REFS).status, "True");
    }

    #[tokio::test]
    async fn a2a_service_rejects_all_websocket_app_protocol_aliases() {
        for app_protocol in ["ws", "wss", "kubernetes.io/ws", "kubernetes.io/wss"] {
            let (ctx, runtime) = context(HashMap::new());
            let mut websocket = service("tenant-a", "agent-ws", &[8080]);
            websocket.spec.as_mut().unwrap().ports.as_mut().unwrap()[0].app_protocol = Some(app_protocol.to_string());
            install_services(&runtime, vec![websocket]);

            let parsed = parse(
                &EdgionBackendHandler::new(),
                service_a2a_backend(None, "agent-ws", 8080),
                &ctx,
            )
            .await;
            assert!(parsed.spec.resolved_a2a.is_none(), "accepted {app_protocol}");
            let resolved = condition(&parsed, condition_types::RESOLVED_REFS);
            assert_eq!(resolved.status, "False", "appProtocol={app_protocol}");
            assert_eq!(resolved.reason, "UnsupportedProtocol", "appProtocol={app_protocol}");
        }
    }

    #[tokio::test]
    async fn a2a_service_missing_requested_port_is_unresolved() {
        let (ctx, runtime) = context(HashMap::new());
        install_services(&runtime, vec![service("tenant-a", "agent-svc", &[9090])]);
        let parsed = parse(
            &EdgionBackendHandler::new(),
            service_a2a_backend(None, "agent-svc", 8080),
            &ctx,
        )
        .await;

        assert!(parsed.spec.resolved_a2a.is_none());
        assert_eq!(
            condition(&parsed, condition_types::RESOLVED_REFS).reason,
            "TargetNotFound"
        );
    }

    #[tokio::test]
    async fn cross_namespace_a2a_service_denied_then_allowed_by_reference_grant() {
        let (ctx, runtime) = context(HashMap::new());
        install_services(&runtime, vec![service("shared", "agent-svc", &[8080])]);
        let handler = EdgionBackendHandler::new();
        let backend = service_a2a_backend(Some("shared"), "agent-svc", 8080);

        let denied = parse(&handler, backend.clone(), &ctx).await;
        assert!(denied.spec.resolved_a2a.is_none());
        assert_eq!(
            condition(&denied, condition_types::RESOLVED_REFS).reason,
            "RefNotPermitted"
        );

        let grant: edgion_resources::resources::ReferenceGrant = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "ReferenceGrant",
            "metadata": {"name": "allow-agent", "namespace": "shared"},
            "spec": {
                "from": [{"group": "edgion.io", "kind": "EdgionBackend", "namespace": "tenant-a"}],
                "to": [{"group": "", "kind": "Service", "name": "agent-svc"}]
            }
        }))
        .unwrap();
        runtime
            .ref_grant_store
            .upsert("shared/allow-agent".to_string(), Arc::new(grant));

        let allowed = parse(&handler, backend, &ctx).await;
        assert!(matches!(
            allowed.spec.resolved_a2a.as_ref().map(|resolved| &resolved.target),
            Some(ResolvedA2aTarget::Service { namespace, name, port })
                if namespace == "shared" && name == "agent-svc" && *port == 8080
        ));
        assert_eq!(condition(&allowed, condition_types::RESOLVED_REFS).status, "True");
    }

    #[tokio::test]
    async fn a2a_service_retarget_and_delete_replace_dependency_set() {
        let (ctx, runtime) = context(HashMap::new());
        install_services(
            &runtime,
            vec![
                service("tenant-a", "agent-a", &[8080]),
                service("tenant-a", "agent-b", &[8080]),
            ],
        );
        let handler = EdgionBackendHandler::new();
        let first = parse(&handler, service_a2a_backend(None, "agent-a", 8080), &ctx).await;
        assert_eq!(runtime.service_ref_mgr.get_refs("tenant-a/agent-a").len(), 1);

        let second = parse(&handler, service_a2a_backend(None, "agent-b", 8080), &ctx).await;
        assert!(runtime.service_ref_mgr.get_refs("tenant-a/agent-a").is_empty());
        assert_eq!(runtime.service_ref_mgr.get_refs("tenant-a/agent-b").len(), 1);

        handler.on_delete(&second, &ctx).await;
        assert!(runtime.service_ref_mgr.get_refs("tenant-a/agent-b").is_empty());

        let recreated = parse(&handler, service_a2a_backend(None, "agent-a", 8080), &ctx).await;
        assert!(recreated.spec.resolved_a2a.is_some());
        assert_eq!(runtime.service_ref_mgr.get_refs("tenant-a/agent-a").len(), 1);
        assert!(first.spec.resolved_a2a.is_some());
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

        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_some()
        );
        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[1]
                .credential
                .secret
                .is_none()
        );
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

        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_some()
        );
        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[1]
                .credential
                .secret
                .is_none()
        );
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
            .as_ref()
            .expect("AI backend")
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
        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_none()
        );
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
            assert!(
                parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                    .credential
                    .secret
                    .is_none()
            );
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
        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_none()
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("shared/provider-key").len(), 1);
    }

    #[tokio::test]
    async fn reconcile_clears_stale_material_and_dropped_reference() {
        let (old_key, old_secret) = secret("tenant-a", "old-key", None, Some(b"old-private"));
        let (ctx, _) = context(HashMap::from([(old_key, old_secret)]));
        let handler = EdgionBackendHandler::new();
        let mut parsed = parse(&handler, backend("old-key", None), &ctx).await;
        assert!(
            parsed.spec.ai.as_ref().expect("AI backend").credential_pool.credentials[0]
                .credential
                .secret
                .is_some()
        );

        parsed.spec.ai.as_mut().expect("AI backend").credential_pool.credentials[0]
            .credential
            .secret_ref
            .name = "new-missing".to_string();
        let reparsed = parse(&handler, parsed, &ctx).await;
        assert!(reparsed
            .spec
            .ai
            .as_ref()
            .expect("AI backend")
            .credential_pool
            .credentials[0]
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
        assert!(repaired
            .spec
            .ai
            .as_ref()
            .expect("AI backend")
            .credential_pool
            .credentials[0]
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

        backend.spec.ai.as_mut().expect("AI backend").models.clear();
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
            .conditions
            .push(condition_true("StaleCondition", "Stale", "stale", Some(1)));
        for condition in &mut backend.status.as_mut().unwrap().conditions {
            condition.last_transition_time = SENTINEL.to_string();
        }

        handler.update_status(&mut backend, &ctx, &[]);
        let conditions = &backend.status.as_ref().unwrap().conditions;
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
        let conditions = &backend.status.as_ref().unwrap().conditions;
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
