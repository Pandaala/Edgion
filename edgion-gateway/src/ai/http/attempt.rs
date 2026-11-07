//! AI attempt policy at the HTTP transport lifecycle boundary.
//!
//! Common callbacks supply transport facts; the existing AI scheduler and
//! attempt runtime remain the sole transition and accounting owners.

use crate::ai::protocols::error::render_ai_error;
use crate::ai::runtime::attempt_runtime::{AiAttemptAdmission, AiAttemptFailure};
use crate::ai::runtime::dispatch::{
    ai_predispatch_deadline, commit_ai_attempt_context, handle_ai_predispatch_failure,
    handle_ai_predispatch_service_failure, handle_ai_response, handle_ai_transport_failure, prepare_first_attempt,
    AiDispatchError, AiPeerPredispatchFailure, AiRetryDecision, PreparedAiAttempt,
};
use crate::ai::AiProxyError;
use crate::backends::peer_builder::{create_tls_peer, extract_tls_config, record_tls_to_upstream};
use crate::backends::policy::BackendTlsPolicyLookup;
use crate::backends::runtime::CompiledBackendTarget;
use crate::backends::{compute_routed_external_resolver, next_address_rotation, upstream_dns_cache, FailureClass};
use crate::ctx::{BackendContext, BackendSource};
use crate::observe::metrics::record_retry_budget_denied;
use crate::plugins::http::common::outbound::egress::{default_block_private, filter_resolved_addrs, EgressPolicy};
use crate::routes::http::proxy_http::{backfill_upstream_addr, configure_peer_timeouts, EdgionHttpProxy};
use crate::{end_response_500, end_response_503, EdgionHttpContext, PeerTimeoutSource};
use bytes::Bytes;
use edgion_common::dns::DnsQueryPolicy;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::{BError, Error as PingoraError, Error, ErrorSource, ErrorType};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::net::{IpAddr, SocketAddr};

/// Live transport facts captured at the callback's existing retry boundary.
/// They are passed by value, never retained in the request context.
#[derive(Clone, Copy)]
pub(crate) struct RetryTransportFacts {
    pub(crate) relay_allows_retry: bool,
    pub(crate) final_response_written: bool,
}

fn ai_predispatch_error(error: AiDispatchError) -> Option<AiProxyError> {
    match error {
        AiDispatchError::Unavailable => Some(AiProxyError::BackendUnavailable),
        AiDispatchError::Client(error) => Some(error),
        AiDispatchError::Rejected
        | AiDispatchError::InvalidConfiguration
        | AiDispatchError::RequestDeadlineExceeded
        | AiDispatchError::GrpcDeadlineExceeded => None,
    }
}

fn ai_predispatch_deadline_mapping(error: &AiDispatchError) -> Option<(EdgionErrorCode, &'static str)> {
    match error {
        AiDispatchError::RequestDeadlineExceeded => Some((
            EdgionErrorCode::RequestTimeoutExceeded,
            "Request timeout exceeded during AI predispatch selection",
        )),
        AiDispatchError::GrpcDeadlineExceeded => Some((
            EdgionErrorCode::GrpcClientDeadlineExceeded,
            "gRPC client deadline exceeded during AI predispatch selection",
        )),
        _ => None,
    }
}

fn ai_predispatch_deadline_response(error: &AiDispatchError, ctx: &mut EdgionHttpContext) -> Option<BError> {
    let (status, detail) = ai_predispatch_deadline_mapping(error)?;
    ctx.record_error(status);
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.status = Some(504);
    }
    Some(PingoraError::explain(ErrorType::HTTPStatus(504), detail))
}

async fn stop_ai_predispatch_if_expired(ctx: &mut EdgionHttpContext) -> Option<BError> {
    let error = ai_predispatch_deadline(ctx)?;
    let response = ai_predispatch_deadline_response(&error, ctx)?;
    if let Some(state) = ctx.ai_state_mut() {
        state.attempt_runtime.finish_predispatch();
        state.attempt_runtime.drain_pending().await;
    }
    Some(response)
}

