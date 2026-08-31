use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_backend::{AiProvider, AI_MODEL_NAME_MAX_BYTES};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{
    AiBackendSelection, AiProxyConfig, OnUnknownModel, PluginValidation,
};
use pingora_http::ResponseHeader;
use serde_json::{Map, Value};

use crate::ai::attempt::{AttemptScheduler, BackendCredentials, MAX_AI_ROUTE_RETRIES};
use crate::ai::model_router::ModelRouteTable;
use crate::ai::quota::{CompiledAiQuotaPlan, QuotaAdmission, QuotaExceededAction};
use crate::ai::routing::{invalid_weight_gate, CandidateEligibility, ConformanceGate, RouteCandidate};
use crate::ai::{
    get_adaptive_telemetry_store, global_ai_backend_store, openai_error_response, AdaptiveTelemetryKey,
    AdaptiveTelemetryStore, AiAttemptPlan, AiBackendStore, AiCandidateModel, AiPlanCandidate, AiPlanError,
    AiProxyError, ReadyAiAttemptPlan,
};
use crate::backends::policy::backend_tls::get_global_backend_tls_policy_store;
use crate::backends::policy::backend_traffic::get_backend_traffic_policy_store;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::match_unit::CompiledBackendTarget;

const DEFAULT_MAXIMUM_REQUEST_DURATION: Duration = Duration::from_secs(300);

fn ai_candidate_spec(backend: &crate::routes::http::match_unit::CompiledBackend) -> Option<(usize, i32, String)> {
    if !backend.is_permitted_ai_backend() {
        return None;
    }
    match &backend.target {
        CompiledBackendTarget::EdgionBackend { namespace, name } => Some((
            backend.slot,
            backend.schema.weight.unwrap_or(1),
            format!("{namespace}/{name}"),
        )),
        CompiledBackendTarget::Service { .. } | CompiledBackendTarget::Invalid { .. } => None,
    }
}

fn gate_candidate(
    order: usize,
    backend: &crate::routes::http::match_unit::CompiledBackend,
    runtime_available: bool,
) -> Result<RouteCandidate, ()> {
    Ok(RouteCandidate {
        slot: u8::try_from(backend.slot).map_err(|_| ())?,
        order: u8::try_from(order).map_err(|_| ())?,
        weight: backend.schema.weight.unwrap_or(1),
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
    headers: &[(String, String)],
    lookup: impl Fn(&str) -> Option<std::sync::Arc<crate::ai::RuntimeAiBackend>>,
) -> Vec<String> {
    let mut proposed = BTreeSet::new();
    for backend in backends.iter().filter(|backend| backend.is_permitted_ai_backend()) {
        let Some((_, _, key)) = ai_candidate_spec(backend) else {
            continue;
        };
        if let Some(runtime) = lookup(&key) {
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
                let weight = decision.weights.get(backend.slot).copied().unwrap_or(0);
                if weight <= 0 || !backend.is_permitted_ai_backend() {
                    return false;
                }
                let Some((_, _, key)) = ai_candidate_spec(backend) else {
                    return false;
                };
                lookup(&key).is_some_and(|runtime| {
                    let resolved = runtime.resolve_model(&decision.identity_model);
                    let has_symbolic_owner = !model_id.ends_with('*')
                        || decision.rewrite_requires_canonical
                        || runtime
                            .wildcard_models
                            .iter()
                            .any(|alias| alias.pattern == *model_id && alias.model.public);
                    resolved.is_some_and(|model| model.public)
                        && has_symbolic_owner
                        && (!decision.rewrite_requires_canonical
                            || runtime.has_canonical_model(&decision.identity_model))
                })
            })
        })
        .collect()
}

