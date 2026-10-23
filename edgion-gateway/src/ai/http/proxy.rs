use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::ai::protocols::openai::request;
use crate::ai::runtime::attempt_plan::AiOperation;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_backend::AiProvider;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{
    AiApi, AiBackendSelection, AiProxyConfig, AiUsageObservation, OnUnknownModel, PluginValidation,
};
use pingora_http::ResponseHeader;

use crate::ai::quota::{CompiledAiQuotaPlan, QuotaAdmission, QuotaExceededAction};
use crate::ai::routing::candidate::{invalid_weight_gate, CandidateEligibility, ConformanceGate, RouteCandidate};
use crate::ai::routing::model_router::{ModelRouteDecision, ModelRouteTable};
use crate::ai::runtime::attempt::{AttemptScheduler, BackendCredentials, MAX_AI_ROUTE_RETRIES};
use crate::ai::{
    get_adaptive_telemetry_store, global_ai_backend_store, AdaptiveTelemetryKey, AdaptiveTelemetryStore, AiAttemptPlan,
    AiBackendStore, AiCandidateModel, AiPlanCandidate, AiPlanError, AiProxyError, ReadyAiAttemptPlan,
};
use crate::backends::policy::backend_traffic::get_backend_traffic_policy_store;
use crate::backends::runtime::CompiledBackendTarget;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

const DEFAULT_MAXIMUM_REQUEST_DURATION: Duration = Duration::from_secs(300);

fn ai_candidate_spec(backend: &crate::routes::http::match_unit::CompiledBackend) -> Option<(usize, i32, &str)> {
    if !backend.is_permitted_ai_backend() {
        return None;
    }
    match backend.target() {
        CompiledBackendTarget::EdgionBackend(target) => {
            Some((backend.slot(), backend.schema().weight.unwrap_or(1), target.key()))
        }
        CompiledBackendTarget::Service(_) | CompiledBackendTarget::Invalid { .. } => None,
    }
}

fn gate_candidate(
    order: usize,
    backend: &crate::routes::http::match_unit::CompiledBackend,
    runtime_available: bool,
) -> Result<RouteCandidate, ()> {
    Ok(RouteCandidate {
        slot: u8::try_from(backend.slot()).map_err(|_| ())?,
        order: u8::try_from(order).map_err(|_| ())?,
        weight: backend.schema().weight.unwrap_or(1),
        eligibility: CandidateEligibility {
            valid_ref: backend.is_permitted_ai_backend() && runtime_available,
            ..CandidateEligibility::default()
        },
        adaptive: Default::default(),
        cost: None,
    })
}

fn visible_model_ids(
    model_routes: &ModelRouteTable,
    backends: &[crate::routes::http::match_unit::CompiledBackend],
    headers: &http::HeaderMap,
    lookup: impl Fn(&str) -> Option<std::sync::Arc<crate::ai::RuntimeAiBackend>>,
) -> Vec<String> {
    let mut proposed = BTreeSet::new();
    for backend in backends.iter().filter(|backend| backend.is_permitted_ai_backend()) {
        let Some((_, _, key)) = ai_candidate_spec(backend) else {
            continue;
        };
        if let Some(runtime) = lookup(key) {
            proposed.extend(runtime.public_model_ids.iter().cloned());
        }
    }
    proposed.extend(model_routes.header_applicable_patterns(headers).map(str::to_string));

    proposed
        .into_iter()
        .filter(|model_id| {
            if model_id.ends_with('*') && model_routes.wildcard_is_partially_shadowed(model_id, headers) {
                return false;
            }
            let Ok(decision) = model_routes.decide(model_id, headers, backends) else {
                return false;
            };
            backends.iter().any(|backend| {
                let weight = decision.weights.get(backend.slot()).copied().unwrap_or(0);
                if weight <= 0 || !backend.is_permitted_ai_backend() {
                    return false;
                }
                let Some((_, _, key)) = ai_candidate_spec(backend) else {
                    return false;
                };
                lookup(key).is_some_and(|runtime| {
                    let resolved = runtime.resolve_model(decision.identity_model);
                    let has_symbolic_owner = !model_id.ends_with('*')
                        || decision.rewrite_requires_canonical
                        || runtime
                            .wildcard_models
                            .iter()
                            .any(|alias| alias.pattern == *model_id && alias.model.public);
                    resolved.is_some_and(|model| model.public)
                        && has_symbolic_owner
                        && (!decision.rewrite_requires_canonical
                            || runtime.has_canonical_model(decision.identity_model))
                })
            })
        })
        .collect()
}