async fn end_ai_predispatch_error(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    error: AiProxyError,
) -> pingora_core::Result<()> {
    let (status, body) = render_ai_error(ctx, error);
    let mut response = ResponseHeader::build(status, None)?;
    edgion_http.server_header_opts.apply_to_response(&mut response);
    response.insert_header("content-type", "application/json")?;
    response.insert_header("content-length", body.len().to_string())?;
    if !ctx.claim_final_response(status) {
        return Ok(());
    }
    ctx.request_info.trace_id.reconcile_response(&mut response);
    session.write_response_header(Box::new(response), false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await
}

/// Select and admit one AI peer, retaining all predispatch settlement gates.
pub(crate) async fn select_peer(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let entropy = uuid::Uuid::new_v4().as_u128();
    let route_retry_cap = ctx.retry_attempts();
    if let Some(state) = ctx.ai_state_mut() {
        state.attempt_runtime.drain_pending().await;
    }
    loop {
        let mut target = match prepare_first_attempt(session, ctx, entropy, route_retry_cap, ctx.egc.retry_attempts) {
            Ok(target) => target,
            Err(error) => {
                if let Some(response) = ai_predispatch_deadline_response(&error, ctx) {
                    return Err(response);
                }
                if let Some(error) = ai_predispatch_error(error) {
                    end_ai_predispatch_error(edgion_http, session, ctx, error).await?;
                } else {
                    end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                }
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        };
        let admission_input = target
            .admission_input()
            .map_err(|_| PingoraError::new(ErrorType::InternalError))?;
        let admission = ctx.ensure_ai_state().attempt_runtime.admit(admission_input).await;
        match admission {
            AiAttemptAdmission::Ready(runtime) => {
                ctx.ensure_ai_state().attempt_runtime.install(runtime);
                if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                    return Err(response);
                }
                let peer = try_ai_backend_peer(edgion_http, ctx, &target).await;
                if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                    return Err(response);
                }
                match peer {
                    Ok(peer) => {
                        commit_ai_attempt_context(ctx, &mut target)
                            .map_err(|_| PingoraError::new(ErrorType::InternalError))?;
                        return Ok(peer);
                    }
                    Err(failure) => {
                        if !failure.finish(ctx) {
                            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                            return Err(PingoraError::new(ErrorType::InternalError));
                        }
                        if let Some(state) = ctx.ai_state_mut() {
                            state.attempt_runtime.drain_pending().await;
                        }
                    }
                }
            }
            AiAttemptAdmission::Skip | AiAttemptAdmission::DependencyDenied => {
                if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                    return Err(response);
                }
                if !handle_ai_predispatch_failure(ctx, true) {
                    end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                    return Err(PingoraError::new(ErrorType::InternalError));
                }
            }
            AiAttemptAdmission::SkipBackend => {
                if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                    return Err(response);
                }
                if !handle_ai_predispatch_service_failure(ctx) {
                    end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                    return Err(PingoraError::new(ErrorType::InternalError));
                }
            }
        }
    }
}