fn unknown_model_passthrough_allowed(
    rewrite_requires_canonical: bool,
    configured: OnUnknownModel,
    candidates: &[(
        (usize, i32, String),
        Option<std::sync::Arc<crate::ai::RuntimeAiBackend>>,
    )],
) -> bool {
    !rewrite_requires_canonical
        && configured == OnUnknownModel::Passthrough
        && candidates
            .iter()
            .filter(|((_, weight, _), _)| *weight > 0)
            .all(|(_, backend)| {
                backend.as_ref().is_some_and(|backend| {
                    matches!(backend.provider, AiProvider::OpenAI | AiProvider::OpenAICompatible)
                })
            })
}

fn has_positive_weight_service_backend(backends: &[crate::routes::http::match_unit::CompiledBackend]) -> bool {
    backends.iter().any(|backend| {
        backend.schema.weight.unwrap_or(1) > 0 && matches!(backend.target, CompiledBackendTarget::Service { .. })
    })
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
        openai_error_response(session, error)
    }

    fn install_rejected_plan(
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        error: AiPlanError,
    ) -> PluginRunningResult {
        if session.set_ai_attempt_plan(AiAttemptPlan::Rejected(error)) {
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
        let headers = session.request_headers();
        let visible = visible_model_ids(&self.model_routes, backends, &headers, |key| snapshot.get_by_key(key));
        let data = visible
            .into_iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "object": "model",
                    "created": 0,
                    "owned_by": "edgion"
                })
            })
            .collect::<Vec<_>>();
        let body = match serde_json::to_vec(&serde_json::json!({"object": "list", "data": data})) {
            Ok(body) => Bytes::from(body),
            Err(_) => return Self::reject(session, log, "model-list-encode", AiProxyError::BackendUnavailable),
        };
        let mut response = match ResponseHeader::build(200, None) {
            Ok(response) => response,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
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

        if session.get_method() == "GET" && session.get_path().ends_with("/models") {
            return self.write_model_list(&snapshot, &backends, session, log).await;
        }

        if session.get_method() != "POST" || !session.get_path().ends_with("/chat/completions") {
            return Self::reject(session, log, "endpoint-unsupported", AiProxyError::UnsupportedEndpoint);
        }
        if session.declares_body() == Some(false) {
            return Self::reject(session, log, "body-required", AiProxyError::MalformedBody);
        }
        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(crate::plugins::runtime::traits::session::BodyReadError::BufferLimitExceeded { .. }) => {
                return Self::reject(session, log, "body-too-large", AiProxyError::RequestTooLarge);
            }
            Err(_) => return Self::reject(session, log, "body-unavailable", AiProxyError::BackendUnavailable),
        };
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

        let mut canonical = match canonical_openai_request(body, self.config.inject_usage_tracking) {
            Ok(canonical) => canonical,
            Err(error) => return Self::reject(session, log, "request-invalid", error),
        };

        let decision = match self
            .model_routes
            .decide(&canonical.requested_model, &session.request_headers(), &backends)
        {
            Ok(decision) => decision,
            Err(_) => return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration),
        };
        if decision.identity_model != canonical.logical_model
            && canonical.replace_logical_model(&decision.identity_model).is_err()
        {
            return Self::reject(session, log, "model-rewrite-invalid", AiProxyError::BackendUnavailable);
        }

        let mut gate_candidates = Vec::with_capacity(backends.len());
        for (order, backend) in backends.iter().enumerate() {
            let runtime_available = ai_candidate_spec(backend)
                .and_then(|(_, _, key)| snapshot.get_by_key(&key))
                .is_some();
            let Ok(mut candidate) = gate_candidate(order, backend, runtime_available) else {
                return Self::install_rejected_plan(session, log, AiPlanError::InvalidRouteConfiguration);
            };
            // A missing runtime object remains an invalid declared share even
            // when an explicit filter omits this syntactically valid slot.
            // Otherwise the filter would silently normalize onto healthy
            // candidates during publication skew.
            if runtime_available {
                candidate.weight = decision.weights.get(backend.slot).copied().unwrap_or(candidate.weight);
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

        let policy_store = get_backend_traffic_policy_store();
        let policy_snapshot = policy_store.snapshot();
        let backend_tls_snapshot = get_global_backend_tls_policy_store().snapshot();
        let adaptive_store = get_adaptive_telemetry_store();
        let candidate_specs: Vec<_> = backends
            .iter()
            .enumerate()
            .filter_map(|(index, backend)| {
                let (slot, _, key) = ai_candidate_spec(backend)?;
                let logical = crate::backends::LogicalBackendHandle::new(Arc::clone(&backends), index)?;
                Some((logical, slot, decision.weights.get(slot).copied().unwrap_or(0), key))
            })
            .collect();
        let resolved = snapshot.get_many_by_key(candidate_specs.iter().map(|(_, _, _, key)| key.as_str()));
        let passthrough_inputs = candidate_specs
            .iter()
            .map(|(_, slot, weight, key)| (*slot, *weight, key.clone()))
            .zip(resolved.iter().cloned())
            .collect::<Vec<_>>();
        let passthrough_allowed = unknown_model_passthrough_allowed(
            decision.rewrite_requires_canonical,
            self.config.on_unknown_model,
            &passthrough_inputs,
        );
        let mut candidates = candidate_specs
            .into_iter()
            .zip(resolved)
            .enumerate()
            .map(|(order, ((logical, slot, weight, backend_key), backend))| {
                let target = logical.policy_target_key();
                let effective_policy = target.as_ref().and_then(|target| policy_snapshot.get_effective(target));
                let backend_tls_policy = target
                    .as_ref()
                    .and_then(|target| backend_tls_snapshot.get_policy(target, None));
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
                AiPlanCandidate {
                    logical,
                    slot,
                    order: u8::try_from(order).expect("AI route candidates are bounded before body read"),
                    weight,
                    backend_key,
                    backend,
                    model: None,
                    effective_policy,
                    backend_tls_policy,
                    adaptive: None,
                    credential_outliers,
                }
            })
            .collect::<Vec<_>>();
        if decision.rewrite_requires_canonical
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
        let route_retries = session.ctx().retry_attempts().unwrap_or(0);
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
                    session.record_outcome(DyeOutcome::FailOpen, "quota-redis-unreachable");
                    log.push_err("quota-redis-fail-open");
                    quota_status = Some("fail_open");
                    None
                }
                QuotaAdmission::IdentityMissing => {
                    session.record_outcome(DyeOutcome::FailClose, "quota-identity-missing");
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
                                session.record_outcome(DyeOutcome::FailOpen, "quota-redis-unreachable");
                                log.push_err("quota-redis-fail-open");
                                quota_status = Some("fail_open");
                                None
                            }
                            QuotaAdmission::IdentityMissing => {
                                session.record_outcome(DyeOutcome::FailClose, "quota-identity-missing");
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
                                session.record_outcome(DyeOutcome::FailClose, "quota-redis-unreachable");
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
                    session.record_outcome(DyeOutcome::FailClose, "quota-redis-unreachable");
                    return Self::reject(session, log, "quota-unavailable", AiProxyError::QuotaUnavailable);
                }
            }
        } else {
            None
        };

        let plan = AiAttemptPlan::Ready(ReadyAiAttemptPlan {
            canonical_openai_body: canonical.body,
            canonical_path: session.ctx().path().to_string(),
            canonical_query: session.get_query(),
            logical_model: canonical.logical_model,
            requested_streaming: canonical.requested_streaming,
            inject_usage_tracking: self.config.inject_usage_tracking,
            input_chars: canonical.input_chars,
            selection: self.config.backend_selection.selection_type,
            selection_sequence: (self.config.backend_selection.selection_type == AiBackendSelection::LowestLatency)
                .then(|| adaptive_store.next_selection_sequence())
                .unwrap_or_default(),
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
        if !session.set_ai_attempt_plan(plan) {
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
        let entropy = session
            .ctx()
            .request_info
            .x_trace_id
            .as_deref()
            .and_then(|trace_id| uuid::Uuid::parse_str(trace_id).ok())
            .map(|trace_id| trace_id.as_u128())
            .unwrap_or_else(|| uuid::Uuid::new_v4().as_u128());
        self.run_request_with_store_and_entropy(store, entropy, session, log)
            .await
    }
}

struct CanonicalOpenAiRequest {
    body: Bytes,
    requested_model: String,
    logical_model: String,
    requested_streaming: bool,
    input_chars: u64,
}

impl CanonicalOpenAiRequest {
    fn replace_logical_model(&mut self, logical_model: &str) -> Result<(), AiProxyError> {
        let mut value: Value = serde_json::from_slice(&self.body).map_err(|_| AiProxyError::BackendUnavailable)?;
        let object = value.as_object_mut().ok_or(AiProxyError::BackendUnavailable)?;
        object.insert("model".to_string(), Value::String(logical_model.to_string()));
        let body = Bytes::from(serde_json::to_vec(&value).map_err(|_| AiProxyError::BackendUnavailable)?);
        let input_chars = checked_utf8_chars(&body)?;
        self.body = body;
        self.logical_model = logical_model.to_string();
        self.input_chars = input_chars;
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
    session.record_outcome(DyeOutcome::Reject, "quota-exhausted");
    let retry_seconds = retry_after
        .as_secs()
        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
        .max(1);
    let _ = session.set_response_header("retry-after", &retry_seconds.to_string());
    AiProxy::reject(session, log, "quota-exhausted", AiProxyError::QuotaExceeded)
}

fn canonical_openai_request(body: Bytes, inject_usage_tracking: bool) -> Result<CanonicalOpenAiRequest, AiProxyError> {
    let mut value: Value = serde_json::from_slice(&body).map_err(|_| AiProxyError::MalformedBody)?;
    let object = value.as_object_mut().ok_or(AiProxyError::MalformedBody)?;
    let logical_model = match object.get("model") {
        None => return Err(AiProxyError::ModelNotFound),
        Some(Value::String(model)) if model.is_empty() => return Err(AiProxyError::ModelNotFound),
        Some(Value::String(model)) if model.len() <= AI_MODEL_NAME_MAX_BYTES => model.clone(),
        Some(Value::String(_)) => return Err(AiProxyError::ModelNotFound),
        Some(_) => return Err(AiProxyError::MalformedBody),
    };
    let requested_streaming = object.get("stream").and_then(Value::as_bool) == Some(true);
    let body = if requested_streaming && inject_usage_tracking {
        let stream_options = object
            .entry("stream_options".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(stream_options) = stream_options.as_object_mut() else {
            return Err(AiProxyError::MalformedBody);
        };
        stream_options.insert("include_usage".to_string(), Value::Bool(true));
        Bytes::from(serde_json::to_vec(&value).map_err(|_| AiProxyError::BackendUnavailable)?)
    } else {
        body
    };
    let input_chars = checked_utf8_chars(&body)?;
    Ok(CanonicalOpenAiRequest {
        body,
        requested_model: logical_model.clone(),
        logical_model,
        requested_streaming,
        input_chars,
    })
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
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use crate::ai::backend_store::RuntimeWildcardModel;
    use crate::ai::{ResolvedModel, RuntimeAiBackend, RuntimeAiCredentialPool, RuntimeAiEndpoint};
    use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;
    use crate::plugins::PluginRuntime;
    use crate::routes::http::match_unit::CompiledBackend;
    use edgion_resources::resources::common::RefDenied;
    use edgion_resources::resources::edgion_backend::AiProvider;
    use edgion_resources::resources::edgion_plugins::{
        AiBackendSelection, AiBackendSelectionConfig, AiModelRoute, AiModelRouteAction, AiModelRouteMatch,
    };
    use edgion_resources::HTTPBackendRef;

    fn config() -> AiProxyConfig {
        AiProxyConfig {
            backend_selection: AiBackendSelectionConfig {
                selection_type: AiBackendSelection::Weighted,
            },
            inject_usage_tracking: true,
            on_unknown_model: OnUnknownModel::Reject,
            model_routes: Vec::new(),
            token_quota: None,
        }
    }

    fn compiled_ai_backend(ref_denied: Option<RefDenied>) -> CompiledBackend {
        CompiledBackend::compile(
            HTTPBackendRef {
                name: "provider".to_string(),
                namespace: Some("tenant-b".to_string()),
                port: None,
                weight: Some(1),
                group: Some("edgion.io".to_string()),
                kind: Some("EdgionBackend".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied,
            },
            "tenant-a",
            0,
            Arc::new(PluginRuntime::new()),
        )
    }

    fn compiled_service_backend(weight: i32) -> CompiledBackend {
        CompiledBackend::compile(
            HTTPBackendRef {
                name: "service".to_string(),
                namespace: None,
                port: Some(8080),
                weight: Some(weight),
                group: None,
                kind: Some("Service".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "tenant-a",
            1,
            Arc::new(PluginRuntime::new()),
        )
    }

    #[test]
    fn zero_weight_service_slot_is_protocol_neutral_at_request_gate() {
        let ai = compiled_ai_backend(None);
        assert!(!has_positive_weight_service_backend(&[
            ai.clone(),
            compiled_service_backend(0),
        ]));
        assert!(has_positive_weight_service_backend(&[ai, compiled_service_backend(1),]));
    }

    #[test]
    fn denied_ai_backend_is_invalid_and_never_materialized() {
        let denied = compiled_ai_backend(Some(RefDenied {
            target_namespace: "tenant-b".to_string(),
            target_name: "provider".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }));
        let allowed = compiled_ai_backend(None);

        assert!(!denied.is_permitted_ai_backend());
        assert!(allowed.is_permitted_ai_backend());
        let gate_candidates = [gate_candidate(0, &denied, false).unwrap()];
        assert_eq!(invalid_weight_gate(&gate_candidates, 0), Ok(ConformanceGate::Invalid));
        assert!(ai_candidate_spec(&denied).is_none());
        let (slot, weight, key) = ai_candidate_spec(&allowed).unwrap();
        assert_eq!((slot, weight, key), (0, 1, "tenant-b/provider".to_string()));
        let generation = Arc::new(vec![allowed.clone()]);
        let logical = crate::backends::LogicalBackendHandle::new(generation, 0).unwrap();
        assert_eq!(logical.identity(), &allowed.identity);
    }

    #[test]
    fn missing_runtime_backend_keeps_its_invalid_weight_share() {
        let declared = compiled_ai_backend(None);
        let gate_candidates = [gate_candidate(0, &declared, false).unwrap()];
        assert_eq!(invalid_weight_gate(&gate_candidates, 0), Ok(ConformanceGate::Invalid));
    }

    #[test]
    fn constructor_contains_no_backend_membership() {
        let plugin = AiProxy::from_config(&config(), "tenant-a/plugins/#0").expect("valid plugin");
        assert_eq!(
            plugin.config.backend_selection.selection_type,
            AiBackendSelection::Weighted
        );
        assert!(plugin.quota.is_none());
    }

    #[test]
    fn model_list_is_deterministic_public_and_requires_symbolic_wildcard_owner() {
        let public = ResolvedModel {
            canonical_name: "provider-public".to_string(),
            pricing: None,
            public: true,
        };
        let hidden = ResolvedModel {
            canonical_name: "provider-hidden".to_string(),
            pricing: None,
            public: false,
        };
        let runtime = Arc::new(RuntimeAiBackend {
            uid: "backend-uid".to_string(),
            provider: AiProvider::OpenAI,
            endpoint: RuntimeAiEndpoint {
                domain: "example.test".to_string(),
                port: 443,
                use_tls: true,
                sni: "example.test".to_string(),
                host: "example.test".to_string(),
                path_prefix: String::new(),
            },
            streaming: true,
            max_output_tokens: None,
            credential_pool: RuntimeAiCredentialPool {
                redis_ref: None,
                on_redis_failure: Default::default(),
                credentials: Vec::new(),
                limiter_core: Arc::new(RedisRateLimiter::new()),
            },
            models: HashMap::from([("public".to_string(), public.clone()), ("hidden".to_string(), hidden)]),
            wildcard_models: vec![
                RuntimeWildcardModel {
                    pattern: "owned*".to_string(),
                    prefix: "owned".to_string(),
                    model: public.clone(),
                },
                RuntimeWildcardModel {
                    pattern: "n*".to_string(),
                    prefix: "n".to_string(),
                    model: public,
                },
            ],
            canonical_models: HashSet::from(["public".to_string(), "hidden".to_string()]),
            public_model_ids: BTreeSet::from(["public".to_string(), "owned*".to_string(), "n*".to_string()]),
        });
        let routes = ModelRouteTable::compile(&[
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "virtual*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: Some("hidden".to_string()),
                    candidates: None,
                },
            },
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "owned*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: None,
                    candidates: None,
                },
            },
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "narrow*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: None,
                    candidates: None,
                },
            },
        ])
        .unwrap();
        let visible = visible_model_ids(&routes, &[compiled_ai_backend(None)], &[], |key| {
            (key == "tenant-b/provider").then(|| runtime.clone())
        });
        assert_eq!(visible, vec!["owned*", "public"]);
        assert!(!visible.iter().any(|id| id == "virtual*" || id == "narrow*"));
    }

    #[test]
    fn canonical_request_preserves_logical_model_and_injects_usage_once() {
        let input = Bytes::from_static(
            br#"{"model":"fast","stream":true,"stream_options":{"custom":7,"include_usage":false}}"#,
        );
        let witness = input.clone();
        let request = canonical_openai_request(input, true).unwrap();
        assert_eq!(request.logical_model, "fast");
        assert!(request.requested_streaming);
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "fast");
        assert_eq!(body["stream_options"]["custom"], 7);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(
            request.input_chars,
            u64::try_from(std::str::from_utf8(&request.body).unwrap().chars().count()).unwrap()
        );
        assert_ne!(request.body.as_ptr(), witness.as_ptr());
    }

    #[test]
    fn canonical_request_reuses_nonempty_sliced_bytes_when_wire_body_is_unchanged() {
        let padded = Bytes::from_static(br#"xx{"model":"fast","stream":false}yy"#);
        let input = padded.slice(2..padded.len() - 2);
        let witness = input.clone();
        let expected_ptr = witness.as_ptr();
        let expected_len = witness.len();

        let request = canonical_openai_request(input, true).unwrap();

        assert_eq!(request.body, witness);
        assert_eq!(request.body.len(), expected_len);
        assert_eq!(request.body.as_ptr(), expected_ptr);
    }

    #[test]
    fn canonical_request_rejects_malformed_or_unbounded_model() {
        assert!(matches!(
            canonical_openai_request(Bytes::from_static(br#"{"model":7}"#), true),
            Err(AiProxyError::MalformedBody)
        ));
        let body = format!(r#"{{"model":"{}"}}"#, "x".repeat(AI_MODEL_NAME_MAX_BYTES + 1));
        assert!(matches!(
            canonical_openai_request(Bytes::from(body.into_bytes()), true),
            Err(AiProxyError::ModelNotFound)
        ));
    }

    #[test]
    fn canonical_request_invalid_utf8_remains_malformed_body() {
        assert!(matches!(
            canonical_openai_request(Bytes::from_static(b"{\"model\":\"fast\",\"content\":\"\xff\"}"), true),
            Err(AiProxyError::MalformedBody)
        ));
    }

    #[test]
    fn canonical_request_replaces_only_the_logical_model_for_downgrade() {
        let mut request = canonical_openai_request(
            Bytes::from_static(br#"{"model":"premium","stream":true,"messages":[{"role":"user","content":"hello"}]}"#),
            true,
        )
        .unwrap();

        let premium_chars = request.input_chars;
        request.replace_logical_model("e").unwrap();
        let short_chars = request.input_chars;
        assert!(short_chars < premium_chars);

        request.replace_logical_model("economy-longer-than-premium").unwrap();

        assert_eq!(request.logical_model, "economy-longer-than-premium");
        assert!(request.input_chars > short_chars);
        assert_eq!(
            request.input_chars,
            u64::try_from(std::str::from_utf8(&request.body).unwrap().chars().count()).unwrap()
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "economy-longer-than-premium");
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn canonical_input_chars_tracks_usage_injection_and_unicode_characters() {
        let original =
            Bytes::copy_from_slice(r#"{"model":"fast","stream":true,"messages":[{"content":"é€"}]}"#.as_bytes());
        let without_injection = canonical_openai_request(original.clone(), false).unwrap();
        let with_injection = canonical_openai_request(original, true).unwrap();

        assert_eq!(
            without_injection.input_chars,
            u64::try_from(std::str::from_utf8(&without_injection.body).unwrap().chars().count()).unwrap()
        );
        assert_eq!(
            with_injection.input_chars,
            u64::try_from(std::str::from_utf8(&with_injection.body).unwrap().chars().count()).unwrap()
        );
        assert!(with_injection.input_chars > without_injection.input_chars);
    }

    #[test]
    fn initial_candidate_errors_preserve_model_streaming_and_availability_semantics() {
        assert_eq!(
            initial_candidate_error(false, true, false, false),
            Some(("model-not-found", AiProxyError::ModelNotFound))
        );
        assert_eq!(
            initial_candidate_error(true, true, true, false),
            Some(("streaming-not-supported", AiProxyError::StreamingDisabled))
        );
        assert_eq!(initial_candidate_error(true, false, false, false), None);
        assert_eq!(initial_candidate_error(true, true, true, true), None);
    }

    #[test]
    fn downgrade_re_resolves_models_from_captured_candidates_without_passthrough() {
        let backend = Arc::new(RuntimeAiBackend {
            uid: "backend-uid".to_string(),
            provider: AiProvider::OpenAI,
            endpoint: RuntimeAiEndpoint {
                domain: "example.test".to_string(),
                port: 443,
                use_tls: true,
                sni: "example.test".to_string(),
                host: "example.test".to_string(),
                path_prefix: String::new(),
            },
            streaming: true,
            max_output_tokens: None,
            credential_pool: RuntimeAiCredentialPool {
                redis_ref: None,
                on_redis_failure: Default::default(),
                credentials: Vec::new(),
                limiter_core: Arc::new(RedisRateLimiter::new()),
            },
            models: HashMap::from([
                (
                    "premium".to_string(),
                    ResolvedModel {
                        canonical_name: "provider-premium".to_string(),
                        pricing: None,
                        public: true,
                    },
                ),
                (
                    "economy".to_string(),
                    ResolvedModel {
                        canonical_name: "provider-economy".to_string(),
                        pricing: None,
                        public: true,
                    },
                ),
            ]),
            wildcard_models: Vec::new(),
            canonical_models: HashSet::from(["premium".to_string(), "economy".to_string()]),
            public_model_ids: BTreeSet::from(["premium".to_string(), "economy".to_string()]),
        });
        let compiled = compiled_ai_backend(None);
        let logical = crate::backends::LogicalBackendHandle::new(Arc::new(vec![compiled]), 0).unwrap();
        let mut candidates = vec![AiPlanCandidate {
            logical,
            slot: 0,
            order: 0,
            weight: 1,
            backend_key: "tenant/backend".to_string(),
            backend: Some(backend),
            model: None,
            effective_policy: None,
            backend_tls_policy: None,
            adaptive: None,
            credential_outliers: Vec::new(),
        }];

        assert!(resolve_candidate_models(&mut candidates, "economy", true, false));
        assert_eq!(
            candidates[0].model.as_ref().map(|model| model.provider_model.as_str()),
            Some("provider-economy")
        );
        assert!(!resolve_candidate_models(&mut candidates, "unknown", true, false));
        assert!(candidates[0].model.is_none());

        // Simulate the passthrough result without adding the client-provided
        // name to the bounded backend catalog.
        candidates[0].model = Some(AiCandidateModel {
            provider_model: "client-provided".to_string(),
            pricing: None,
        });
        let adaptive = AdaptiveTelemetryStore::new();
        capture_adaptive_states(
            &adaptive,
            AiBackendSelection::Weighted,
            "route-uid",
            "rule-a",
            "client-provided",
            &mut candidates,
        );
        assert!(adaptive.is_empty());
        capture_adaptive_states(
            &adaptive,
            AiBackendSelection::LowestLatency,
            "route-uid",
            "rule-a",
            "client-provided",
            &mut candidates,
        );
        assert!(adaptive.is_empty());

        assert!(resolve_candidate_models(&mut candidates, "economy", true, false));
        adaptive.reconcile_keys(&std::collections::HashSet::from([AdaptiveTelemetryKey::new(
            "route-uid".to_string(),
            "rule-a".to_string(),
            PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionBackend,
                namespace: "tenant".to_string(),
                name: "backend".to_string(),
            },
            "economy".to_string(),
        )
        .unwrap()]));
        capture_adaptive_states(
            &adaptive,
            AiBackendSelection::LowestLatency,
            "route-uid",
            "rule-a",
            "economy",
            &mut candidates,
        );
        assert_eq!(adaptive.len(), 1);
        assert!(candidates[0].adaptive.is_some());

        let passthrough_inputs = vec![((0, 1, "tenant/backend".to_string()), candidates[0].backend.clone())];
        assert!(unknown_model_passthrough_allowed(
            false,
            OnUnknownModel::Passthrough,
            &passthrough_inputs
        ));
        assert!(!unknown_model_passthrough_allowed(
            true,
            OnUnknownModel::Passthrough,
            &passthrough_inputs
        ));
        assert!(!unknown_model_passthrough_allowed(
            false,
            OnUnknownModel::Reject,
            &passthrough_inputs
        ));
        assert!(!unknown_model_passthrough_allowed(
            false,
            OnUnknownModel::Passthrough,
            &[((0, 1, "missing/backend".to_string()), None)]
        ));
    }

    #[test]
    fn plan_debug_never_renders_canonical_body() {
        let plan = ReadyAiAttemptPlan {
            canonical_openai_body: Bytes::from_static(br#"{"prompt":"must-not-render"}"#),
            canonical_path: "/v1/chat/completions".to_string(),
            canonical_query: None,
            logical_model: "model-a".to_string(),
            requested_streaming: false,
            inject_usage_tracking: true,
            input_chars: 28,
            selection: AiBackendSelection::Weighted,
            selection_sequence: 0,
            candidates: Vec::new(),
            scheduler: AttemptScheduler::new(&[], 0).unwrap(),
            retry_cap_initialized: true,
            backend_snapshot: AiBackendStore::new().snapshot(),
            quota: None,
            quota_status: None,
            terminate_on_over_limit: false,
        };
        let rendered = format!("{plan:?}");
        assert!(!rendered.contains("must-not-render"));
        assert!(rendered.contains("canonical_openai_body_bytes"));
    }
}