fn encode_model_list(visible: Vec<String>) -> serde_json::Result<Vec<u8>> {
    use serde_json::{Map, Value};

    let data = visible
        .into_iter()
        .map(|id| {
            Value::Object(Map::from_iter([
                ("id".into(), Value::String(id)),
                ("object".into(), Value::String("model".into())),
                ("created".into(), Value::from(0)),
                ("owned_by".into(), Value::String("edgion".into())),
            ]))
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&Value::Object(Map::from_iter([
        ("object".into(), Value::String("list".into())),
        ("data".into(), Value::Array(data)),
    ])))
}

fn unknown_model_passthrough_allowed<'a>(
    rewrite_requires_canonical: bool,
    configured: OnUnknownModel,
    candidates: impl Iterator<Item = (i32, Option<&'a crate::ai::RuntimeAiBackend>)>,
) -> bool {
    !rewrite_requires_canonical
        && configured == OnUnknownModel::Passthrough
        && candidates.filter(|(weight, _)| *weight > 0).all(|(_, backend)| {
            backend.is_some_and(|backend| matches!(backend.provider, AiProvider::OpenAI | AiProvider::OpenAICompatible))
        })
}

fn has_positive_weight_service_backend(backends: &[crate::routes::http::match_unit::CompiledBackend]) -> bool {
    backends.iter().any(|backend| {
        backend.schema().weight.unwrap_or(1) > 0 && matches!(backend.target(), CompiledBackendTarget::Service(_))
    })
}

/// Backend publication can precede route facts; check the declaration union
/// against the request-captured generation before endpoint/model filtering.
fn captured_backend_contract_valid(
    config: &AiProxyConfig,
    backends: &[crate::routes::http::match_unit::CompiledBackend],
    snapshot: &crate::ai::AiBackendSnapshot,
) -> bool {
    for backend in backends.iter() {
        let promoted = config
            .model_routes
            .iter()
            .flat_map(|route| route.action.candidates.iter().flatten())
            .any(|candidate| {
                backend.identity().matches_ai_selector(&candidate.backend)
                    && candidate.weight.unwrap_or(backend.schema().weight.unwrap_or(1)) > 0
            });
        if backend.schema().weight.unwrap_or(1) <= 0 && !promoted {
            continue;
        }
        if let Some((_, _, key)) = ai_candidate_spec(backend) {
            if let Some(runtime) = snapshot.get_by_key(key) {
                if !config.accepts_provider(runtime.provider)
                    || (runtime.has_credential_tpm && config.usage_observation == AiUsageObservation::Off)
                {
                    return false;
                }
            }
        }
    }
    true
}

/// Request-stage AI semantics. Provider-specific mutation happens per attempt.
pub struct AiProxy {
    config: AiProxyConfig,
    model_routes: ModelRouteTable,
    quota: Option<CompiledAiQuotaPlan>,
}

impl AiProxy {
    fn from_config(config: &AiProxyConfig, entry_scope: &str) -> Option<Self> {
        if config.validation_error().is_some() {
            return None;
        }
        let quota = match config.token_quota.as_ref() {
            Some(quota) => Some(CompiledAiQuotaPlan::compile(quota, entry_scope)?),
            None => None,
        };
        Some(Self {
            config: config.clone(),
            model_routes: ModelRouteTable::compile(&config.model_routes)?,
            quota,
        })
    }

    pub fn create(config: &AiProxyConfig, _namespace: &str, entry_scope: &str) -> Option<Box<dyn RequestFilter>> {
        Self::from_config(config, entry_scope).map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>)
    }

    fn reject(
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        classification: &'static str,
        error: AiProxyError,
    ) -> PluginRunningResult {
        log.push_err(classification);
        crate::ai::protocols::error::ai_error_response(session, error)
    }

    fn install_rejected_plan(
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        error: AiPlanError,
    ) -> PluginRunningResult {
        if session.ai().set_ai_attempt_plan(AiAttemptPlan::Rejected(error)) {
            log.push("planned");
            PluginRunningResult::GoodNext
        } else {
            Self::reject(session, log, "plan-locked", AiProxyError::BackendUnavailable)
        }
    }

    async fn write_model_list(
        &self,
        snapshot: &crate::ai::AiBackendSnapshot,
        backends: &[crate::routes::http::match_unit::CompiledBackend],
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let visible = visible_model_ids(&self.model_routes, backends, session.request_header_map(), |key| {
            snapshot.get_by_key(key)
        });
        let Ok(body) = encode_model_list(visible) else {
            return Self::reject(session, log, "model-list-encode", AiProxyError::BackendUnavailable);
        };
        let body = Bytes::from(body);
        let Ok(mut response) = ResponseHeader::build(200, None) else {
            return PluginRunningResult::ErrTerminateRequest;
        };
        let _ = response.insert_header("content-type", "application/json");
        let _ = response.insert_header("content-length", body.len().to_string());
        if session.write_response_header(Box::new(response), false).await.is_err()
            || session.write_response_body(Some(body), true).await.is_err()
        {
            log.push_err("model-list-write");
        }
        PluginRunningResult::ErrTerminateRequest
    }

    async fn run_request_with_store_and_entropy(
        &self,
        store: &AiBackendStore,
        entropy: u128,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if !session.ai().capture_ai_client_api(self.config.api) {
            return Self::reject(session, log, "client-api-conflict", AiProxyError::BackendUnavailable);
        }
        if session.ctx().has_ai_attempt_plan() {
            return Self::reject(session, log, "duplicate-ai-proxy", AiProxyError::BackendUnavailable);
        }

        let Some(route_unit) = session.ctx().route_unit.clone() else {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        };
        let (Some(terminal_route_uid), Some(terminal_rule_identity)) = (
            route_unit.rule.resolved_terminal_route_uid.clone(),
            route_unit.rule.resolved_terminal_rule_identity.clone(),
        ) else {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        };
        let backends = route_unit.backends.clone();
        if has_positive_weight_service_backend(&backends) {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        }

        // Capture one runtime generation for the complete request decision.
        let snapshot = store.snapshot();

        if !captured_backend_contract_valid(&self.config, &backends, &snapshot) {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        }

        if self.config.api == AiApi::OpenaiChat && session.get_method() == "GET" && session.get_path() == "/v1/models" {
            return self.write_model_list(&snapshot, &backends, session, log).await;
        }

        let (operation, path) = match self.config.api {
            AiApi::OpenaiChat => (AiOperation::ChatCompletions, "/v1/chat/completions"),
            AiApi::OpenaiResponses => (AiOperation::Responses, "/v1/responses"),
            AiApi::AnthropicMessages => (AiOperation::Messages, "/v1/messages"),
        };
        if matches!(self.config.api, AiApi::OpenaiResponses | AiApi::AnthropicMessages)
            && (session
                .ctx()
                .ai_request_guard_invoked
                .load(std::sync::atomic::Ordering::Relaxed)
                || session
                    .ctx()
                    .ai_state()
                    .is_some_and(|state| state.semantic_guard_claimed))
        {
            return Self::reject(
                session,
                log,
                "responses-guard-unsupported",
                AiProxyError::UnsupportedParameter,
            );
        }
        if session.get_method() != "POST" || session.get_path() != path {
            return Self::reject(session, log, "endpoint-unsupported", AiProxyError::UnsupportedEndpoint);
        }
        if session.declares_body() == Some(false) {
            return Self::reject(session, log, "body-required", AiProxyError::MalformedBody);
        }
        let before_body_generation = session.ctx().request_body.generation();
        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(crate::request_body::BodyReadError::BufferLimitExceeded { .. }) => {
                return Self::reject(session, log, "body-too-large", AiProxyError::RequestTooLarge);
            }
            Err(_) => return Self::reject(session, log, "body-unavailable", AiProxyError::BackendUnavailable),
        };
        let body_generation = session.ctx().request_body.generation();
        if before_body_generation.is_some() && before_body_generation != body_generation {
            session.record_error(edgion_resources::EdgionErrorCode::BodyMutationConflict);
            return Self::reject(
                session,
                log,
                "body generation changed",
                AiProxyError::BackendUnavailable,
            );
        }
        // If an AiGuard settled a decision for this request, the body it
        // settled on must still be the body being canonicalized here. Anything
        // that mutated it in between — a rewrite plugin ordered between the
        // two — would silently discard the guard's mask, because the provider
        // body is derived from the canonical bytes captured below. This is the
        // exact, scope-independent ordering guarantee; plugin ordering alone
        // cannot provide it.
        if let Some(expected) = session.ctx().ai_state().and_then(|state| state.guard_body_generation) {
            if session.ctx().request_body.generation() != Some(expected) {
                session.record_error(edgion_resources::EdgionErrorCode::BodyMutationConflict);
                return Self::reject(session, log, "guard body changed", AiProxyError::BackendUnavailable);
            }
        }

        let mut canonical = match match self.config.api {
            AiApi::OpenaiResponses => canonical_responses_request(body),
            AiApi::AnthropicMessages => canonical_messages_request(body),
            AiApi::OpenaiChat => canonical_openai_request(body, self.config.inject_usage_tracking),
        } {
            Ok(canonical) => canonical,
            Err(error) => return Self::reject(session, log, "request-invalid", error),
        };

        if self.config.usage_observation == AiUsageObservation::Provider
            || self.config.backend_selection.selection_type == AiBackendSelection::CostWeighted
        {
            canonical.input_chars = match checked_utf8_chars(&canonical.body) {
                Ok(chars) => chars,
                Err(error) => return Self::reject(session, log, "request-invalid", error),
            };
        }

        if canonical.continuation {
            if let Err(error) = validate_continuation(&self.config, &backends, &snapshot, &canonical.logical_model) {
                return Self::reject(session, log, "continuation-unsupported", error);
            }
        }
        let Ok(decision) =
            self.model_routes
                .decide(&canonical.requested_model, session.request_header_map(), &backends)
        else {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        };
        let ModelRouteDecision {
            identity_model,
            weights,
            rewrite_requires_canonical,
            ..
        } = decision;
        let model_rewrite = (identity_model != canonical.logical_model).then(|| identity_model.to_owned());
        if model_rewrite
            .as_deref()
            .is_some_and(|model| canonical.replace_logical_model(model).is_err())
        {
            return Self::reject(session, log, "model-rewrite-invalid", AiProxyError::BackendUnavailable);
        }

        let mut gate_candidates = Vec::with_capacity(backends.len());
        for (order, backend) in backends.iter().enumerate() {
            let runtime_available = ai_candidate_spec(backend)
                .and_then(|(_, _, key)| snapshot.get_by_key(key))
                .is_some();
            let Ok(mut candidate) = gate_candidate(order, backend, runtime_available) else {
                return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
            };
            // A missing runtime object remains an invalid declared share even
            // when an explicit filter omits this syntactically valid slot.
            // Otherwise the filter would silently normalize onto healthy
            // candidates during publication skew.
            if runtime_available {
                candidate.weight = weights.get(backend.slot()).copied().unwrap_or(candidate.weight);
            }
            gate_candidates.push(candidate);
        }
        match invalid_weight_gate(&gate_candidates, entropy) {
            Ok(ConformanceGate::Valid) => {}
            Ok(ConformanceGate::Invalid) => {
                return Self::install_rejected_plan(session, log, AiPlanError::InvalidBackend);
            }
            Err(_) => {
                return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
            }
        }

        if canonical.composite
            && (self.config.token_quota.is_some()
                || self.config.backend_selection.selection_type == AiBackendSelection::LowestUsage
                || backends
                    .iter()
                    .filter(|b| weights.get(b.slot()).copied().unwrap_or(0) > 0)
                    .any(|b| {
                        ai_candidate_spec(b)
                            .and_then(|(_, _, k)| snapshot.get_by_key(k))
                            .is_some_and(|b| b.has_credential_tpm)
                    }))
        {
            return Self::reject(
                session,
                log,
                "composite-accounting-unsupported",
                AiProxyError::UnsupportedParameter,
            );
        }
        let policy_store = get_backend_traffic_policy_store();
        let policy_snapshot = policy_store.snapshot();
        let adaptive_store = get_adaptive_telemetry_store();
        let candidate_specs: Vec<_> = backends
            .iter()
            .enumerate()
            .filter_map(|(index, backend)| {
                let (slot, _, key) = ai_candidate_spec(backend)?;
                let logical = crate::backends::LogicalBackendHandle::new(Arc::clone(&backends), index)?;
                Some((logical, slot, weights.get(slot).copied().unwrap_or(0), key))
            })
            .collect();
        let resolved = snapshot.get_many_by_key(candidate_specs.iter().map(|(_, _, _, key)| *key));
        let passthrough_allowed = unknown_model_passthrough_allowed(
            rewrite_requires_canonical,
            self.config.on_unknown_model,
            candidate_specs
                .iter()
                .zip(&resolved)
                .map(|((_, _, weight, _), backend)| (*weight, backend.as_deref())),
        );
        let mut candidates = candidate_specs
            .into_iter()
            .zip(resolved)
            .enumerate()
            .map(|(order, ((logical, slot, weight, backend_key), backend))| {
                let target = logical.policy_target_key();
                let effective_policy = target.as_ref().and_then(|target| policy_snapshot.get_effective(target));
                let backend_tls_policy =
                    crate::backends::policy::query_backend_tls_policy_for_logical_backend(&logical);
                let outlier_generation = effective_policy
                    .as_ref()
                    .and_then(|policy| policy.ai_credential_outlier.clone())
                    .or_else(|| {
                        target
                            .as_ref()
                            .and_then(|target| snapshot.credential_outlier_handle(target))
                    });
                let credential_outliers = backend
                    .as_ref()
                    .map(|backend| {
                        backend
                            .credential_pool
                            .credentials
                            .iter()
                            .map(|credential| {
                                outlier_generation
                                    .as_ref()
                                    .and_then(|generation| generation.capture(&credential.name))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let policy_ancestor_admitted = crate::backends::policy::ancestor_admitted(
                    &backend_tls_policy,
                    effective_policy.as_deref(),
                    session
                        .ctx()
                        .gateway_info
                        .as_deref()
                        .map(|gateway| gateway.gateway_key()),
                );
                AiPlanCandidate {
                    policy_ancestor_admitted,
                    logical,
                    slot,
                    order: u8::try_from(order).expect("AI route candidates are bounded before body read"),
                    weight,
                    backend_key: backend_key.to_owned(),
                    backend,
                    model: None,
                    effective_policy,
                    backend_tls_policy,
                    adaptive: None,
                    credential_outliers,
                }
            })
            .collect::<Vec<_>>();
        if rewrite_requires_canonical
            && !candidates.iter().any(|candidate| {
                candidate.weight > 0
                    && candidate
                        .backend
                        .as_ref()
                        .is_some_and(|backend| backend.has_canonical_model(&canonical.logical_model))
            })
        {
            return Self::reject(session, log, "rewrite-model-not-found", AiProxyError::ModelNotFound);
        }
        let any_available_service = candidates
            .iter()
            .any(|candidate| candidate.weight > 0 && candidate.backend.is_some());
        let any_model_supported = candidates.iter().any(|candidate| {
            candidate.weight > 0
                && candidate.backend.as_ref().is_some_and(|backend| {
                    backend.resolve_model(&canonical.logical_model).is_some() || passthrough_allowed
                })
        });
        let any_model_eligible = resolve_candidate_models(
            &mut candidates,
            &canonical.logical_model,
            canonical.requested_streaming,
            passthrough_allowed,
        );
        if let Some((reason, error)) = initial_candidate_error(
            canonical.requested_streaming,
            any_available_service,
            any_model_supported,
            any_model_eligible,
        ) {
            return Self::reject(session, log, reason, error);
        }
        capture_adaptive_states(
            adaptive_store.as_ref(),
            self.config.backend_selection.selection_type,
            &terminal_route_uid,
            &terminal_rule_identity,
            &canonical.logical_model,
            &mut candidates,
        );

        let credential_weights: Vec<Vec<u32>> = candidates
            .iter()
            .map(|candidate| {
                candidate
                    .backend
                    .as_ref()
                    .map(|backend| {
                        backend
                            .credential_pool
                            .credentials
                            .iter()
                            .map(|credential| credential.weight)
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        let backends: Vec<_> = credential_weights
            .iter()
            .map(|weights| BackendCredentials { weights })
            .collect();
        let route_retries = if canonical.continuation {
            0
        } else {
            session.ctx().retry_attempts().unwrap_or(0)
        };
        let Ok(route_retries) = u8::try_from(route_retries) else {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        };
        if route_retries > MAX_AI_ROUTE_RETRIES {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        }
        let Ok(scheduler) = AttemptScheduler::new(&backends, route_retries) else {
            return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
        };

        let mut quota_status = None;
        let quota = if let Some(quota) = &self.quota {
            let maximum_request_duration = session
                .ctx()
                .request_timeout()
                .unwrap_or(DEFAULT_MAXIMUM_REQUEST_DURATION);
            match quota
                .admit(session, log, &canonical.logical_model, maximum_request_duration)
                .await
            {
                QuotaAdmission::Ready(request) => Some(request),
                QuotaAdmission::Bypass => {
                    quota_status = Some("bypassed");
                    None
                }
                QuotaAdmission::DependencyBypass => {
                    log.push_err("quota-redis-fail-open");
                    quota_status = Some("fail_open");
                    None
                }
                QuotaAdmission::IdentityMissing => {
                    return Self::reject(session, log, "quota-identity-missing", AiProxyError::QuotaUnavailable);
                }
                QuotaAdmission::Exhausted(retry_after) => match quota.on_exceeded() {
                    QuotaExceededAction::Reject => {
                        return reject_quota_exhausted(session, log, retry_after);
                    }
                    QuotaExceededAction::Throttle { .. } => {
                        return reject_quota_exhausted(session, log, retry_after);
                    }
                    QuotaExceededAction::Downgrade { model } => {
                        if !resolve_candidate_models(&mut candidates, model, canonical.requested_streaming, false) {
                            return Self::reject(
                                session,
                                log,
                                "quota-downgrade-model-unavailable",
                                AiProxyError::BackendUnavailable,
                            );
                        }
                        if canonical.replace_logical_model(model).is_err() {
                            return Self::reject(
                                session,
                                log,
                                "quota-downgrade-body-invalid",
                                AiProxyError::BackendUnavailable,
                            );
                        }
                        capture_adaptive_states(
                            adaptive_store.as_ref(),
                            self.config.backend_selection.selection_type,
                            &terminal_route_uid,
                            &terminal_rule_identity,
                            &canonical.logical_model,
                            &mut candidates,
                        );
                        let admitted = quota
                            .admit(session, log, &canonical.logical_model, maximum_request_duration)
                            .await;
                        let admitted = match admitted {
                            QuotaAdmission::Ready(request) => Some(request),
                            QuotaAdmission::Bypass => {
                                quota_status = Some("bypassed");
                                None
                            }
                            QuotaAdmission::DependencyBypass => {
                                log.push_err("quota-redis-fail-open");
                                quota_status = Some("fail_open");
                                None
                            }
                            QuotaAdmission::IdentityMissing => {
                                return Self::reject(
                                    session,
                                    log,
                                    "quota-identity-missing",
                                    AiProxyError::QuotaUnavailable,
                                );
                            }
                            QuotaAdmission::Exhausted(second_retry_after) => {
                                return reject_quota_exhausted(session, log, second_retry_after);
                            }
                            QuotaAdmission::DependencyFailed => {
                                return Self::reject(session, log, "quota-unavailable", AiProxyError::QuotaUnavailable);
                            }
                        };
                        if session
                            .set_response_header("x-edgion-ai-model-downgraded", &canonical.logical_model)
                            .is_err()
                        {
                            return Self::reject(
                                session,
                                log,
                                "quota-downgrade-header-failed",
                                AiProxyError::BackendUnavailable,
                            );
                        }
                        log.push("quota-downgraded");
                        admitted
                    }
                },
                QuotaAdmission::DependencyFailed => {
                    return Self::reject(session, log, "quota-unavailable", AiProxyError::QuotaUnavailable);
                }
            }
        } else {
            None
        };

        // Quota admission can await. Never label captured bytes with a later generation.
        if session.ctx().request_body.generation() != body_generation {
            session.record_error(edgion_resources::EdgionErrorCode::BodyMutationConflict);
            return Self::reject(
                session,
                log,
                "body generation changed",
                AiProxyError::BackendUnavailable,
            );
        }
        let plan = AiAttemptPlan::Ready(ReadyAiAttemptPlan {
            request_body: canonical.body,
            api: self.config.api,
            operation,
            conversion: self.config.conversion,
            usage_observation: self.config.usage_observation,
            observation_unavailable: canonical.composite,
            body_generation,
            body_is_current: !canonical.mutated,
            request_max_output_tokens: canonical.output_bound,
            canonical_path: session.ctx().path().to_string(),
            canonical_query: session.get_query(),
            logical_model: canonical.logical_model,
            requested_streaming: canonical.requested_streaming,
            continuation: canonical.continuation,
            inject_usage_tracking: self.config.inject_usage_tracking,
            input_chars: canonical.input_chars,
            selection: self.config.backend_selection.selection_type,
            selection_sequence: if self.config.backend_selection.selection_type == AiBackendSelection::LowestLatency {
                adaptive_store.next_selection_sequence()
            } else {
                Default::default()
            },
            candidates,
            scheduler,
            retry_cap_initialized: false,
            backend_snapshot: snapshot,
            quota,
            quota_status,
            terminate_on_over_limit: self
                .quota
                .as_ref()
                .is_some_and(|quota| matches!(quota.on_exceeded(), QuotaExceededAction::Reject)),
        });
        if !session.ai().set_ai_attempt_plan(plan) {
            return Self::reject(session, log, "plan-locked", AiProxyError::BackendUnavailable);
        }
        log.push("planned");
        PluginRunningResult::GoodNext
    }

    async fn run_request_with_store(
        &self,
        store: &AiBackendStore,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let entropy = uuid::Uuid::new_v4().as_u128();
        self.run_request_with_store_and_entropy(store, entropy, session, log)
            .await
    }
}

struct CanonicalOpenAiRequest {
    body: Bytes,
    requested_model: String,
    logical_model: String,
    requested_streaming: bool,
    continuation: bool,
    api: AiApi,
    composite: bool,
    input_chars: u64,
    output_bound: Option<u64>,
    mutated: bool,
}

impl CanonicalOpenAiRequest {
    fn replace_logical_model(&mut self, logical_model: &str) -> Result<(), AiProxyError> {
        let changed = if self.api == AiApi::AnthropicMessages {
            crate::ai::protocols::anthropic::request::rewrite_model(&self.body, logical_model)?
        } else {
            request::rewrite_model(&self.body, logical_model)?
        };
        if let Some(body) = changed {
            self.body = body;
            self.mutated = true;
        }
        if self.input_chars != 0 {
            self.input_chars = checked_utf8_chars(&self.body)?;
        }
        self.logical_model = logical_model.to_string();
        Ok(())
    }
}

fn resolve_candidate_models(
    candidates: &mut [AiPlanCandidate],
    logical_model: &str,
    requested_streaming: bool,
    passthrough_allowed: bool,
) -> bool {
    let mut any_eligible = false;
    for candidate in candidates {
        candidate.adaptive = None;
        candidate.model = candidate.backend.as_ref().and_then(|backend| {
            if requested_streaming && !backend.streaming {
                return None;
            }
            backend
                .resolve_model(logical_model)
                .map(|model| AiCandidateModel {
                    provider_model: model.canonical_name.clone(),
                    pricing: model.pricing,
                })
                .or_else(|| {
                    passthrough_allowed.then(|| AiCandidateModel {
                        provider_model: logical_model.to_string(),
                        pricing: None,
                    })
                })
        });
        any_eligible |= candidate.weight > 0 && candidate.model.is_some();
    }
    any_eligible
}

fn initial_candidate_error(
    requested_streaming: bool,
    any_available_service: bool,
    any_model_supported: bool,
    any_model_eligible: bool,
) -> Option<(&'static str, AiProxyError)> {
    if any_available_service && !any_model_supported {
        return Some(("model-not-found", AiProxyError::ModelNotFound));
    }
    if requested_streaming && any_model_supported && !any_model_eligible {
        return Some(("streaming-not-supported", AiProxyError::StreamingDisabled));
    }
    None
}

fn capture_adaptive_states(
    store: &AdaptiveTelemetryStore,
    selection: AiBackendSelection,
    terminal_route_uid: &str,
    terminal_rule_identity: &str,
    logical_model: &str,
    candidates: &mut [AiPlanCandidate],
) {
    if !matches!(
        selection,
        AiBackendSelection::LowestLatency | AiBackendSelection::LowestUsage
    ) {
        return;
    }
    for candidate in candidates {
        // Unknown-model passthrough is deliberately excluded: allowing
        // client-provided names to allocate global state is unbounded.
        let model_is_catalogued = candidate
            .backend
            .as_ref()
            .is_some_and(|backend| backend.resolve_model(logical_model).is_some());
        if !model_is_catalogued || candidate.model.is_none() {
            continue;
        }
        let (namespace, name) = candidate.backend_key.split_once('/').unwrap_or(("", ""));
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: namespace.to_string(),
            name: name.to_string(),
        };
        candidate.adaptive = AdaptiveTelemetryKey::new(
            terminal_route_uid.to_string(),
            terminal_rule_identity.to_string(),
            target,
            logical_model.to_string(),
        )
        .and_then(|key| store.get(&key));
    }
}

fn reject_quota_exhausted(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    retry_after: Duration,
) -> PluginRunningResult {
    let retry_seconds = retry_after
        .as_secs()
        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
        .max(1);
    let _ = session.set_response_header("retry-after", &retry_seconds.to_string());
    AiProxy::reject(session, log, "quota-exhausted", AiProxyError::QuotaExceeded)
}

fn canonical_openai_request(body: Bytes, inject_usage_tracking: bool) -> Result<CanonicalOpenAiRequest, AiProxyError> {
    let metadata = request::metadata(&body)?;
    let changed = if inject_usage_tracking {
        request::inject_usage(&body)?
    } else {
        None
    };
    let mutated = changed.is_some();
    Ok(CanonicalOpenAiRequest {
        body: changed.unwrap_or(body),
        requested_model: metadata.model.clone(),
        logical_model: metadata.model,
        requested_streaming: metadata.streaming,
        continuation: false,
        api: AiApi::OpenaiChat,
        composite: false,
        input_chars: 0,
        output_bound: metadata.output_bound,
        mutated,
    })
}

fn canonical_messages_request(body: Bytes) -> Result<CanonicalOpenAiRequest, AiProxyError> {
    let metadata = crate::ai::protocols::anthropic::request::metadata(&body)?;
    Ok(CanonicalOpenAiRequest {
        body,
        requested_model: metadata.model.clone(),
        logical_model: metadata.model,
        requested_streaming: metadata.streaming,
        continuation: metadata.binding,
        api: AiApi::AnthropicMessages,
        composite: metadata.composite,
        input_chars: 0,
        output_bound: Some(metadata.output_bound),
        mutated: false,
    })
}

fn canonical_responses_request(body: Bytes) -> Result<CanonicalOpenAiRequest, AiProxyError> {
    let metadata = request::responses_metadata(&body)?;
    Ok(CanonicalOpenAiRequest {
        body,
        requested_model: metadata.model.clone(),
        logical_model: metadata.model,
        requested_streaming: metadata.streaming,
        continuation: metadata.continuation,
        api: AiApi::OpenaiResponses,
        composite: false,
        input_chars: 0,
        output_bound: metadata.output_bound,
        mutated: false,
    })
}

fn validate_continuation(
    config: &AiProxyConfig,
    backends: &[crate::routes::http::match_unit::CompiledBackend],
    snapshot: &crate::ai::AiBackendSnapshot,
    model: &str,
) -> Result<(), AiProxyError> {
    // Declaration counts are evaluated before any availability/weight/model filtering.
    // Model-route policy is intentionally excluded from this first continuation subset.
    if backends.len() != 1
        || !config.model_routes.is_empty()
        || config.token_quota.as_ref().is_some_and(|quota| {
            matches!(
                quota.on_exceeded,
                edgion_resources::resources::edgion_plugins::AiTokenQuotaOnExceeded::Downgrade { .. }
            )
        })
    {
        return Err(AiProxyError::UnsupportedParameter);
    }
    let (_, _, key) = ai_candidate_spec(&backends[0]).ok_or(AiProxyError::UnsupportedParameter)?;
    let backend = snapshot.get_by_key(key).ok_or(AiProxyError::BackendUnavailable)?;
    let expected_provider = if config.api == AiApi::AnthropicMessages {
        AiProvider::Anthropic
    } else {
        AiProvider::OpenAI
    };
    if backend.provider != expected_provider
        || backend.declared_credential_count != 1
        || backend
            .resolve_model(model)
            .is_some_and(|resolved| resolved.canonical_name != model)
    {
        return Err(AiProxyError::UnsupportedParameter);
    }
    Ok(())
}

fn checked_utf8_chars(body: &[u8]) -> Result<u64, AiProxyError> {
    std::str::from_utf8(body)
        .map_err(|_| AiProxyError::BackendUnavailable)?
        .chars()
        .count()
        .try_into()
        .map_err(|_| AiProxyError::BackendUnavailable)
}

#[async_trait]
impl RequestFilter for AiProxy {
    fn name(&self) -> &str {
        "AiProxy"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.run_request_with_store(global_ai_backend_store(), session, log)
            .await
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/http/proxy_test.rs"]
mod tests;