async fn try_ai_backend_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
    target: &PreparedAiAttempt,
) -> Result<Box<HttpPeer>, AiPeerPredispatchFailure> {
    let endpoint = &target.selected.backend.endpoint;
    let addr = resolve_ai_endpoint(ctx, &endpoint.domain, endpoint.port).await?;
    let compiled_target = match target.selected.logical().target() {
        CompiledBackendTarget::EdgionBackend(target) => target,
        CompiledBackendTarget::Service(_) | CompiledBackendTarget::Invalid { .. } => {
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    let (namespace, name) = (compiled_target.namespace(), compiled_target.name());
    ctx.backend_context = Some(BackendContext {
        source: Some(BackendSource::AiBackend),
        rsc: Some(crate::Rsc::new("EdgionBackend", namespace, name)),
        service_key: Some(compiled_target.key().to_owned()),
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: Some(target.selected.logical().slot()),
        effective_policy_captured: true,
        effective_policy: None,
    });
    ctx.ensure_ai_state().effective_policy = target.selected.effective_policy.clone();

    if target.selected.backend_tls_policy.is_invalid() {
        ctx.record_error(edgion_resources::EdgionErrorCode::BackendTlsPolicyInvalid);
        return Err(AiPeerPredispatchFailure::Endpoint);
    }
    if target.selected.backend_tls_policy.effective().is_some() && !endpoint.use_tls {
        return Err(AiPeerPredispatchFailure::Endpoint);
    }
    let mut peer = match &target.selected.backend_tls_policy {
        BackendTlsPolicyLookup::Effective(effective) => create_tls_peer(PingoraSocketAddr::Inet(addr), effective, None)
            .map_err(|status| {
                ctx.record_error(status);
                AiPeerPredispatchFailure::Endpoint
            })?,
        BackendTlsPolicyLookup::Absent => Box::new(HttpPeer::new(addr, endpoint.use_tls, endpoint.sni.clone())),
        BackendTlsPolicyLookup::Invalid => {
            ctx.record_error(edgion_resources::EdgionErrorCode::BackendTlsPolicyInvalid);
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    begin_ai_peer_attempt(ctx);
    let (use_tls, sni) = match &target.selected.backend_tls_policy {
        BackendTlsPolicyLookup::Effective(_) => extract_tls_config(&target.selected.backend_tls_policy),
        BackendTlsPolicyLookup::Absent => (endpoint.use_tls, endpoint.sni.as_str()),
        BackendTlsPolicyLookup::Invalid => {
            ctx.record_error(edgion_resources::EdgionErrorCode::BackendTlsPolicyInvalid);
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    record_tls_to_upstream(ctx, use_tls, sni);
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);
    ctx.ensure_ai_state().dialed_addrs.record(addr);
    Ok(peer)
}

/// Upstream addresses one AI request has dialed. Pre-connect failures consume
/// no dispatch, so the number of attempts is not bounded by
/// `MAX_AI_DISPATCH_ATTEMPTS`; recording saturates, and once an address could
/// not be recorded the record no longer says which addresses are untried, so
/// selection falls back to rotation alone.
#[derive(Debug, Default)]
pub(crate) struct DialedAddrs {
    addrs: [Option<SocketAddr>; 8],
    len: usize,
    overflowed: bool,
}

impl DialedAddrs {
    pub(crate) fn record(&mut self, addr: SocketAddr) {
        if self.contains(&addr) {
            return;
        }
        if let Some(slot) = self.addrs.get_mut(self.len) {
            *slot = Some(addr);
            self.len += 1;
        } else {
            self.overflowed = true;
        }
    }

    fn contains(&self, addr: &SocketAddr) -> bool {
        self.addrs[..self.len]
            .iter()
            .any(|dialed| dialed.as_ref() == Some(addr))
    }
}

/// Pick the address for one attempt: starting at `rotation` (spread across
/// requests, since cached answers keep their order for the record TTL), the
/// first allowed address this request has not dialed yet; once every address
/// was dialed, or the dialed record overflowed, the rotated address.
fn select_ai_addr(allowed: &[IpAddr], port: u16, dialed: &DialedAddrs, rotation: usize) -> Option<SocketAddr> {
    let len = allowed.len();
    if len == 0 {
        return None;
    }
    let start = rotation % len;
    let candidate = |offset: usize| SocketAddr::new(allowed[(start + offset) % len], port);
    if dialed.overflowed {
        return Some(candidate(0));
    }
    (0..len)
        .map(candidate)
        .find(|addr| !dialed.contains(addr))
        .or_else(|| Some(candidate(0)))
}

async fn resolve_ai_endpoint(
    ctx: &EdgionHttpContext,
    domain: &str,
    port: u16,
) -> Result<SocketAddr, AiPeerPredispatchFailure> {
    let (spec, family, policy) =
        compute_routed_external_resolver(ctx).map_err(|_| AiPeerPredispatchFailure::Endpoint)?;
    let addrs = upstream_dns_cache()
        .lookup(&spec, domain, DnsQueryPolicy::Family(family), &policy)
        .await
        .map_err(|_| AiPeerPredispatchFailure::Endpoint)?;
    let allowed = filter_resolved_addrs(
        addrs,
        EgressPolicy {
            block_private: default_block_private(),
        },
        ctx.allow_loopback_upstream,
    );
    let rotation = next_address_rotation();
    let dialed = ctx.ai_state().map(|state| &state.dialed_addrs);
    select_ai_addr(&allowed, port, dialed.unwrap_or(&DialedAddrs::default()), rotation)
        .ok_or(AiPeerPredispatchFailure::Endpoint)
}

fn begin_ai_peer_attempt(ctx: &mut EdgionHttpContext) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::lb::finalize_attempt(upstream);
    }
    ctx.try_cnt += 1;
    ctx.push_upstream(None, None);
    ctx.record_test_upstream_start();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiProxyFailureClass {
    BackendRequestTimeout,
    AmbiguousUpstream,
}

fn ai_failure_wants_retry(
    class: AiProxyFailureClass,
    retry_configured: bool,
    retry_allow_non_idempotent: bool,
) -> bool {
    match class {
        AiProxyFailureClass::BackendRequestTimeout => retry_configured,
        AiProxyFailureClass::AmbiguousUpstream => retry_allow_non_idempotent,
    }
}

#[inline]
fn ai_dispatch_should_retry<E>(decision: &Result<AiRetryDecision, E>) -> bool {
    matches!(decision, Ok(AiRetryDecision::Retry))
}

fn classify_ai_proxy_failure(
    error_type: &ErrorType,
    error_source: &ErrorSource,
    timeout_source: Option<PeerTimeoutSource>,
) -> Option<AiProxyFailureClass> {
    if !matches!(error_source, ErrorSource::Upstream) {
        return None;
    }
    match error_type {
        ErrorType::ReadTimedout | ErrorType::WriteTimedout => (timeout_source
            == Some(PeerTimeoutSource::BackendRequest))
        .then_some(AiProxyFailureClass::BackendRequestTimeout),
        ErrorType::InvalidHTTPHeader
        | ErrorType::H1Error
        | ErrorType::H2Error
        | ErrorType::H2Downgrade
        | ErrorType::InvalidH2
        | ErrorType::ReadError
        | ErrorType::WriteError
        | ErrorType::ConnectionClosed => Some(AiProxyFailureClass::AmbiguousUpstream),
        _ => None,
    }
}

/// Resolve AI policy after common transport accounting and the head retry veto.
pub(crate) fn on_transport_failure(
    ctx: &mut EdgionHttpContext,
    transport: RetryTransportFacts,
    mut e: Box<Error>,
) -> Box<Error> {
    // A status retry was decided and transitioned exactly once in the
    // response-header hook. This callback only asks Pingora to re-enter
    // upstream_peer; it must not touch scheduler or budget state again.
    if ctx.retry_on_status && transport.relay_allows_retry {
        ctx.retry_on_status = false;
        e.set_retry(true);
        return e;
    }

    let Some(failure_class) = classify_ai_proxy_failure(e.etype(), e.esource(), ctx.peer_timeout_source) else {
        return e;
    };
    let retry_allow_non_idempotent = ctx.retry_allow_non_idempotent();
    let common_retry_gates = transport.relay_allows_retry
        && !ctx.request_timeout_exceeded()
        && !ctx.grpc_deadline_exceeded()
        && !ctx.response_write_started()
        && !transport.final_response_written;
    let wants_retry = ai_failure_wants_retry(failure_class, ctx.retry_configured(), retry_allow_non_idempotent);
    let gates_allow_retry = common_retry_gates
        && (failure_class == AiProxyFailureClass::BackendRequestTimeout || retry_allow_non_idempotent);
    let decision = handle_ai_transport_failure(
        ctx,
        wants_retry,
        gates_allow_retry,
        failure_class == AiProxyFailureClass::AmbiguousUpstream,
    );
    e.set_retry(ai_dispatch_should_retry(&decision));
    if let Some(state) = ctx.ai_state_mut() {
        state.attempt_runtime.finish_failure(match failure_class {
            AiProxyFailureClass::BackendRequestTimeout => AiAttemptFailure::BackendRequestTimeout,
            AiProxyFailureClass::AmbiguousUpstream => AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin),
        });
    }
    match decision {
        Ok(AiRetryDecision::Retry) => {
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                match failure_class {
                    AiProxyFailureClass::BackendRequestTimeout => {
                        upstream.status = Some(504);
                        upstream.err.push("AiBackendRequestTimeoutRetry".into());
                    }
                    AiProxyFailureClass::AmbiguousUpstream => {
                        upstream.err.push("AiAmbiguousUpstreamRetry".into());
                    }
                }
            }
            if failure_class == AiProxyFailureClass::BackendRequestTimeout {
                ctx.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);
            }
        }
        Ok(AiRetryDecision::BudgetDenied) => {
            record_retry_budget_denied();
            ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
            return Error::explain(ErrorType::HTTPStatus(503), "AI retry budget exhausted");
        }
        Ok(AiRetryDecision::Final) | Err(_) => {}
    }
    e
}

/// Release an AI predispatch admission; advance the service only when replay is allowed.
pub(crate) fn on_connect_failure(ctx: &mut EdgionHttpContext, relay_allows_retry: bool, error: &mut Error) {
    if !relay_allows_retry {
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.finish_predispatch();
        }
    } else if AiPeerPredispatchFailure::Endpoint.finish(ctx) {
        error.set_retry(true);
    }
}

/// Observe the raw provider response and advance an AI status retry exactly once.
pub(crate) fn on_raw_response(
    ctx: &mut EdgionHttpContext,
    upstream_response: &mut ResponseHeader,
    transport: RetryTransportFacts,
) -> pingora_core::Result<()> {
    let status_code = upstream_response.status.as_u16();
    let wants_retry = status_code != 101 && ctx.retry_codes().is_some_and(|codes| codes.contains(&status_code));
    if status_code == 429 {
        crate::ai::http::response::apply_provider_cooldown(ctx, upstream_response);
    }
    let failure_class = FailureClass::from_http_status(status_code)
        .or_else(|| (status_code == 429 || wants_retry).then_some(FailureClass::Application));
    if let Some(active) = ctx.ai_state_mut().and_then(|state| state.attempt_runtime.active_mut()) {
        active.record_response(status_code, failure_class);
    }
    let gates_allow_retry = !ctx.request_timeout_exceeded()
        && !ctx.grpc_deadline_exceeded()
        && !ctx.response_write_started()
        && !transport.final_response_written
        && transport.relay_allows_retry;
    match handle_ai_response(ctx, wants_retry, gates_allow_retry)
        .map_err(|_| pingora_core::Error::new(pingora_core::ErrorType::InternalError))?
    {
        AiRetryDecision::Retry => {
            if let Some(state) = ctx.ai_state_mut() {
                state.attempt_runtime.finish_failure(AiAttemptFailure::Known(
                    failure_class.unwrap_or(FailureClass::Application),
                ));
            }
            ctx.retry_on_status = true;
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.err.push("AiRetryOnStatus".into());
            }
            return Err(pingora_core::Error::explain(
                pingora_core::ErrorType::HTTPStatus(status_code),
                "AI retry on upstream status",
            ));
        }
        AiRetryDecision::BudgetDenied => {
            if let Some(state) = ctx.ai_state_mut() {
                state.attempt_runtime.finish_failure(AiAttemptFailure::Known(
                    failure_class.unwrap_or(FailureClass::Application),
                ));
            }
            record_retry_budget_denied();
            ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
            return Err(pingora_core::Error::explain(
                pingora_core::ErrorType::HTTPStatus(503),
                "AI retry budget exhausted",
            ));
        }
        AiRetryDecision::Final => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/http/attempt_test.rs"]
pub(crate) mod tests;
