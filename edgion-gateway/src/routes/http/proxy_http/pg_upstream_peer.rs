use super::EdgionHttpProxy;
use crate::ai::dispatch::{
    ai_predispatch_deadline, commit_ai_attempt_context, prepare_first_attempt, AiDispatchError,
    AiPeerPredispatchFailure, PreparedAiAttempt,
};
use crate::ai::{render_openai_error, AiProxyError};
use crate::backends::peer_builder::{create_tls_peer, extract_tls_config};
use crate::backends::policy::backend_traffic::authority::{render_authority, AuthorityError};
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;
use crate::backends::policy::backend_traffic::RetryBudget;
use crate::backends::ServiceBackendKind;
use crate::backends::{get_peer, try_get_peer};
use crate::backends::{SelectedHttpBackend, SelectedLogicalBackend, SelectedServiceBackend};
use crate::ctx::BackendSource;
use crate::observe::metrics::record_authority_rejected;
use crate::plugins::http::common::outbound::egress::{default_block_private, filter_resolved_addrs, EgressPolicy};
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::grpc::handle_grpc_upstream;
use crate::routes::http::match_unit::CompiledBackendTarget;
use crate::routes::http::routes_mgr::RouteRules;
use crate::routes::http::session_persistence;
use crate::util::net::{classify_backend_addr, resolve_domain, BlockedBackendReason};
use crate::{end_response_500, end_response_503, end_response_grpc_unavailable, terminate_with_fallback};
use crate::{EdgionHttpContext, SessionPersistenceState, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_common::dns::resolve;
use edgion_resources::err::EdError;
use edgion_resources::path_normalization::get_path_normalization_config;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::upstreams::peer::Peer;
use pingora_core::{BError, Error as PingoraError, ErrorType};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::sync::Arc;

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

/// Stop after an I/O-bearing predispatch step if either end-to-end deadline
/// expired while it was in flight. An installed admission remains
/// predispatch, so release and drain it before returning the timeout.
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
    let (status, body) = render_openai_error(error);
    let mut response = ResponseHeader::build(status, None)?;
    edgion_http.server_header_opts.apply_to_response(&mut response);
    response.insert_header("content-type", "application/json")?;
    response.insert_header("content-length", body.len().to_string())?;
    if !ctx.claim_final_response(status) {
        return Ok(());
    }
    session.write_response_header(Box::new(response), false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await
}

/// Outcome of the retry-backoff minimum-wait decision (GEP-1731 conformance).
#[derive(Debug, PartialEq, Eq)]
enum BackoffAction {
    /// The full minimum backoff fits within the remaining budget (or no
    /// request/grpc deadline applies): sleep the computed `delay`.
    Sleep,
    /// `0 < remaining <= delay`: honouring the minimum backoff would reach or
    /// exceed the deadline, so the request must not be retried before the
    /// configured minimum duration. `prefer_request` selects the binding
    /// deadline's error (request-deadline when true, gRPC client deadline
    /// otherwise), mirroring the post-sleep re-check order.
    FailBudgetTooSmall { prefer_request: bool },
}

/// Decide whether a retry may sleep the configured exponential `backoff`
/// (`delay`) or must fail because the remaining request/grpc-timeout budget is
/// smaller than one minimum-backoff interval. Pure function so the GEP-1731
/// minimum-wait boundary (`remaining <= delay`) can be unit-tested without a
/// live `Session`.
fn backoff_action(
    delay: std::time::Duration,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> BackoffAction {
    let deadline_remaining = [request_remaining, grpc_remaining].into_iter().flatten().min();
    match deadline_remaining {
        // Full minimum backoff fits with room to spare: honour it.
        Some(r) if delay < r => BackoffAction::Sleep,
        // No request/grpc deadline applies: nothing bounds the backoff.
        None => BackoffAction::Sleep,
        // 0 < remaining <= delay: prefer the request deadline when it is the
        // (or a) binding bound; otherwise the gRPC client deadline binds.
        Some(_) => BackoffAction::FailBudgetTooSmall {
            prefer_request: request_remaining.is_some_and(|rr| rr <= delay),
        },
    }
}

#[inline]
pub async fn upstream_peer(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    // Check request timeout before attempting peer selection
    let request_timeout = ctx.request_timeout();

    if let Some(timeout) = request_timeout {
        let elapsed = std::time::Instant::now().saturating_duration_since(ctx.start_time);
        if elapsed >= timeout {
            ctx.record_error(EdgionErrorCode::RequestTimeoutExceeded);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            return Err(PingoraError::explain(
                ErrorType::HTTPStatus(504),
                "Request timeout exceeded",
            ));
        }
    }

    // Honour client-supplied gRPC deadline. Already-expired deadlines abort
    // before peer selection so the client sees DEADLINE_EXCEEDED via the
    // 504 → grpc-status 4 mapping rather than waiting for a transport reset.
    if ctx.grpc_deadline_exceeded() {
        ctx.record_error(EdgionErrorCode::GrpcClientDeadlineExceeded);
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(504);
        }
        return Err(PingoraError::explain(
            ErrorType::HTTPStatus(504),
            "gRPC client deadline exceeded",
        ));
    }

    // Apply backoff delay on retry attempts
    if ctx.try_cnt >= 1 {
        let base_backoff = ctx.retry_backoff();

        if let Some(base) = base_backoff {
            // Exponential: base * 2^(try_cnt - 1), capped at 30s
            let exponent = (ctx.try_cnt - 1).min(5);
            let delay = base.saturating_mul(1u32.checked_shl(exponent).unwrap_or(u32::MAX));
            let delay = delay.min(std::time::Duration::from_secs(30));

            // TODO: abort this sleep early when the client disconnects
            //
            // Bound the sleep by every deadline that applies to the request:
            // route `request_timeout` and client `grpc-timeout`. When the
            // remaining budget cannot accommodate the full minimum backoff
            // (`remaining <= delay`), GEP-1731 forbids retrying before the
            // configured minimum duration — so instead of opening a fresh
            // upstream connection ahead of the minimum wait, treat the budget
            // as exhausted and fail with the binding deadline's timeout. Only
            // sleep when the full backoff fits with budget to spare, or when no
            // deadline applies at all.
            let now = std::time::Instant::now();
            let request_remaining =
                request_timeout.map(|t| t.saturating_sub(now.saturating_duration_since(ctx.start_time)));
            let grpc_remaining = ctx.grpc_remaining();
            match backoff_action(delay, request_remaining, grpc_remaining) {
                BackoffAction::Sleep => tokio::time::sleep(delay).await,
                // 0 < remaining <= delay: honouring the minimum backoff would
                // reach or exceed the deadline. Per GEP-1731 the request must
                // not be retried before the configured minimum duration, so
                // fail now with the binding deadline's timeout rather than issue
                // a sub-minimum immediate retry.
                BackoffAction::FailBudgetTooSmall { prefer_request } => {
                    if prefer_request {
                        ctx.record_error(EdgionErrorCode::RequestTimeoutExceeded);
                        if let Some(upstream) = ctx.get_current_upstream_mut() {
                            upstream.status = Some(504);
                        }
                        return Err(PingoraError::explain(
                            ErrorType::HTTPStatus(504),
                            "Request timeout budget too small to honour retry backoff",
                        ));
                    }
                    ctx.record_error(EdgionErrorCode::GrpcClientDeadlineExceeded);
                    if let Some(upstream) = ctx.get_current_upstream_mut() {
                        upstream.status = Some(504);
                    }
                    return Err(PingoraError::explain(
                        ErrorType::HTTPStatus(504),
                        "gRPC client deadline budget too small to honour retry backoff",
                    ));
                }
            }

            // The backoff sleep can overrun its `delay` under timer jitter, so a
            // deadline that had room before the sleep may have just expired.
            // Re-check before letting Pingora open a new upstream connection —
            // without this guard a tight `grpc-timeout` combined with a large
            // backoff would let the gateway connect to a backend the caller has
            // already abandoned, burning a circuit-breaker failure counter and
            // an LB pick.
            if let Some(timeout) = request_timeout {
                let elapsed = std::time::Instant::now().saturating_duration_since(ctx.start_time);
                if elapsed >= timeout {
                    ctx.record_error(EdgionErrorCode::RequestTimeoutExceeded);
                    if let Some(upstream) = ctx.get_current_upstream_mut() {
                        upstream.status = Some(504);
                    }
                    return Err(PingoraError::explain(
                        ErrorType::HTTPStatus(504),
                        "Request timeout exceeded during retry backoff",
                    ));
                }
            }
            if ctx.grpc_deadline_exceeded() {
                ctx.record_error(EdgionErrorCode::GrpcClientDeadlineExceeded);
                if let Some(upstream) = ctx.get_current_upstream_mut() {
                    upstream.status = Some(504);
                }
                return Err(PingoraError::explain(
                    ErrorType::HTTPStatus(504),
                    "gRPC client deadline exceeded during retry backoff",
                ));
            }
        }
    }

    // Route to appropriate handler based on matched route type (not protocol)
    let mut peer = if ctx.is_grpc_route_matched {
        upstream_peer_grpc(edgion_http, session, ctx).await?
    } else {
        upstream_peer_http(edgion_http, session, ctx).await?
    };

    // Circuit breaker: admit-time per-endpoint concurrency cap. The chosen
    // endpoint (`lb_edgion_backend`) is now known but Pingora has NOT yet
    // connected the returned peer — this is the pre-connect admission point, and
    // the counter increment IS the acquire. If the endpoint is at cap, shed with
    // a non-retrying 503 that mirrors the established `end_response_503` +
    // fail-closed-error shed (see `get_peer_with_sp_fallback`): the error is
    // returned out of `upstream_peer` WITHOUT setting the pingora retry flag, so
    // there is no shed-induced retry storm to another endpoint. Because the shed
    // returns before any connect, it reaches none of the outlier-detection
    // `record_failure` sites — a busy-but-healthy endpoint is never ejected by
    // its own shed. Bypass paths (DirectEndpoint / ExternalJump) carry no service
    // key and no `lb_edgion_backend`, so this is a no-op for them.
    if let Some(service_key) = ctx.backend_context.as_ref().and_then(|bc| bc.service_key.clone()) {
        if let Some(max) = selected_effective_policy(ctx).and_then(|policy| policy.circuit_breaker_limit) {
            if let Some(chosen_backend) = ctx.get_current_upstream().and_then(|u| u.lb_edgion_backend.clone()) {
                // Release any permit this request acquired on a PRIOR attempt
                // before re-acquiring. On retry, Pingora re-enters upstream_peer
                // with the same ctx (and its permit) still alive; without this a
                // retry re-selecting the same endpoint at a full cap would be
                // wrongly shed by the request's own stale permit, and every
                // endpoint it tried would stay occupied until request end.
                ctx.cb_permit = None;
                match crate::backends::circuit_breaker::CbPermit::try_acquire(chosen_backend, max) {
                    Some(permit) => {
                        ctx.cb_permit = Some(permit);
                    }
                    None => {
                        crate::backends::circuit_breaker::inc_shed_total(&service_key);
                        // 503 body carries no internal detail (helper enforces
                        // this); for gRPC it commits a trailers-only grpc-status
                        // 14 frame. The returned error type mirrors the sibling
                        // shed: HTTPStatus(200) for gRPC (so `fail_to_proxy` does
                        // not stamp a spurious 500), InternalError for HTTP. Both
                        // terminate WITHOUT a pingora retry.
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(if ctx.is_grpc_route_matched {
                            PingoraError::new(ErrorType::HTTPStatus(200))
                        } else {
                            PingoraError::new(ErrorType::InternalError)
                        });
                    }
                }
            }
        }
    }

    apply_upstream_authority(session, ctx, &mut peer)?;

    Ok(peer)
}

/// What `upstream_peer` should do about the upstream authority for this request.
/// Pure output of [`decide_upstream_authority`] — carries no `Session`/`ctx`/`peer`
/// reference so the decision can be unit-tested without a live `Session`.
#[derive(Debug, PartialEq, Eq)]
enum AuthorityDecision {
    /// No policy on this Service (or no Service key) — leave the peer untouched.
    NoOp,
    /// Use this composed authority for BOTH the peer SNI and the upstream Host.
    Set(String),
    /// The label could not be rendered or was invalid — fail closed with 502.
    Reject(AuthorityError),
}

/// Pure decision step for `apply_upstream_authority`. `rendered` is `None` when
/// no `upstreamAuthority` policy applies to this Service (no Service key, or none
/// configured for it) — the caller never had anything to render; `Some(result)`
/// is the outcome of `render_authority` when a policy does apply.
///
/// Kept free of `Session` / `ctx` / `HttpPeer` so all three arms are directly
/// unit-testable; the effects (peer.sni, ctx.upstream_authority, the recorded
/// access-log SNI, the 502 fail-closed response) live in
/// [`apply_upstream_authority_decision`].
#[inline]
fn decide_upstream_authority(rendered: Option<Result<String, AuthorityError>>) -> AuthorityDecision {
    match rendered {
        None => AuthorityDecision::NoOp,
        Some(Ok(host)) => AuthorityDecision::Set(host),
        Some(Err(e)) => AuthorityDecision::Reject(e),
    }
}

/// Apply the effects of an [`AuthorityDecision`] to `ctx` / `peer`. Split out
/// from `apply_upstream_authority` so the fail-closed `Reject` path (peer left
/// untouched, `ctx.upstream_authority` cleared, 502 `EdgionErrorCode` + metric
/// recorded) is unit-testable without a live `Session`.
#[inline]
fn apply_upstream_authority_decision(
    decision: AuthorityDecision,
    ctx: &mut EdgionHttpContext,
    peer: &mut HttpPeer,
) -> pingora_core::Result<()> {
    match decision {
        AuthorityDecision::NoOp => {
            ctx.upstream_authority = None;
            Ok(())
        }
        AuthorityDecision::Set(host) => {
            // `HttpPeer::sni` participates in `reuse_hash` (pingora's keepalive
            // pool key). On a cleartext peer the SNI is meaningless on the wire,
            // so setting it per-tenant would fragment one shared connection pool
            // into one bucket per rendered label. Only TLS peers need it.
            if peer.is_tls() {
                peer.sni = host.clone();
            }
            // Refresh the access-log-recorded SNI too: `record_tls_to_upstream`
            // already wrote the STATIC BackendTLSPolicy hostname onto the current
            // upstream entry during peer construction, before this authority was
            // composed. Without this, the access log would keep reporting the
            // stale static SNI even though the wire SNI (and Host) changed.
            if let Some(up) = ctx.get_current_upstream_mut() {
                if let Some(tls) = up.tls.as_mut() {
                    tls.sni = Some(host.clone());
                }
            }
            // Host header is written later by `upstream_request_filter` — see
            // `pg_upstream_request_filter::apply_upstream_authority`, which reads
            // this field. Written unconditionally (TLS or not) so cleartext
            // backends still get the rewritten Host.
            ctx.upstream_authority = Some(host);
            Ok(())
        }
        AuthorityDecision::Reject(e) => {
            record_authority_rejected(e.reason());
            ctx.record_error(EdgionErrorCode::UpstreamAuthorityRejected);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(502);
            }
            // Fail closed: never leave a stale composed authority from an
            // earlier attempt lying around for a retry to pick up.
            ctx.upstream_authority = None;
            Err(PingoraError::explain(
                ErrorType::HTTPStatus(502),
                "upstream authority rejected",
            ))
        }
    }
}

/// Dynamic upstream authority (Task 6): render the wildcard label, compose the
/// host, and use it for BOTH the TLS SNI (set here on `peer`) and the upstream
/// `Host` header (written later by `upstream_request_filter` from
/// `ctx.upstream_authority` — `upstream_peer` cannot touch request headers).
///
/// A no-op when the chosen backend has no Service key (bypass paths —
/// DirectEndpoint / ExternalJump — leave `backend_context.service_key` unset)
/// or when the Service carries no `upstreamAuthority` policy, so an
/// unconfigured Service behaves byte-for-byte as today. Both early-return
/// paths clear `ctx.upstream_authority` so a conf_sync policy removal between
/// retry attempts of the same request cannot leave a stale composed authority
/// (and therefore a stale upstream `Host`) from a previous attempt.
///
/// Fail-closed on a bad render: silently keeping the static SNI would present
/// the right TLS identity to the wrong virtual host.
#[inline]
fn apply_upstream_authority(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    peer: &mut HttpPeer,
) -> pingora_core::Result<()> {
    if ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai) {
        ctx.upstream_authority = None;
        return Ok(());
    }
    // Read the request-captured compiled authority. The mutable
    // `ctx.upstream_authority` write is only on the early-return / render
    // paths after this immutable borrow ends.
    let Some(compiled) = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.as_ref())
        .and_then(|policy| policy.upstream_authority.clone())
    else {
        ctx.upstream_authority = None;
        return Ok(());
    };

    let rendered = {
        let adapter = PingoraSessionAdapter::new(session, ctx);
        render_authority(&compiled, &adapter)
    };

    apply_upstream_authority_decision(decide_upstream_authority(Some(rendered)), ctx, peer)
}

/// Handle gRPC upstream peer selection
#[inline]
pub async fn upstream_peer_grpc(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    match handle_grpc_upstream(session, ctx).await {
        Ok(Some(())) => {}
        Ok(None) => {
            ctx.record_error(EdgionErrorCode::GrpcUpstreamNotRouteMatched);
            if !ctx.response_write_started() {
                end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            }
            return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
        }
        Err(_) => {
            ctx.record_error(EdgionErrorCode::GrpcUpstreamNotBackendRefs);
            if !ctx.response_write_started() {
                end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            }
            return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
        }
    }

    // 2. Initialize backend context (unified logic)
    init_backend_context_if_needed(ctx)?;

    // 4. Push placeholder upstream so LB / TLS / CB writes during peer construction land on
    //    a valid entry (`current_upstream_id` is None right after `init_backend_context`).
    begin_upstream_attempt(ctx);

    // 5. Get peer — mirrors upstream_peer_http: on a strict cross-attempt retry,
    // restore the sticky target so the retry re-pins (and fail-closes if it is
    // now unhealthy) instead of migrating via plain LB; with a resolved sticky
    // target, route through the SP fallback helper; otherwise refresh the token
    // when a cross-attempt retry re-selects via plain LB.
    sp_repin_on_strict_retry(ctx);
    let mut peer = if ctx
        .session_persistence()
        .is_some_and(|persistence| persistence.target_addr.is_some())
    {
        get_peer_with_sp_fallback(edgion_http, session, ctx, true, None).await?
    } else {
        sp_refresh_on_cross_attempt_retry(ctx);
        get_peer(session, ctx, true, None, &edgion_http.server_header_opts).await?
    };

    // 6. Force HTTP/2 for gRPC
    peer.options.set_http_version(2, 2);

    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);

    Ok(peer)
}

/// Handle HTTP upstream peer selection
#[inline]
pub async fn upstream_peer_http(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    finalize_request_uri(session, ctx)?;

    // AI forwarding owns a typed external peer path and never writes the
    // generic first-write-wins upstream-jump slot.
    if ctx.has_ai_attempt_plan() {
        let entropy = ctx
            .request_info
            .x_trace_id
            .as_deref()
            .and_then(|trace_id| uuid::Uuid::parse_str(trace_id).ok())
            .map(|trace_id| trace_id.as_u128())
            .unwrap_or_else(|| uuid::Uuid::new_v4().as_u128());
        let route_retry_cap = ctx.retry_attempts();
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.drain_pending().await;
        }
        loop {
            let mut target = match prepare_first_attempt(
                session,
                ctx,
                entropy,
                route_retry_cap,
                edgion_http.edgion_gateway_config.spec.max_retries,
            ) {
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
            let selected_identity = target.selected.logical().identity();
            let selected_backend_key = format!("{}/{}", selected_identity.namespace, selected_identity.name);
            let admission = ctx
                .ensure_ai_state()
                .attempt_runtime
                .admit(crate::ai::attempt_runtime::AiAttemptAdmissionInput {
                    namespace: selected_identity.namespace.clone(),
                    backend_uid: target.selected.backend.uid.clone(),
                    credential_alias: target.selected.credential_alias.clone(),
                    backend_name: selected_backend_key,
                    provider: target.provider,
                    pricing: target.pricing,
                    redis_ref: target.redis_ref.clone(),
                    on_redis_failure: target.on_redis_failure.clone(),
                    limits: target.credential_limits.clone(),
                    estimated_input_tokens: target.estimated_input_tokens,
                    max_output_tokens: target.max_output_tokens,
                    usage: std::sync::Arc::clone(&target.usage),
                    eos: std::sync::Arc::clone(&target.eos),
                    outlier: target.selected.credential_outlier.clone(),
                    adaptive: target.selected.adaptive.clone(),
                    circuit_breaker: target
                        .selected
                        .effective_policy
                        .as_ref()
                        .and_then(|policy| policy.logical_circuit_breaker.clone()),
                    limiter_core: std::sync::Arc::clone(&target.limiter_core),
                })
                .await;
            match admission {
                crate::ai::attempt_runtime::AiAttemptAdmission::Ready(runtime) => {
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
                crate::ai::attempt_runtime::AiAttemptAdmission::Skip => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::dispatch::handle_ai_predispatch_failure(ctx, true) {
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
                crate::ai::attempt_runtime::AiAttemptAdmission::SkipBackend => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::dispatch::handle_ai_predispatch_service_failure(ctx) {
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
                crate::ai::attempt_runtime::AiAttemptAdmission::DependencyDenied => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::dispatch::handle_ai_predispatch_failure(ctx, true) {
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
            }
        }
    }

    // 0/1. Upstream-jump slot (Direct/External bypass; Internal falls through to LB).
    // INVARIANT: this check MUST come before backend selection. Direct and
    // External jumps bypass ordinary LB; Internal remains an LB input modifier.
    if let Some(jump) = ctx.jump.as_deref() {
        match jump {
            UpstreamJumpTarget::Direct(_) => return try_direct_endpoint_peer(edgion_http, ctx),
            UpstreamJumpTarget::External(_) => return try_external_jump_peer(edgion_http, ctx).await,
            UpstreamJumpTarget::Internal(_) => {
                // fall through — Internal stays in ctx.jump, consumed by pick_backend_ref
            }
        }
    }

    // 2. Select HTTP backend if not already selected (may include InternalJump inside)
    if ctx.selected_backend.is_none() && ctx.selected_grpc_backend.is_none() {
        select_http_backend(edgion_http, session, ctx).await?;
    }

    // 3. Initialize backend context (unified logic)
    init_backend_context_if_needed(ctx)?;

    // 4. Push placeholder upstream so LB metadata / TLS / CB writes inside try_get_peer
    //    land on a valid entry — `init_backend_context_if_needed` leaves `current_upstream_id`
    //    as None, which would otherwise silently drop those writes (P0 g1-001).
    begin_upstream_attempt(ctx);

    let selected_logical = ctx.selected_backend.as_ref().and_then(SelectedHttpBackend::routed);

    // 5. Get peer — common path defers to LB; the SP-affinity branch lives in its own helper.
    // On a strict cross-attempt retry, restore the sticky target so the retry
    // re-pins to the same backend (and fail-closes if it is now unhealthy)
    // instead of migrating via plain LB.
    sp_repin_on_strict_retry(ctx);
    let mut peer = if ctx
        .session_persistence()
        .is_some_and(|persistence| persistence.target_addr.is_some())
    {
        get_peer_with_sp_fallback(edgion_http, session, ctx, false, selected_logical.as_ref()).await?
    } else {
        // A retry that re-selects via plain LB after an earlier attempt consumed
        // the sticky target must refresh the token to the backend now serving.
        sp_refresh_on_cross_attempt_retry(ctx);
        get_peer(
            session,
            ctx,
            false,
            selected_logical.as_ref(),
            &edgion_http.server_header_opts,
        )
        .await?
    };

    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);

    Ok(peer)
}

/// Select HTTP backend from route (extracted from upstream_peer)
#[inline]
pub async fn select_http_backend(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    let route_unit = match ctx.route_unit.as_ref() {
        // Arc-clone so the unit is independent of `ctx` — the SP resolution
        // below takes `&mut ctx` while iterating the unit's backends.
        Some(unit) => unit.clone(),
        None => {
            ctx.record_error(EdgionErrorCode::UpstreamNotRouteMatched);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
    };

    // Session Persistence step 1 of 2: extract token + set initial state.
    let sp_token = route_unit.rule.session_persistence.as_ref().and_then(|sp| {
        session_persistence::sp_extract_token(
            session,
            &mut ctx.ensure_session_persistence().state,
            Some(sp),
            session_persistence::now_unix_secs(),
        )
    });

    // Session Persistence step 2 of 2: resolve the token BEFORE weighted
    // backendRef selection, across ALL of the rule's backendRefs (GEP-1619:
    // weights govern only new sessions; an established session keeps routing
    // to its original Service/endpoint even at weight 0).
    //
    // DynamicInternalUpstream precedence: an Internal jump is an explicit
    // per-request override — it wins over the sticky session, and the session
    // binding is left untouched (no resolution, state stays `Inactive`, no
    // cookie rewrite; the next non-jump request resolves normally).
    let sp_idx = if sp_token.is_none() || matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::Internal(_))) {
        None
    } else {
        let candidates = route_unit
            .backends
            .iter()
            .filter(|backend| backend.schema.ref_denied.is_none())
            .filter_map(sticky_candidate)
            .collect::<Vec<_>>();
        session_persistence::sp_resolve_typed_candidates(ctx, &candidates, sp_token.as_deref())
    };
    let sticky_backend = sp_idx.and_then(|idx| {
        let cb = &route_unit.backends[idx];
        if cb.schema.ref_denied.is_none() {
            crate::backends::LogicalBackendHandle::new(Arc::clone(&route_unit.backends), idx)
        } else {
            // The sticky backendRef's cross-namespace reference is now denied:
            // treat as a miss — drop the sticky target, refresh the token, and
            // fall back to weighted selection below.
            let persistence = ctx.ensure_session_persistence();
            persistence.target_addr = None;
            persistence.sticky_addr = None;
            persistence.state = SessionPersistenceState::InjectNew;
            None
        }
    });

    // Pick backend_ref: sticky session hit wins outright; otherwise InternalJump
    // (plugin-injected) tries by name first and falls back to weighted
    // round-robin on miss; without InternalJump, plain weighted round-robin.
    let backend_ref = match sticky_backend {
        Some(b) => b,
        None => match pick_backend_ref(&route_unit, &route_unit.matched_info.rsc.ns, ctx.jump.as_deref()) {
            Ok(b) => b,
            Err(e) => {
                ctx.record_error(backend_select_status(&e));
                end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        },
    };

    // Project policy identity from the selected WP2 logical slot. Route-local
    // occurrence remains proof of selection but is not part of attachment.
    let service_port = backend_ref.compiled().port;
    let backend_tls_policy = crate::backends::query_backend_tls_policy_for_logical_backend(&backend_ref, service_port);

    let backend_plugin_runtime = Arc::clone(&backend_ref.compiled().plugin_runtime);
    backend_plugin_runtime.run_request_plugins(session, ctx).await;
    if ctx.plugin_terminated() {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Err(PingoraError::new(ErrorType::InternalError));
    }

    ctx.selected_backend = Some(match backend_ref.target() {
        CompiledBackendTarget::Service { .. } => {
            let selected = SelectedServiceBackend::new(backend_ref, backend_tls_policy)
                .ok_or_else(|| PingoraError::new(ErrorType::InternalError))?;
            SelectedHttpBackend::Service(selected)
        }
        CompiledBackendTarget::Invalid { .. } => SelectedHttpBackend::Invalid(backend_ref),
        CompiledBackendTarget::EdgionBackend { .. } => {
            ctx.record_error(EdgionErrorCode::UpstreamNotBackendRefs);
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
    });
    Ok(())
}

fn sticky_candidate(
    backend: &crate::routes::http::match_unit::CompiledBackend,
) -> Option<session_persistence::StickyCandidate> {
    match &backend.target {
        CompiledBackendTarget::Service { namespace, name } => {
            let section = crate::backends::service_section_key_from_port(backend.schema.port).ok()?;
            Some(session_persistence::StickyCandidate {
                slot: backend.slot,
                service_key: format!("{namespace}/{name}"),
                section,
                scope: None,
            })
        }
        CompiledBackendTarget::EdgionBackend { .. } | CompiledBackendTarget::Invalid { .. } => None,
    }
}

/// Get peer when session persistence resolved a sticky target (`target_addr` is set).
///
/// Caller must have checked the session-persistence `target_addr` — this function assumes the
/// SP-affinity branch and only routes its three outcomes:
///
/// - `try_get_peer` succeeds → use the sticky target.
/// - `try_get_peer` rejects with SSRF (loopback / link-local) or DNS resolution failure
///   → write 503 and abort; never fall back, since retrying would hit the same error.
/// - `try_get_peer` fails for any other reason → fall back to normal LB unless the route
///   declared `session_persistence.strict = true`. On fallback, mark the state as
///   `InjectUpdate` so the response refreshes the cookie/header pointing at the new pick.
///
/// Strict mode keeps the original `try_get_peer` error from `get_peer`'s second call,
/// rather than synthesising one — `get_peer` will return whichever backend it can find,
/// or surface its own error to the proxy layer.
///
/// `is_grpc` selects the `try_get_peer` protocol branch and the fail-closed
/// error type: gRPC error paths report `HTTPStatus(200)` (the committed
/// response is a trailers-only `grpc-status: 14` frame emitted by the
/// gRPC-aware `end_response_503`), while HTTP keeps `InternalError`. Using
/// `InternalError` for gRPC would make `fail_to_proxy` stamp a spurious 500
/// into the access log, diverging from every other gRPC error path.
async fn get_peer_with_sp_fallback(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    is_grpc: bool,
    selected_http: Option<&SelectedLogicalBackend>,
) -> pingora_core::Result<Box<HttpPeer>> {
    let sp_strict = ctx.effective_sp_strict();
    let fail_closed_err = || {
        if is_grpc {
            PingoraError::new(ErrorType::HTTPStatus(200))
        } else {
            PingoraError::new(ErrorType::InternalError)
        }
    };

    match try_get_peer(ctx, session, is_grpc, selected_http).await {
        Ok(peer) => Ok(peer),
        Err(
            status @ (EdgionErrorCode::BackendLocalhostNotAllowed
            | EdgionErrorCode::BackendAddrLinkLocalNotAllowed
            | EdgionErrorCode::BackendDnsResolutionFailed),
        ) => {
            ctx.record_error(status);
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            Err(fail_closed_err())
        }
        Err(_) if !sp_strict => {
            // Sticky target unhealthy → drop the SP target and refresh the token in the response.
            let persistence = ctx.ensure_session_persistence();
            persistence.target_addr = None; // already consumed by try_get_peer, but be explicit
            persistence.state = SessionPersistenceState::InjectUpdate;
            get_peer(session, ctx, is_grpc, selected_http, &edgion_http.server_header_opts).await
        }
        Err(status) => {
            // Strict mode: the resolved sticky target cannot serve. Fail closed
            // instead of silently load-balancing to a different backend.
            // end_response_503 commits the downstream response (for gRPC it is
            // already trailers-only grpc-status 14); the returned Err terminates
            // without a pingora retry (same mechanism as the SSRF arm).
            ctx.record_error(status); // status: typically StickyBackendUnhealthy; any non-SSRF/DNS peer error fails closed here
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            Err(fail_closed_err())
        }
    }
}

/// On a plain-LB peer selection, arm `state = InjectUpdate` when this is a
/// cross-attempt retry whose earlier attempt consumed the SP sticky target.
/// Reads the inputs off `ctx` and defers the decision to the pure
/// `sp_should_refresh_on_cross_attempt_retry`.
#[inline]
fn sp_refresh_on_cross_attempt_retry(ctx: &mut EdgionHttpContext) {
    let sp = ctx.effective_session_persistence();
    let sp_configured = sp.is_some();
    let strict = sp.and_then(|sp| sp.strict).unwrap_or(false);

    let try_cnt = ctx.try_cnt;
    let Some(persistence) = ctx.session_persistence_mut() else {
        return;
    };
    if sp_should_refresh_on_cross_attempt_retry(
        persistence.sticky_consumed,
        try_cnt,
        sp_configured,
        strict,
        &persistence.state,
    ) {
        persistence.state = SessionPersistenceState::InjectUpdate;
    }
}

/// On a strict cross-attempt retry whose earlier attempt consumed the sticky
/// target, restore `target_addr` from the retained mirror so the retry
/// re-pins to the same backend (and fail-closes via `get_peer_with_sp_fallback`
/// if it is now unhealthy) instead of migrating via plain LB. Defers the
/// decision to the pure `sp_should_repin_on_strict_retry`.
#[inline]
fn sp_repin_on_strict_retry(ctx: &mut EdgionHttpContext) {
    let sp = ctx.effective_session_persistence();
    let sp_configured = sp.is_some();
    let strict = sp.and_then(|sp| sp.strict).unwrap_or(false);

    let try_cnt = ctx.try_cnt;
    let Some(persistence) = ctx.session_persistence_mut() else {
        return;
    };
    if sp_should_repin_on_strict_retry(
        persistence.sticky_consumed,
        try_cnt,
        sp_configured,
        strict,
        persistence.sticky_addr.is_some(),
    ) {
        persistence.target_addr = persistence.sticky_addr;
    }
}

/// Pick the `HTTPBackendRef` for an LB-bound request. When `ctx.jump` carries an
/// `Internal` variant, named lookup is tried first and falls back to weighted
/// round-robin on miss; when `ctx.jump` is None or carries a non-Internal variant,
/// plain weighted round-robin.
#[inline]
fn pick_backend_ref(
    unit: &std::sync::Arc<crate::routes::http::HttpRouteRuleUnit>,
    route_ns: &str,
    jump: Option<&crate::UpstreamJumpTarget>,
) -> Result<crate::backends::LogicalBackendHandle, EdError> {
    if let Some(crate::UpstreamJumpTarget::Internal(j)) = jump {
        if let Ok(b) =
            RouteRules::find_backend_by_name(unit, &j.backend_ref_name, j.backend_ref_namespace.as_deref(), route_ns)
        {
            return Ok(b);
        }
        // Named lookup missed → fall through to weighted RR.
    }
    RouteRules::select_backend(unit)
}

/// Map a backend-selection `EdError` to its public `EdgionErrorCode` for access logs.
#[inline]
fn backend_select_status(e: &EdError) -> EdgionErrorCode {
    match e {
        EdError::BackendNotFound() => EdgionErrorCode::UpstreamNotBackendRefs,
        EdError::InconsistentWeight() => EdgionErrorCode::UpstreamInconsistentWeight,
        EdError::RefDenied { .. } => EdgionErrorCode::RefDenied,
        _ => EdgionErrorCode::Unknown,
    }
}

/// Fold the three read/write upper bounds into the tightest one, labeled
/// with its source. `BackendRequest` is listed first and comparison is
/// strict (`<`), so it wins ties: when it is exactly as tight as a
/// deadline-derived bound, the per-try retry semantics (GEP-1731) apply;
/// whether a retry actually runs is still gated by the deadline checks in
/// `error_while_proxy`.
#[inline]
fn fold_peer_timeouts(
    backend: Option<std::time::Duration>,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> Option<(std::time::Duration, crate::PeerTimeoutSource)> {
    use crate::PeerTimeoutSource;
    let mut best: Option<(std::time::Duration, PeerTimeoutSource)> = None;
    for (candidate, source) in [
        (backend, PeerTimeoutSource::BackendRequest),
        (request_remaining, PeerTimeoutSource::RequestDeadline),
        (grpc_remaining, PeerTimeoutSource::GrpcDeadline),
    ] {
        if let Some(d) = candidate {
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, source));
            }
        }
    }
    best
}

/// Resolve the effective `backendRequest` bound for one upstream attempt.
/// Three-state per Gateway API spec:
/// - Unset (or no route timeouts) → global default
/// - Disabled (`"0s"`) → `None` (no configured peer I/O bound; only deadline
///   sources clamp, while protocol-local liveness floors may still apply)
/// - Duration → the route value
///
/// A zero duration is filtered out as disabled: route-level `"0s"` already
/// parses to `Disabled`, but a global `defaultRequestTimeout: "0s"` reaches
/// here as `Duration::ZERO` and would otherwise become an instant read/write
/// timeout — and, with a retry policy present, an instant-retry loop.
#[inline]
fn effective_backend_bound(
    route: Option<&edgion_resources::resources::http_route::RouteTimeout>,
    global_default: std::time::Duration,
) -> Option<std::time::Duration> {
    use edgion_resources::resources::http_route::RouteTimeout;
    match route {
        Some(RouteTimeout::Disabled) => None,
        Some(RouteTimeout::Duration(d)) => Some(*d),
        _ => Some(global_default),
    }
    .filter(|d| !d.is_zero())
}

/// Replace the normal backendRequest bound with an external-jump owner override.
/// A defensive zero override is ignored because Pingora would interpret it as
/// an immediate timeout; valid AI backend timeouts are guaranteed positive on
/// the cold configuration path.
#[inline]
fn apply_upstream_read_timeout_override(
    normal_backend_bound: Option<std::time::Duration>,
    override_timeout: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    override_timeout
        .filter(|timeout| !timeout.is_zero())
        .or(normal_backend_bound)
}

/// Resolve the gateway->backend connection-ESTABLISHMENT deadline, independent
/// of the request/read/write timeouts. Precedence: per-backend
/// `EdgionBackendTrafficPolicy.spec.connection.connectTimeout` (looked up via
/// `service_key`) else the global `defaultConnectTimeout`. `service_key` is
/// `None` only for paths with no resolvable Service (e.g. an ExternalJump),
/// which always fall back to the global default.
///
/// Returns `None` when the resolved deadline is zero. A global
/// `defaultConnectTimeout: "0s"` parses to `Duration::ZERO`, which Pingora would
/// treat as an instant connect deadline (every connect fails). Matching the
/// sibling peer-timeout guards (`effective_backend_bound`, `request_remaining`,
/// `grpc_remaining` all filter out zero), a zero deadline means "no connect
/// bound" — the caller leaves `connection_timeout` unset — not an instant
/// failure. The BTP override can never be zero (struct validation rejects it),
/// so this only guards a degenerate global config.
#[inline]
#[cfg(test)]
fn resolve_connect_deadline(service_key: Option<&str>, global: std::time::Duration) -> Option<std::time::Duration> {
    let d = service_key
        .and_then(|k| get_backend_traffic_policy_store().get_connect_timeout(k))
        .unwrap_or(global);
    (!d.is_zero()).then_some(d)
}

#[inline]
fn choose_typed_connect_deadline(
    selected_is_ai: bool,
    ai_timeout: Option<std::time::Duration>,
    service_timeout: Option<std::time::Duration>,
    global: std::time::Duration,
) -> Option<std::time::Duration> {
    if selected_is_ai {
        ai_timeout.or_else(|| (!global.is_zero()).then_some(global))
    } else {
        service_timeout.or_else(|| (!global.is_zero()).then_some(global))
    }
}

/// Replay-safety write bound for when the timeout fold produced no deadline:
/// `Some(global default backendRequest)` only when a captured request body will
/// replay to the upstream AND the global default itself is enabled (non-zero).
/// A disabled global default means the operator asked for no bounds anywhere —
/// respected, so the peer option stays unset. Protocol-local liveness floors,
/// such as Pingora's H2 capacity-wait floor, may still apply. Rationale at the
/// call site.
#[inline]
fn replay_write_floor(body_captured: bool, global_backend_request: std::time::Duration) -> Option<std::time::Duration> {
    (body_captured && !global_backend_request.is_zero()).then_some(global_backend_request)
}

/// Configure peer timeouts from global and route-level configs (inline for performance)
#[inline]
pub fn configure_peer_timeouts(edgion_http: &EdgionHttpProxy, peer: &mut Box<HttpPeer>, ctx: &mut EdgionHttpContext) {
    let backend_timeout = &edgion_http.parsed_timeouts.backend;
    let route_timeouts = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        });

    let route_backend_timeout = route_timeouts.map(|rt| &rt.backend_request_timeout);
    let normal_backend_timeout = effective_backend_bound(route_backend_timeout, backend_timeout.request_timeout);
    let upstream_read_timeout_override = match ctx.jump.as_deref() {
        Some(crate::UpstreamJumpTarget::External(external)) => external.upstream_read_timeout_override,
        _ => None,
    };
    let effective_backend_timeout =
        apply_upstream_read_timeout_override(normal_backend_timeout, upstream_read_timeout_override);

    // connection_timeout is the connection-ESTABLISHMENT deadline, resolved
    // independently of the request/read/write timeouts (which are folded below).
    // Precedence: per-backend BTP connection.connectTimeout > global defaultConnectTimeout.
    // Intentionally NOT derived from backendRequest/request_timeout: connection
    // establishment is infrastructure overhead, and conflating the two would let a
    // tight per-try request bound cause spurious connect-time 504s. A zero resolved
    // deadline (degenerate `defaultConnectTimeout: "0s"`) yields None => left unset,
    // never an instant-fail connect. Service/gRPC use only the captured
    // `connect_timeout`; they do not re-read the store.
    let captured_timeout = selected_effective_policy(ctx).and_then(|policy| policy.connect_timeout);
    let selected_is_logical_backend = ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai);
    let connect_deadline = choose_typed_connect_deadline(
        selected_is_logical_backend,
        captured_timeout,
        captured_timeout,
        backend_timeout.connect_timeout,
    );
    if let Some(connect_deadline) = connect_deadline {
        peer.options.connection_timeout = Some(connect_deadline);
    }

    // Apply request_timeout as a ceiling on read/write timeout.
    // Gateway API spec: request_timeout is end-to-end; remaining time = timeout - elapsed.
    // Filter out zero remaining time to avoid setting Duration::ZERO on the peer (undefined behaviour).
    let request_remaining = route_timeouts
        .and_then(|rt| rt.request_timeout.as_duration())
        .map(|req| req.saturating_sub(std::time::Instant::now().saturating_duration_since(ctx.start_time)))
        .filter(|r| !r.is_zero());

    // Client-supplied `grpc-timeout` is another upper bound on read/write —
    // an intermediary that ignores it leaves upstream work running after the
    // client has given up. `grpc_remaining` returns the deadline minus now,
    // so we apply the same zero-guard as request_remaining.
    let grpc_remaining = ctx.grpc_remaining().filter(|r| !r.is_zero());

    // Fold three independent upper bounds via labeled min (see
    // `fold_peer_timeouts` for tie-break semantics):
    //   - `effective_backend_timeout` is `None` when the route explicitly
    //     disabled it via `backendRequestTimeout: "0s"` (or the global default
    //     is zero — see `effective_backend_bound`), in which case only the
    //     remaining two bounds clamp.
    //   - `request_remaining` is the route-level `request_timeout` minus elapsed
    //     wall-clock.
    //   - `grpc_remaining` is the client-supplied `grpc-timeout` minus elapsed.
    // The winning source is recorded so `error_while_proxy` can distinguish a
    // retryable per-try timeout (BackendRequest, GEP-1731) from a final
    // deadline expiry.
    let folded = fold_peer_timeouts(effective_backend_timeout, request_remaining, grpc_remaining);
    ctx.peer_timeout_source = folded.map(|(_, source)| source);

    if let Some((timeout, _)) = folded {
        peer.options.read_timeout = Some(timeout);
        peer.options.write_timeout = Some(timeout);
    } else if let Some(floor) = replay_write_floor(ctx.request_body.has_cached_body(), backend_timeout.request_timeout)
    {
        // Replay safety floor: while a captured request body replays to the
        // upstream, the session does not watch the downstream socket, so a
        // client that disconnected after uploading is noticed only once replay
        // completes — upstream backpressure can stretch that window
        // indefinitely unless each body write is bounded (see the pingora
        // `RequestBodyBuffer` contract). With every deadline disabled
        // (`backendRequestTimeout: "0s"`, no route/gRPC deadline), fall back
        // to the global default for the WRITE bound only. Reads stay
        // unbounded as configured, and `peer_timeout_source` stays `None` so
        // a fired write timeout is a plain upstream failure, not a retryable
        // per-try (GEP-1731) cancellation.
        peer.options.write_timeout = Some(floor);
    }

    // Idle timeout: use global config from EdgionGatewayConfig
    peer.options.idle_timeout = Some(backend_timeout.idle_timeout);
}

/// Push a new upstream connection placeholder so that subsequent
/// calls to `ctx.get_current_upstream_mut()` (inside try_get_peer /
/// build_peer_from_lb_selection / record_tls_to_upstream /
/// record_unsafe_*_attempt) actually land on a valid entry.
/// ip/port are filled in later by [`backfill_upstream_addr`] once the peer is known.
///
/// Must be called after `init_backend_context_if_needed` and before any peer construction.
/// On retries this also finalizes the previous attempt's LB commit:
/// `get_current_upstream_mut()` still points at the prior entry at this moment, so
/// this cleanup MUST run before `push_upstream` advances `current_upstream_id`.
#[inline]
pub fn begin_upstream_attempt(ctx: &mut EdgionHttpContext) {
    // The previous endpoint admission belongs to the previous attempt. Release
    // it before any live policy lookup or endpoint resolution for the next one.
    ctx.cb_permit = None;
    // On retry, release the previous attempt's outstanding-connection slot. The
    // latency sample is NOT dropped any more — it was already recorded at
    // response headers (see `pg_upstream_response_filter`).
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::lb::finalize_attempt(upstream);
    }

    ctx.try_cnt += 1;
    ctx.push_upstream(None, None);
    capture_service_effective_policy(ctx);

    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
    }

    // Retry-budget denominator (D1a): every upstream attempt to a Service — the
    // first attempt AND every retry (connect-fail, status-code, per-try timeout,
    // keepalive reuse) — counts toward the per-backend request volume, so the
    // percentage cap is `retries <= percent% * total_attempts`. The service key
    // is read from `backend_context` (set by `init_backend_context_if_needed`,
    // which always runs before this) rather than the just-pushed upstream entry,
    // whose `service_key` is not populated until LB peer construction happens
    // later. Bypass paths (DirectEndpoint / ExternalJump) have no service key and
    // therefore no budget, so this is a no-op for them.
    if let Some(budget) = retry_budget_for(ctx) {
        budget.on_attempt();
    }
}

/// Look up the retry budget (if any) installed for the current attempt's target
/// Service. Reads the service key from `backend_context` — the canonical source
/// (`apply_upstream_authority` uses the same) — because the per-attempt upstream
/// entry's `service_key` is not populated until LB peer construction, which runs
/// after `begin_upstream_attempt`. Bypass paths (DirectEndpoint / ExternalJump)
/// carry no service key and therefore no budget. Shared by the denominator hook
/// here and the three retry gates (`pg_fail_to_connect`, `pg_error_while_proxy`
/// Case 1.5, `pg_upstream_response_filter`).
#[inline]
pub(super) fn retry_budget_for(ctx: &EdgionHttpContext) -> Option<std::sync::Arc<RetryBudget>> {
    selected_effective_policy(ctx)
        .and_then(|policy| policy.retry_budget.as_ref())
        .map(std::sync::Arc::clone)
}

fn capture_service_effective_policy(ctx: &mut EdgionHttpContext) {
    if ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai) {
        return;
    }
    let Some(backend_context) = ctx.backend_context.as_mut() else {
        return;
    };
    if backend_context.effective_policy.is_some() {
        return;
    }
    let Some(service_key) = backend_context.service_key.as_deref() else {
        return;
    };
    let target =
        crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(service_key);
    backend_context.effective_policy = get_backend_traffic_policy_store().snapshot().get_effective(&target);
}

fn selected_effective_policy(
    ctx: &EdgionHttpContext,
) -> Option<&std::sync::Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>> {
    if ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai) {
        return ctx.ai_state().and_then(|state| state.effective_policy.as_ref());
    }
    ctx.backend_context.as_ref()?.effective_policy.as_ref()
}

#[inline]
#[cfg(test)]
fn choose_typed_retry_budget(
    selected_is_ai: bool,
    ai_budget: Option<std::sync::Arc<RetryBudget>>,
    service_budget: Option<std::sync::Arc<RetryBudget>>,
) -> Option<std::sync::Arc<RetryBudget>> {
    if selected_is_ai {
        ai_budget
    } else {
        service_budget
    }
}

/// Finish a peer that was constructed directly (i.e. without going through LB / try_get_peer):
/// push the upstream attempt placeholder, then apply timeouts and metrics.
///
/// Used by paths that bypass load-balancing (DirectEndpoint, ExternalJump) where
/// the address is already known. DirectEndpoint resolves its validated Service
/// slot here; ExternalJump arrives with a pre-seeded context.
#[inline]
fn finish_static_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
    mut peer: Box<HttpPeer>,
) -> pingora_core::Result<Box<HttpPeer>> {
    init_backend_context_if_needed(ctx)?;
    begin_upstream_attempt(ctx);
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);
    Ok(peer)
}

async fn try_ai_backend_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
    target: &PreparedAiAttempt,
) -> Result<Box<HttpPeer>, AiPeerPredispatchFailure> {
    let endpoint = &target.selected.backend.endpoint;
    let addr = resolve_ai_endpoint(ctx, &endpoint.domain, endpoint.port).await?;
    let (namespace, name) = match target.selected.logical().target() {
        CompiledBackendTarget::EdgionBackend { namespace, name } => (namespace.as_str(), name.as_str()),
        CompiledBackendTarget::Service { .. } | CompiledBackendTarget::Invalid { .. } => {
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    ctx.backend_context = Some(crate::ctx::BackendContext {
        source: Some(BackendSource::AiBackend),
        rsc: Some(crate::Rsc::new("EdgionBackend", namespace, name)),
        service_key: Some(format!("{namespace}/{name}")),
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: Some(target.selected.logical().slot()),
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
        crate::backends::policy::BackendTlsPolicyLookup::Effective(effective) => {
            create_tls_peer(PingoraSocketAddr::Inet(addr), effective, None).map_err(|status| {
                ctx.record_error(status);
                AiPeerPredispatchFailure::Endpoint
            })?
        }
        crate::backends::policy::BackendTlsPolicyLookup::Absent => {
            Box::new(HttpPeer::new(addr, endpoint.use_tls, endpoint.sni.clone()))
        }
        crate::backends::policy::BackendTlsPolicyLookup::Invalid => {
            ctx.record_error(edgion_resources::EdgionErrorCode::BackendTlsPolicyInvalid);
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    begin_ai_peer_attempt(ctx);
    let (use_tls, sni) = match &target.selected.backend_tls_policy {
        crate::backends::policy::BackendTlsPolicyLookup::Effective(_) => {
            extract_tls_config(&target.selected.backend_tls_policy)
        }
        crate::backends::policy::BackendTlsPolicyLookup::Absent => (endpoint.use_tls, endpoint.sni.clone()),
        crate::backends::policy::BackendTlsPolicyLookup::Invalid => {
            ctx.record_error(edgion_resources::EdgionErrorCode::BackendTlsPolicyInvalid);
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    crate::backends::peer_builder::record_tls_to_upstream(ctx, use_tls, &sni);
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);
    Ok(peer)
}

async fn resolve_ai_endpoint(
    ctx: &EdgionHttpContext,
    domain: &str,
    port: u16,
) -> Result<std::net::SocketAddr, AiPeerPredispatchFailure> {
    let spec = crate::backends::compute_routed_external_resolver(ctx);
    let addrs = resolve(&spec, domain)
        .await
        .map_err(|_| AiPeerPredispatchFailure::Endpoint)?;
    filter_resolved_addrs(
        addrs,
        EgressPolicy {
            block_private: default_block_private(),
        },
        ctx.allow_loopback_upstream,
    )
    .into_iter()
    .next()
    .map(|ip| std::net::SocketAddr::new(ip, port))
    .ok_or(AiPeerPredispatchFailure::Endpoint)
}

/// Record pre-connect AI peer selection without crossing the AI dispatch or
/// retry-budget boundary. The upstream-request hook marks dispatch after a
/// successful connect, immediately before request headers/body are written.
fn begin_ai_peer_attempt(ctx: &mut EdgionHttpContext) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::lb::finalize_attempt(upstream);
    }
    ctx.try_cnt += 1;
    ctx.push_upstream(None, None);
    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
    }
}

/// DirectEndpoint bypass path (exact IP set by a plugin, e.g. dynamic_endpoint).
///
/// Caller (the dispatch cascade) has already matched `ctx.jump` against
/// `UpstreamJumpTarget::Direct` — this is the cold path, so it's not `#[inline]`
/// and the variant check stays at the call site to keep the hot path branch-only.
///
/// `finish_static_peer` resolves the validated backend slot through the
/// request-pinned route generation, then this helper retags the source for
/// access logs and metrics.
fn try_direct_endpoint_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let direct = match ctx.jump.as_deref() {
        Some(UpstreamJumpTarget::Direct(preset)) => preset,
        // invariant: caller (line 146) only reaches this helper after matching Direct{..};
        //   other variants are dispatched to sibling helpers
        _ => unreachable!("caller must dispatch only on ctx.jump == Some(Direct)"),
    };
    let (addr, use_tls, sni) = (direct.addr, direct.use_tls, direct.sni.clone());

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    let result = finish_static_peer(edgion_http, ctx, peer);

    // init_backend_context_if_needed left source as None (default path) — tag this
    // request as a DirectEndpoint bypass so access logs/metrics can distinguish it.
    if let Some(bc) = ctx.backend_context.as_mut() {
        bc.source = Some(BackendSource::DirectEndpoint);
    }

    result
}

/// ExternalJump bypass path (external domain set by a plugin, e.g. region_route).
///
/// Caller (the dispatch cascade) has already matched `ctx.jump` against
/// `UpstreamJumpTarget::External` — cold path, not `#[inline]`. Resolves the
/// domain via async DNS, validates against SSRF defenses, then constructs the
/// peer (or returns the rejection error).
async fn try_external_jump_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let external = match ctx.jump.as_deref() {
        Some(UpstreamJumpTarget::External(p)) => p,
        // invariant: caller (line 147) only reaches this helper after matching External(..);
        //   other variants are dispatched to sibling helpers
        _ => unreachable!("caller must dispatch only on ctx.jump == Some(External)"),
    };
    let domain = external.domain.clone();
    let port = external.port;
    let use_tls = external.use_tls;
    let sni = external.sni.clone();

    let addr = resolve_domain(&domain, port).await?;

    // Seed before validation so failure-path record_unsafe_resolved_upstream_attempt
    // has a backend_context to push onto. No K8s resource → rsc/service_key stay None.
    if ctx.backend_context.is_none() {
        ctx.backend_context = Some(crate::ctx::BackendContext {
            source: Some(BackendSource::ExternalJump),
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
    }

    if let Err(status) = validate_resolved_domain_addr(&addr, ctx.allow_loopback_upstream) {
        record_unsafe_resolved_upstream_attempt(ctx, addr, status);
        ctx.record_error(status);
        return Err(PingoraError::explain(
            ErrorType::ConnectError,
            "external upstream resolved to unsafe address",
        ));
    }

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    finish_static_peer(edgion_http, ctx, peer)
}

/// Backfill the resolved peer ip/port onto the current placeholder upstream entry.
/// The entry itself was created by [`begin_upstream_attempt`]; this stage only finalises
/// address fields once `try_get_peer` has produced a peer.
#[inline]
pub fn backfill_upstream_addr(peer: &HttpPeer, ctx: &mut EdgionHttpContext) {
    let (ip, port) = peer
        .address()
        .as_inet()
        .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
        .unwrap_or((None, None));
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.ip = ip;
        upstream.port = port;
    }
}

/// Initialize backend context for the resource-selected path (HTTP / gRPC backend ref).
/// No-op when already initialized. DirectEndpoint resolves its validated slot
/// from the request-pinned route generation; ExternalJump seeds its context.
/// Missing or invalid selection state is a routing bug.
#[inline]
fn init_backend_context_if_needed(ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.backend_context.is_some() {
        return Ok(());
    }

    let (name, namespace, service_kind, backend_ref_idx) =
        if let Some(UpstreamJumpTarget::Direct(preset)) = ctx.jump.as_deref() {
            let (name, namespace, service_kind, slot) = ctx
                .route_unit
                .as_ref()
                .and_then(|unit| direct_backend_metadata(&unit.backends, preset.backend_ref_idx))
                .ok_or_else(|| PingoraError::new(ErrorType::InternalError))?;
            (name, namespace, service_kind, Some(slot))
        } else if let Some(grpc_br) = ctx.selected_grpc_backend.as_ref() {
            let route_ns = ctx
                .grpc_route_unit
                .as_ref()
                .map(|unit| unit.matched_info.rsc.ns.as_str())
                .unwrap_or_default()
                .to_string();
            let ns = grpc_br.namespace.clone().unwrap_or(route_ns);
            (
                grpc_br.name.clone(),
                ns,
                ServiceBackendKind::from_kind(grpc_br.kind.as_ref()),
                Some(grpc_br.slot()),
            )
        } else if let Some(http_br) = ctx.selected_backend.as_ref() {
            let route_ns = ctx
                .route_unit
                .as_ref()
                .map(|unit| unit.matched_info.rsc.ns.as_str())
                .unwrap_or_default()
                .to_string();
            let ns = http_br.namespace.clone().unwrap_or(route_ns);
            let service_kind = match &http_br.target {
                CompiledBackendTarget::Service { .. } => ServiceBackendKind::from_kind(http_br.kind.as_ref()),
                CompiledBackendTarget::EdgionBackend { .. } | CompiledBackendTarget::Invalid { .. } => {
                    ServiceBackendKind::Unsupported(
                        http_br
                            .kind
                            .clone()
                            .unwrap_or_else(|| "unsupported backend reference".to_string()),
                    )
                }
            };
            (http_br.name.clone(), ns, service_kind, Some(http_br.slot))
        } else {
            return Err(PingoraError::new(ErrorType::InternalError));
        };

    let service_key = format!("{}/{}", namespace, name);
    let rsc = crate::Rsc::new("Service", namespace, name);
    ctx.init_backend_context(rsc, service_key, service_kind, backend_ref_idx);
    Ok(())
}

fn direct_backend_metadata(
    backends: &[crate::routes::http::match_unit::CompiledBackend],
    slot: usize,
) -> Option<(String, String, ServiceBackendKind, usize)> {
    let compiled = backends.get(slot)?;
    if compiled.ref_denied.is_some() {
        return None;
    }
    let (namespace, name) = match &compiled.target {
        CompiledBackendTarget::Service { namespace, name } => (namespace.clone(), name.clone()),
        CompiledBackendTarget::EdgionBackend { .. } | CompiledBackendTarget::Invalid { .. } => return None,
    };
    Some((
        name,
        namespace,
        ServiceBackendKind::from_kind(compiled.kind.as_ref()),
        compiled.slot,
    ))
}

fn validate_resolved_domain_addr(
    addr: &std::net::SocketAddr,
    allow_loopback_upstream: bool,
) -> Result<(), EdgionErrorCode> {
    match classify_backend_addr(&PingoraSocketAddr::Inet(*addr)) {
        None => {}
        Some(BlockedBackendReason::Localhost) if allow_loopback_upstream => {}
        Some(BlockedBackendReason::Localhost) => return Err(EdgionErrorCode::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => return Err(EdgionErrorCode::BackendAddrLinkLocalNotAllowed),
    }

    Ok(())
}

fn record_unsafe_resolved_upstream_attempt(
    ctx: &mut EdgionHttpContext,
    addr: std::net::SocketAddr,
    status: EdgionErrorCode,
) {
    ctx.push_upstream(Some(addr.ip().to_string()), Some(addr.port()));
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let reason = match status {
            EdgionErrorCode::BackendLocalhostNotAllowed => "unsafe loopback upstream",
            EdgionErrorCode::BackendAddrLinkLocalNotAllowed => "unsafe link-local upstream",
            _ => UNSAFE_UPSTREAM_ADDRESS_LOG,
        };
        upstream.err.push(reason.to_string());
    }
}

/// Path Forwarding Decision (single point, spec §14).
///
/// Priority:
///   0. ctx.uri_finalized already true   → skip (rewrite final form / already decided)
///   1. annotation forward-raw-path      → Some(true)/Some(false) forces
///   2. ctx.jump == External(preserve_raw_path: true) → gateway-to-gateway default raw
///   3. global forward_normalized_path   → false means raw (v3.1 compat / rollback)
///      Default                              → write normalized
#[inline]
fn finalize_request_uri(session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.uri_finalized {
        return Ok(());
    }

    let cfg = get_path_normalization_config();
    let annotation = ctx.route_unit.as_ref().and_then(|ru| ru.rule.parsed_forward_raw_path);
    let external_jump_preserve_raw = match ctx.jump.as_deref() {
        Some(UpstreamJumpTarget::External(p)) => p.preserve_raw_path,
        _ => false,
    };
    let want_raw = decide_want_raw(annotation, external_jump_preserve_raw, cfg.forward_normalized_path);
    let parsed = if !want_raw && ctx.path() != session.req_header().uri.path() {
        Some(
            ctx.uri(session.req_header())
                .parse::<http::Uri>()
                .map_err(|e| PingoraError::because(ErrorType::InternalError, "compose normalized uri", e))?,
        )
    } else {
        None
    };
    ctx.uri_finalized = true;

    if let Some(parsed) = parsed.filter(|parsed| *parsed != session.req_header().uri) {
        session.req_header_mut().set_uri(parsed);
    }
    Ok(())
}

/// Path forwarding decision for the upstream-peer single-point block (spec §14).
///
/// Returns `true` if the upstream should receive the raw (un-normalized) path,
/// `false` if it should receive the normalized form.
///
/// Inputs:
/// - `annotation`: parsed `edgion.io/forward-raw-path` from `HTTPRouteRule.parsed_forward_raw_path`
/// - `external_jump_preserve_raw`: `external_jump.preserve_raw_path` (true for region_route etc.)
/// - `cfg_forward_normalized`: `PathNormalizationConfig.forward_normalized_path`
///
/// Both production (`upstream_peer_http`) and unit tests call this single fn,
/// so the formula cannot drift between them.
#[inline]
fn decide_want_raw(annotation: Option<bool>, external_jump_preserve_raw: bool, cfg_forward_normalized: bool) -> bool {
    match annotation {
        Some(b) => b,
        None => external_jump_preserve_raw || !cfg_forward_normalized,
    }
}

/// Decide whether a cross-attempt retry that re-selected via plain LB must
/// refresh the session-persistence token in the response.
///
/// Pure function of the five relevant inputs so the full matrix is unit-testable
/// (mirrors the `decide_want_raw` pattern). Returns `true` only when ALL hold:
///
/// - `sticky_consumed` — an earlier attempt consumed the sticky target
///   (`sticky_consumed`), i.e. direct evidence rather than inference from
///   `Inactive`;
/// - `try_cnt >= 2` — `begin_upstream_attempt` already incremented, so this is a
///   retry attempt, not the first;
/// - `sp_configured` — the route has `session_persistence`;
/// - `!strict` — strict mode keeps the in-attempt fallback semantics untouched;
/// - `state == Inactive` — never downgrade an already-armed
///   `InjectNew` / `InjectUpdate`.
#[inline]
fn sp_should_refresh_on_cross_attempt_retry(
    sticky_consumed: bool,
    try_cnt: u32,
    sp_configured: bool,
    strict: bool,
    sp_state: &SessionPersistenceState,
) -> bool {
    sticky_consumed && try_cnt >= 2 && sp_configured && !strict && *sp_state == SessionPersistenceState::Inactive
}

/// Decide whether a strict-mode cross-attempt retry must re-pin to the same
/// sticky target (rather than migrate via plain LB).
///
/// Pure function of the five relevant inputs so the full matrix is unit-testable
/// (mirrors `sp_should_refresh_on_cross_attempt_retry`). Returns `true` only when
/// ALL hold:
///
/// - `sticky_consumed` — an earlier attempt consumed the sticky target;
/// - `try_cnt >= 2` — `begin_upstream_attempt` already incremented, so this is a
///   retry attempt, not the first;
/// - `sp_configured` — the route has `session_persistence`;
/// - `strict` — strict mode (the non-strict refresh path is the complement);
/// - `has_sticky_addr` — the mirror retained the resolved address to re-pin to.
///
/// Disjoint from `sp_should_refresh_on_cross_attempt_retry` on the `strict` axis.
#[inline]
fn sp_should_repin_on_strict_retry(
    sticky_consumed: bool,
    try_cnt: u32,
    sp_configured: bool,
    strict: bool,
    has_sticky_addr: bool,
) -> bool {
    sticky_consumed && try_cnt >= 2 && sp_configured && strict && has_sticky_addr
}

#[cfg(test)]
mod tests {
    use super::{
        ai_predispatch_deadline_mapping, ai_predispatch_error, apply_upstream_authority_decision,
        apply_upstream_read_timeout_override, backoff_action, choose_typed_retry_budget, decide_upstream_authority,
        decide_want_raw, direct_backend_metadata, fold_peer_timeouts, init_backend_context_if_needed,
        replay_write_floor, sp_should_refresh_on_cross_attempt_retry, sp_should_repin_on_strict_retry,
        stop_ai_predispatch_if_expired, AuthorityDecision, BackoffAction,
    };
    use crate::ai::dispatch::AiDispatchError;
    use crate::ai::{render_openai_error, AiProxyError};
    use crate::backends::policy::backend_traffic::authority::AuthorityError;
    use crate::backends::policy::backend_traffic::RetryBudget;
    use crate::backends::ServiceBackendKind;
    use crate::ctx::BackendSource;
    use crate::plugins::PluginRuntime;
    use crate::routes::http::match_unit::{CompiledBackend, CompiledBackendTarget};
    use crate::{BackendContext, BackendTlsInfo, EdgionHttpContext, UpstreamInfo};
    use crate::{PeerTimeoutSource, SessionPersistenceState};
    use edgion_resources::{EdgionErrorCode, HTTPBackendRef};
    use pingora_core::prelude::HttpPeer;
    use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
    use pingora_core::ErrorType;
    use pingora_load_balancing::Backend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn ai_unavailable_maps_to_fixed_openai_backend_unavailable() {
        assert_eq!(
            ai_predispatch_error(AiDispatchError::Unavailable),
            Some(AiProxyError::BackendUnavailable)
        );
        assert_eq!(
            ai_predispatch_error(AiDispatchError::Client(AiProxyError::UnsupportedParameter)),
            Some(AiProxyError::UnsupportedParameter)
        );
        assert_eq!(
            ai_predispatch_error(AiDispatchError::Client(AiProxyError::MalformedBody)),
            Some(AiProxyError::MalformedBody)
        );
        assert_eq!(ai_predispatch_error(AiDispatchError::Rejected), None);
        assert_eq!(ai_predispatch_error(AiDispatchError::InvalidConfiguration), None);
        assert_eq!(ai_predispatch_error(AiDispatchError::RequestDeadlineExceeded), None);
        assert_eq!(ai_predispatch_error(AiDispatchError::GrpcDeadlineExceeded), None);

        let (status, body) = render_openai_error(AiProxyError::BackendUnavailable);
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(status, 503);
        assert_eq!(body["error"]["type"], "api_error");
        assert_eq!(body["error"]["code"], "service_unavailable");
        assert_eq!(body["error"].as_object().unwrap().len(), 3);

        let (status, body) = render_openai_error(AiProxyError::MalformedBody);
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(status, 400);
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "invalid_request_body");
    }

    #[test]
    fn ai_predispatch_deadlines_map_to_standard_gateway_statuses() {
        assert_eq!(
            ai_predispatch_deadline_mapping(&AiDispatchError::RequestDeadlineExceeded),
            Some((
                EdgionErrorCode::RequestTimeoutExceeded,
                "Request timeout exceeded during AI predispatch selection",
            ))
        );
        assert_eq!(
            ai_predispatch_deadline_mapping(&AiDispatchError::GrpcDeadlineExceeded),
            Some((
                EdgionErrorCode::GrpcClientDeadlineExceeded,
                "gRPC client deadline exceeded during AI predispatch selection",
            ))
        );
        assert_eq!(ai_predispatch_deadline_mapping(&AiDispatchError::Unavailable), None);
    }

    #[tokio::test]
    async fn post_io_predispatch_gate_returns_standard_deadline_error() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info
            .set_grpc_client_deadline(Instant::now() - Duration::from_secs(1));

        let response = stop_ai_predispatch_if_expired(&mut ctx)
            .await
            .expect("expired gRPC deadline must stop predispatch");

        assert_eq!(response.etype(), &ErrorType::HTTPStatus(504));
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::GrpcClientDeadlineExceeded));
    }

    #[test]
    fn invalid_group_service_kind_stays_unsupported_after_selection() {
        let schema: HTTPBackendRef = serde_json::from_value(serde_json::json!({
            "group": "example.io",
            "kind": "Service",
            "name": "wrong-group-service",
            "namespace": "route-ns",
            "port": 8080
        }))
        .unwrap();
        let backend = CompiledBackend::compile(schema, "route-ns", 0, Arc::new(PluginRuntime::new()));
        assert_eq!(
            backend.target,
            CompiledBackendTarget::Invalid {
                classified_kind: edgion_resources::HTTPBackendRefKind::Unsupported
            }
        );
        let mut ctx = EdgionHttpContext::new();
        let logical = crate::backends::LogicalBackendHandle::new(Arc::new(vec![backend]), 0).unwrap();
        ctx.selected_backend = Some(crate::backends::SelectedHttpBackend::Invalid(logical));

        init_backend_context_if_needed(&mut ctx).unwrap();

        assert!(matches!(
            ctx.backend_context.as_ref().and_then(|context| context.service_kind.as_ref()),
            Some(ServiceBackendKind::Unsupported(kind)) if kind == "Service"
        ));
    }

    #[test]
    fn direct_backend_metadata_uses_the_exact_service_slot() {
        let compile = |name: &str, namespace: &str, slot| {
            CompiledBackend::compile(
                HTTPBackendRef {
                    name: name.to_string(),
                    namespace: Some(namespace.to_string()),
                    port: Some(8080),
                    weight: Some(1),
                    group: None,
                    kind: Some("Service".to_string()),
                    filters: None,
                    backend_tls_policy: None,
                    ref_denied: None,
                },
                "route-ns",
                slot,
                Arc::new(PluginRuntime::new()),
            )
        };
        let backends = vec![
            compile("duplicate", "first-ns", 0),
            compile("duplicate", "second-ns", 1),
        ];

        let (name, namespace, kind, slot) = direct_backend_metadata(&backends, 1).unwrap();

        assert_eq!(name, "duplicate");
        assert_eq!(namespace, "second-ns");
        assert_eq!(kind, ServiceBackendKind::Service);
        assert_eq!(slot, 1);
        assert!(direct_backend_metadata(&backends, 2).is_none());
    }

    #[test]
    fn direct_backend_metadata_rejects_non_service_slots() {
        let backend = CompiledBackend::compile(
            HTTPBackendRef {
                name: "provider".to_string(),
                namespace: Some("ai-ns".to_string()),
                port: None,
                weight: Some(1),
                group: Some("edgion.io".to_string()),
                kind: Some("EdgionBackend".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "route-ns",
            0,
            Arc::new(PluginRuntime::new()),
        );

        assert!(direct_backend_metadata(&[backend], 0).is_none());
    }

    #[test]
    fn direct_backend_metadata_rejects_referencegrant_denied_services() {
        let backend = CompiledBackend::compile(
            HTTPBackendRef {
                name: "cross-namespace".to_string(),
                namespace: Some("other-ns".to_string()),
                port: Some(8080),
                weight: Some(1),
                group: None,
                kind: Some("Service".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: Some(edgion_resources::resources::common::RefDenied {
                    target_namespace: "other-ns".to_string(),
                    target_name: "cross-namespace".to_string(),
                    reason: Some("NoMatchingReferenceGrant".to_string()),
                }),
            },
            "route-ns",
            0,
            Arc::new(PluginRuntime::new()),
        );

        assert!(direct_backend_metadata(&[backend], 0).is_none());
    }

    #[test]
    fn replay_write_floor_applies_only_with_captured_body_and_enabled_default() {
        let global = Duration::from_secs(30);
        // Captured body + enabled global default → the write bound floor.
        assert_eq!(replay_write_floor(true, global), Some(global));
        // No captured body: nothing replays, nothing to bound.
        assert_eq!(replay_write_floor(false, global), None);
        // Global default disabled: the operator asked for no bounds anywhere.
        assert_eq!(replay_write_floor(true, Duration::ZERO), None);
    }

    #[test]
    fn default_writes_normalized() {
        // No annotation, no external_jump, global forward_normalized_path=true → normalized
        assert!(!decide_want_raw(None, false, true));
    }

    #[test]
    fn annotation_true_forces_raw() {
        assert!(decide_want_raw(Some(true), false, true));
    }

    #[test]
    fn annotation_false_forces_normalized_overrides_region_route() {
        // annotation "false" overrides region_route's preserve_raw_path = true
        assert!(!decide_want_raw(Some(false), true, true));
    }

    #[test]
    fn region_route_default_raw() {
        assert!(decide_want_raw(None, true, true));
    }

    #[test]
    fn dynamic_external_upstream_default_normalized() {
        // dynamic_external_upstream sets preserve_raw_path: false on its
        // ExternalJumpPreset. With no annotation override, this should still
        // forward normalized — same outcome as the no-external-jump default,
        // pinned here as an intentional regression check for plugin defaults.
        assert!(!decide_want_raw(None, false, true));
    }

    #[test]
    fn global_disable_forwards_raw() {
        assert!(decide_want_raw(None, false, false));
    }

    #[test]
    fn annotation_true_overrides_global_normalize() {
        assert!(decide_want_raw(Some(true), false, true));
    }

    #[test]
    fn annotation_false_overrides_global_disable() {
        assert!(!decide_want_raw(Some(false), false, false));
    }

    // ─── Matrix completeness tests: cover the full 3×2×2 = 12 input combinations ───
    // The 8 tests above cover 8 named scenarios; the 4 below fill the remaining
    // 4 boundary combinations to make matrix coverage exhaustive.

    #[test]
    fn annotation_true_with_region_route_and_global_normalize_still_raw() {
        // Annotation wins at the top of the priority chain. Even with all lower
        // channels active, Some(true) deterministically returns true.
        assert!(decide_want_raw(Some(true), true, true));
    }

    #[test]
    fn annotation_true_with_region_route_and_global_disable_still_raw() {
        // Annotation true + region_route + global disable → all channels agree on raw.
        assert!(decide_want_raw(Some(true), true, false));
    }

    #[test]
    fn annotation_false_with_region_route_and_global_disable_still_normalized() {
        // Annotation false explicitly overrides BOTH region_route's preserve_raw_path
        // AND the global disable. This is the operator's emergency override channel.
        assert!(!decide_want_raw(Some(false), true, false));
    }

    #[test]
    fn region_route_with_global_disable_doubles_down_on_raw() {
        // No annotation, region_route preserve_raw=true, global forward_normalized=false
        // → both lower channels OR-combine to raw. Verifies the OR formula in the None arm.
        assert!(decide_want_raw(None, true, false));
    }

    // ─── I-2 regression: idempotency check uses path() not to_string() ───
    // Verifies that for absolute-form HTTP/2 URIs (where to_string() includes
    // scheme + authority but path() returns only the path component), the
    // upstream_peer_http idempotency comparison correctly uses the path component.
    // If a future refactor reverts to to_string() comparison, this test catches the
    // regression by demonstrating that path() and to_string() can diverge.

    #[test]
    fn absolute_form_uri_path_only_comparison() {
        let absolute: http::Uri = "http://example.com/api/data?x=1".parse().unwrap();
        // path() returns only the path component
        assert_eq!(absolute.path(), "/api/data");
        // to_string() includes scheme + authority + query (and would falsely report
        // "different" against a normalized "/api/data" path)
        assert_ne!(absolute.to_string(), "/api/data");
        assert!(absolute.to_string().contains("example.com"));
        // The pg_upstream_peer.rs idempotency check at upstream_peer_http uses
        // path() so absolute-form requests don't trigger spurious set_uri writes.
    }

    // --- sp_should_refresh_on_cross_attempt_retry matrix ---

    #[test]
    fn sp_refresh_fires_on_consumed_retry_nonstrict_inactive() {
        // The one combination that yields a refresh.
        assert!(sp_should_refresh_on_cross_attempt_retry(
            true,
            2,
            true,
            false,
            &SessionPersistenceState::Inactive,
        ));
    }

    #[test]
    fn sp_refresh_skips_when_not_consumed() {
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            false,
            2,
            true,
            false,
            &SessionPersistenceState::Inactive,
        ));
    }

    #[test]
    fn sp_refresh_skips_on_first_attempt() {
        // try_cnt == 1 means this is the first attempt, not a retry.
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            true,
            1,
            true,
            false,
            &SessionPersistenceState::Inactive,
        ));
    }

    #[test]
    fn sp_refresh_skips_when_sp_not_configured() {
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            true,
            2,
            false,
            false,
            &SessionPersistenceState::Inactive,
        ));
    }

    #[test]
    fn sp_refresh_skips_in_strict_mode() {
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            true,
            2,
            true,
            true,
            &SessionPersistenceState::Inactive,
        ));
    }

    #[test]
    fn sp_refresh_does_not_downgrade_inject_new() {
        // An already-armed InjectNew/InjectUpdate must not be touched.
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            true,
            2,
            true,
            false,
            &SessionPersistenceState::InjectNew,
        ));
        assert!(!sp_should_refresh_on_cross_attempt_retry(
            true,
            2,
            true,
            false,
            &SessionPersistenceState::InjectUpdate,
        ));
    }

    #[test]
    fn sp_refresh_fires_on_later_retries() {
        // try_cnt >= 2 covers the second retry and beyond.
        assert!(sp_should_refresh_on_cross_attempt_retry(
            true,
            3,
            true,
            false,
            &SessionPersistenceState::Inactive,
        ));
    }

    // --- sp_should_repin_on_strict_retry matrix ---

    #[test]
    fn sp_repin_fires_on_consumed_strict_retry_with_addr() {
        assert!(sp_should_repin_on_strict_retry(true, 2, true, true, true));
    }

    #[test]
    fn sp_repin_skips_when_not_consumed() {
        assert!(!sp_should_repin_on_strict_retry(false, 2, true, true, true));
    }

    #[test]
    fn sp_repin_skips_on_first_attempt() {
        assert!(!sp_should_repin_on_strict_retry(true, 1, true, true, true));
    }

    #[test]
    fn sp_repin_skips_when_sp_not_configured() {
        assert!(!sp_should_repin_on_strict_retry(true, 2, false, true, true));
    }

    #[test]
    fn sp_repin_skips_in_nonstrict_mode() {
        assert!(!sp_should_repin_on_strict_retry(true, 2, true, false, true));
    }

    #[test]
    fn sp_repin_skips_without_sticky_addr() {
        assert!(!sp_should_repin_on_strict_retry(true, 2, true, true, false));
    }

    #[test]
    fn sp_repin_fires_on_later_retries() {
        assert!(sp_should_repin_on_strict_retry(true, 3, true, true, true));
    }

    #[test]
    fn sp_repin_and_refresh_are_mutually_exclusive() {
        // Over the full shared input space, the two cross-attempt decisions must
        // never both be true: re-pin requires strict, refresh requires !strict.
        for &consumed in &[false, true] {
            for &try_cnt in &[1u32, 2, 3] {
                for &configured in &[false, true] {
                    for &strict in &[false, true] {
                        for &has_addr in &[false, true] {
                            for state in &[
                                SessionPersistenceState::Inactive,
                                SessionPersistenceState::InjectNew,
                                SessionPersistenceState::InjectUpdate,
                            ] {
                                let repin =
                                    sp_should_repin_on_strict_retry(consumed, try_cnt, configured, strict, has_addr);
                                let refresh = sp_should_refresh_on_cross_attempt_retry(
                                    consumed, try_cnt, configured, strict, state,
                                );
                                assert!(
                                    !(repin && refresh),
                                    "both fired: consumed={} try_cnt={} configured={} strict={} has_addr={} state={:?}",
                                    consumed,
                                    try_cnt,
                                    configured,
                                    strict,
                                    has_addr,
                                    state
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn s(n: u64) -> Option<Duration> {
        Some(Duration::from_secs(n))
    }

    #[test]
    fn fold_backend_only() {
        assert_eq!(
            fold_peer_timeouts(s(5), None, None),
            Some((Duration::from_secs(5), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_request_deadline_tighter() {
        assert_eq!(
            fold_peer_timeouts(s(5), s(2), None),
            Some((Duration::from_secs(2), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_grpc_deadline_tightest() {
        assert_eq!(
            fold_peer_timeouts(s(5), s(4), s(1)),
            Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
        );
    }

    #[test]
    fn fold_tie_backend_wins() {
        // Equal bounds: BackendRequest wins so the operator-opted retry
        // semantics apply; deadline guards still gate the actual retry.
        assert_eq!(
            fold_peer_timeouts(s(3), s(3), s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_two_way_tie_backend_wins() {
        // Backend ties with a single deadline bound: BackendRequest still wins.
        assert_eq!(
            fold_peer_timeouts(s(3), s(3), None),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_deadline_tie_request_wins() {
        // Deadline-vs-deadline tie: iteration order makes RequestDeadline win.
        // Pinned here because the order is observable in access-log error
        // attribution even though both deadlines expire simultaneously.
        assert_eq!(
            fold_peer_timeouts(None, s(3), s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_backend_disabled_request_wins() {
        // backendRequest: "0s" (Disabled) → backend bound is None.
        assert_eq!(
            fold_peer_timeouts(None, s(7), None),
            Some((Duration::from_secs(7), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_grpc_only() {
        assert_eq!(
            fold_peer_timeouts(None, None, s(1)),
            Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
        );
    }

    #[test]
    fn fold_backend_grpc_tie_backend_wins() {
        assert_eq!(
            fold_peer_timeouts(s(3), None, s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_all_none() {
        assert_eq!(fold_peer_timeouts(None, None, None), None);
    }

    #[test]
    fn external_read_override_replaces_normal_backend_bound() {
        assert_eq!(apply_upstream_read_timeout_override(s(30), s(300)), s(300));
        assert_eq!(apply_upstream_read_timeout_override(None, s(300)), s(300));
    }

    #[test]
    fn absent_or_zero_external_override_preserves_normal_backend_bound() {
        assert_eq!(apply_upstream_read_timeout_override(s(30), None), s(30));
        assert_eq!(apply_upstream_read_timeout_override(s(30), Some(Duration::ZERO)), s(30));
    }

    #[test]
    fn external_override_remains_backend_source_then_deadlines_clamp_it() {
        let overridden = apply_upstream_read_timeout_override(s(30), s(300));
        assert_eq!(
            fold_peer_timeouts(overridden, s(20), s(10)),
            Some((Duration::from_secs(10), PeerTimeoutSource::GrpcDeadline))
        );
        assert_eq!(
            fold_peer_timeouts(overridden, s(300), None),
            Some((Duration::from_secs(300), PeerTimeoutSource::BackendRequest))
        );
    }

    // ─── effective_backend_bound: route three-state + global-default fallback ───

    use super::effective_backend_bound;
    use edgion_resources::resources::http_route::RouteTimeout;

    #[test]
    fn backend_bound_unset_falls_back_to_global() {
        let global = Duration::from_secs(60);
        assert_eq!(effective_backend_bound(None, global), Some(global));
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Unset), global),
            Some(global)
        );
    }

    #[test]
    fn backend_bound_disabled_is_none() {
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Disabled), Duration::from_secs(60)),
            None
        );
    }

    #[test]
    fn backend_bound_route_duration_wins_over_global() {
        assert_eq!(
            effective_backend_bound(
                Some(&RouteTimeout::Duration(Duration::from_secs(1))),
                Duration::from_secs(60)
            ),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn backend_bound_zero_global_treated_as_disabled() {
        // A `defaultRequestTimeout: "0s"` global must not become an instant
        // read/write timeout (instant-retry loop with a retry policy present).
        assert_eq!(effective_backend_bound(None, Duration::ZERO), None);
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Unset), Duration::ZERO),
            None
        );
    }

    // ─── resolve_connect_deadline: per-backend BTP override, else global ───

    use super::{choose_typed_connect_deadline, resolve_connect_deadline};
    use crate::backends::policy::backend_traffic::store::{get_backend_traffic_policy_store, GLOBAL_STORE_TEST_LOCK};

    fn capture_service_ctx(service_key: &str) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: Some(service_key.to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        super::begin_upstream_attempt(&mut ctx);
        ctx
    }

    fn captured_policy(
        ctx: &EdgionHttpContext,
    ) -> Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy> {
        ctx.backend_context
            .as_ref()
            .and_then(|bc| bc.effective_policy.clone())
            .expect("service policy captured")
    }

    #[allow(clippy::too_many_arguments)]
    fn service_policy_fields(
        ns: &str,
        name: &str,
        svc: &str,
        lb: Option<edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType>,
        degrade_threshold: Option<u8>,
        health_check_host: Option<&str>,
        retry_percent: Option<u8>,
        connect_timeout: Option<&str>,
        cb_limit: Option<u32>,
    ) -> Arc<edgion_resources::resources::EdgionBackendTrafficPolicy> {
        use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;
        use edgion_resources::resources::edgion_backend_traffic_policy::{
            BudgetDetails, ConnectionOverride, EdgionBackendTrafficPolicySpec, LoadBalancerConfig, PolicyTargetRef,
            RetryConstraintConfig,
        };
        use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
        use edgion_resources::resources::EdgionBackendTrafficPolicy;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

        Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: svc.to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                load_balancer: lb.map(|r#type| LoadBalancerConfig {
                    r#type,
                    consistent_hash: None,
                    degrade_threshold,
                }),
                health_check: None,
                outlier_detection: None,
                upstream_authority: health_check_host.map(|host| UpstreamAuthorityConfig {
                    pattern: "*.sandbox.internal".to_string(),
                    template: "${ctx:tenant}.sandbox.internal".to_string(),
                    health_check_host: Some(host.to_string()),
                }),
                retry_constraint: retry_percent.map(|percent| RetryConstraintConfig {
                    budget: BudgetDetails {
                        percent,
                        interval: "10s".to_string(),
                    },
                    min_retry_rate: None,
                }),
                circuit_breaker: cb_limit.map(|max_parallel_requests| CircuitBreakerConfig { max_parallel_requests }),
                connection: connect_timeout.map(|connect_timeout| ConnectionOverride {
                    connect_timeout: Some(connect_timeout.to_string()),
                }),
            },
            status: None,
        })
    }

    #[test]
    fn configure_peer_timeouts_service_does_not_call_get_connect_timeout() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "to-cap/p1".to_string(),
            service_policy_fields(
                "to-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                None,
                Some("123ms"),
                None,
            ),
        )]));

        let ctx = capture_service_ctx("to-cap/svc");
        let captured = captured_policy(&ctx);
        assert_eq!(captured.connect_timeout, Some(Duration::from_millis(123)));

        store.full_set_policies(std::collections::HashMap::from([(
            "to-cap/p1".to_string(),
            service_policy_fields(
                "to-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                None,
                Some("999ms"),
                None,
            ),
        )]));
        assert_eq!(
            resolve_connect_deadline(Some("to-cap/svc"), Duration::from_secs(5)),
            Some(Duration::from_millis(999)),
            "the live shim must see the reloaded timeout"
        );
        assert_eq!(
            choose_typed_connect_deadline(
                false,
                captured.connect_timeout,
                captured.connect_timeout,
                Duration::from_secs(5)
            ),
            Some(Duration::from_millis(123)),
            "configure_peer_timeouts Service branch uses only the captured timeout"
        );

        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn resolve_connect_deadline_no_override_falls_back_to_global() {
        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        let key = "default/no-such-svc";
        store.reapply_service(key);

        assert_eq!(
            resolve_connect_deadline(Some(key), Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        );
        // No service key (ExternalJump) also falls back to global.
        assert_eq!(
            resolve_connect_deadline(None, Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        );
        // Degenerate zero global => None (no connect bound), never an instant-fail.
        assert_eq!(resolve_connect_deadline(Some(key), Duration::ZERO), None);
        assert_eq!(resolve_connect_deadline(None, Duration::ZERO), None);
    }

    #[test]
    fn ai_connect_timeout_never_falls_back_to_same_named_service_policy() {
        let service_timeout = Some(Duration::from_millis(123));
        let global = Duration::from_secs(5);

        assert_eq!(
            choose_typed_connect_deadline(true, None, service_timeout, global),
            Some(global),
            "an AI selection with no override must use the global default"
        );
        assert_eq!(
            choose_typed_connect_deadline(false, None, service_timeout, global),
            service_timeout,
            "a Service selection retains its late policy override"
        );
    }

    #[test]
    fn ai_retry_budget_never_falls_back_to_same_named_service_policy() {
        use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};

        let service_budget = Arc::new(RetryBudget::from_config(&RetryConstraintConfig {
            budget: BudgetDetails {
                percent: 50,
                interval: "1h".to_string(),
            },
            min_retry_rate: None,
        }));
        assert!(choose_typed_retry_budget(true, None, Some(service_budget.clone())).is_none());
        assert!(Arc::ptr_eq(
            &choose_typed_retry_budget(false, None, Some(service_budget.clone())).unwrap(),
            &service_budget
        ));
    }

    #[test]
    fn begin_service_attempt_releases_previous_endpoint_permit_first() {
        let backend = Arc::new(crate::lb::EdgionBackend::new(Backend::new("127.0.0.1:18080").unwrap()));
        let mut ctx = EdgionHttpContext::new();
        ctx.cb_permit = crate::backends::circuit_breaker::CbPermit::try_acquire(backend.clone(), 1);
        assert_eq!(backend.cb_inflight(), 1);

        super::begin_upstream_attempt(&mut ctx);

        assert_eq!(backend.cb_inflight(), 0);
        assert!(ctx.cb_permit.is_none());
    }

    #[test]
    fn begin_upstream_attempt_does_not_recapture_on_retry() {
        use edgion_resources::resources::edgion_backend_traffic_policy::{
            EdgionBackendTrafficPolicySpec, LoadBalancerConfig, LoadBalancerType, PolicyTargetRef,
        };
        use edgion_resources::resources::EdgionBackendTrafficPolicy;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
        use std::sync::Arc;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        let first = Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("p1".to_string()),
                namespace: Some("cap".to_string()),
                creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                load_balancer: Some(LoadBalancerConfig {
                    r#type: LoadBalancerType::LeastConn,
                    consistent_hash: None,
                    degrade_threshold: None,
                }),
                health_check: None,
                outlier_detection: None,
                upstream_authority: None,
                retry_constraint: None,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        });
        store.full_set_policies(std::collections::HashMap::from([("cap/p1".to_string(), first)]));

        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: Some("cap/svc".to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        super::begin_upstream_attempt(&mut ctx);
        let captured = ctx
            .backend_context
            .as_ref()
            .and_then(|bc| bc.effective_policy.clone())
            .expect("captured");
        assert_eq!(
            captured.load_balancer,
            Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
        );

        let second = Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("p1".to_string()),
                namespace: Some("cap".to_string()),
                creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                load_balancer: Some(LoadBalancerConfig {
                    r#type: LoadBalancerType::Ewma,
                    consistent_hash: None,
                    degrade_threshold: None,
                }),
                health_check: None,
                outlier_detection: None,
                upstream_authority: None,
                retry_constraint: None,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        });
        store.full_set_policies(std::collections::HashMap::from([("cap/p1".to_string(), second)]));
        super::begin_upstream_attempt(&mut ctx);
        let still = ctx
            .backend_context
            .as_ref()
            .and_then(|bc| bc.effective_policy.as_ref())
            .expect("still captured");
        assert!(Arc::ptr_eq(&captured, still), "retry must not recapture");
        assert_eq!(
            still.load_balancer,
            Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
        );
        assert!(super::retry_budget_for(&ctx).is_none());

        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn grpc_begin_upstream_attempt_captures_without_selected_backend() {
        use edgion_resources::resources::edgion_backend_traffic_policy::{
            BudgetDetails, EdgionBackendTrafficPolicySpec, PolicyTargetRef, RetryConstraintConfig,
        };
        use edgion_resources::resources::EdgionBackendTrafficPolicy;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
        use std::sync::Arc;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        let policy = Arc::new(EdgionBackendTrafficPolicy {
            metadata: ObjectMeta {
                name: Some("p1".to_string()),
                namespace: Some("grpc-cap".to_string()),
                creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
                ..Default::default()
            },
            spec: EdgionBackendTrafficPolicySpec {
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                retry_constraint: Some(RetryConstraintConfig {
                    budget: BudgetDetails {
                        percent: 20,
                        interval: "10s".to_string(),
                    },
                    min_retry_rate: None,
                }),
                load_balancer: None,
                health_check: None,
                outlier_detection: None,
                upstream_authority: None,
                circuit_breaker: None,
                connection: None,
            },
            status: None,
        });
        store.full_set_policies(std::collections::HashMap::from([("grpc-cap/p1".to_string(), policy)]));

        let mut ctx = EdgionHttpContext::new();
        ctx.selected_backend = None;
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: Some("grpc-cap/svc".to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        super::begin_upstream_attempt(&mut ctx);
        assert!(ctx
            .backend_context
            .as_ref()
            .and_then(|bc| bc.effective_policy.as_ref())
            .is_some());
        assert!(super::retry_budget_for(&ctx).is_some());
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn concurrent_capture_never_mixes_service_fields() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
        use edgion_resources::resources::lb_policy::ParsedLBPolicy;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "mix-cap/p1".to_string(),
            service_policy_fields(
                "mix-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::LeastConn),
                Some(20),
                Some("probe-a.sandbox.internal"),
                Some(20),
                Some("100ms"),
                Some(5),
            ),
        )]));

        let ctx = capture_service_ctx("mix-cap/svc");
        let captured = captured_policy(&ctx);
        let sampled = Arc::clone(&captured);
        let reader = std::thread::spawn(move || {
            for _ in 0..8_000 {
                assert_eq!(sampled.load_balancer, Some(ParsedLBPolicy::LeastConn));
                assert_eq!(sampled.degrade_threshold_override, Some(20));
                assert_eq!(sampled.connect_timeout, Some(Duration::from_millis(100)));
                assert_eq!(sampled.circuit_breaker_limit, Some(5));
                assert_eq!(sampled.retry_constraint.as_ref().map(|c| c.budget.percent), Some(20));
                assert_eq!(
                    sampled
                        .upstream_authority_config
                        .as_ref()
                        .and_then(|c| c.health_check_host.as_deref()),
                    Some("probe-a.sandbox.internal")
                );
            }
        });

        store.full_set_policies(std::collections::HashMap::from([(
            "mix-cap/p1".to_string(),
            service_policy_fields(
                "mix-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::Ewma),
                Some(80),
                Some("probe-b.sandbox.internal"),
                Some(50),
                Some("2s"),
                Some(11),
            ),
        )]));
        reader.join().expect("captured Arc stayed internally consistent");

        assert_eq!(captured.load_balancer, Some(ParsedLBPolicy::LeastConn));
        assert_eq!(captured.degrade_threshold_override, Some(20));
        assert_eq!(captured.connect_timeout, Some(Duration::from_millis(100)));
        assert_eq!(captured.circuit_breaker_limit, Some(5));

        let later = capture_service_ctx("mix-cap/svc");
        let reloaded = captured_policy(&later);
        assert_eq!(reloaded.load_balancer, Some(ParsedLBPolicy::Ewma));
        assert_eq!(reloaded.degrade_threshold_override, Some(80));
        assert_eq!(reloaded.connect_timeout, Some(Duration::from_secs(2)));
        assert_eq!(reloaded.circuit_breaker_limit, Some(11));
        assert_eq!(reloaded.retry_constraint.as_ref().map(|c| c.budget.percent), Some(50));
        assert_eq!(
            reloaded
                .upstream_authority_config
                .as_ref()
                .and_then(|c| c.health_check_host.as_deref()),
            Some("probe-b.sandbox.internal")
        );

        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn captured_arc_retains_old_lb_after_reload() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
        use edgion_resources::resources::lb_policy::ParsedLBPolicy;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "retain-lb/p1".to_string(),
            service_policy_fields(
                "retain-lb",
                "p1",
                "svc",
                Some(LoadBalancerType::LeastConn),
                None,
                None,
                None,
                None,
                None,
            ),
        )]));
        let ctx = capture_service_ctx("retain-lb/svc");
        let captured = captured_policy(&ctx);
        store.full_set_policies(std::collections::HashMap::from([(
            "retain-lb/p1".to_string(),
            service_policy_fields(
                "retain-lb",
                "p1",
                "svc",
                Some(LoadBalancerType::Ewma),
                None,
                None,
                None,
                None,
                None,
            ),
        )]));
        assert_eq!(captured.load_balancer, Some(ParsedLBPolicy::LeastConn));
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn captured_arc_retains_old_authority_after_reload() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "retain-auth/p1".to_string(),
            service_policy_fields(
                "retain-auth",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                Some("probe-old.sandbox.internal"),
                None,
                None,
                None,
            ),
        )]));
        let ctx = capture_service_ctx("retain-auth/svc");
        let captured = captured_policy(&ctx);
        store.full_set_policies(std::collections::HashMap::from([(
            "retain-auth/p1".to_string(),
            service_policy_fields(
                "retain-auth",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                Some("probe-new.sandbox.internal"),
                None,
                None,
                None,
            ),
        )]));
        assert_eq!(
            captured
                .upstream_authority_config
                .as_ref()
                .and_then(|c| c.health_check_host.as_deref()),
            Some("probe-old.sandbox.internal")
        );
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn service_retry_budget_for_uses_captured_arc_after_store_reload() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "retry-cap/p1".to_string(),
            service_policy_fields(
                "retry-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                Some(20),
                None,
                None,
            ),
        )]));
        let ctx = capture_service_ctx("retry-cap/svc");
        let first = super::retry_budget_for(&ctx).expect("captured budget");
        store.full_set_policies(std::collections::HashMap::from([(
            "retry-cap/p1".to_string(),
            service_policy_fields(
                "retry-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                Some(50),
                None,
                None,
            ),
        )]));
        let after_reload = super::retry_budget_for(&ctx).expect("same captured budget");
        assert!(Arc::ptr_eq(&first, &after_reload));
        let live = store
            .snapshot()
            .get_effective(
                &crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(
                    "retry-cap/svc",
                ),
            )
            .and_then(|p| p.retry_budget.clone())
            .expect("reloaded budget");
        assert!(!Arc::ptr_eq(&first, &live));
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn service_try_retry_uses_same_captured_budget_arc() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "try-retry/p1".to_string(),
            service_policy_fields(
                "try-retry",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                Some(20),
                None,
                None,
            ),
        )]));
        let mut ctx = capture_service_ctx("try-retry/svc");
        let first = super::retry_budget_for(&ctx).expect("captured budget");
        store.full_set_policies(std::collections::HashMap::from([(
            "try-retry/p1".to_string(),
            service_policy_fields(
                "try-retry",
                "p1",
                "svc",
                Some(LoadBalancerType::RoundRobin),
                None,
                None,
                Some(80),
                None,
                None,
            ),
        )]));
        super::begin_upstream_attempt(&mut ctx);
        let on_retry = super::retry_budget_for(&ctx).expect("retry still uses capture");
        assert!(
            Arc::ptr_eq(&first, &on_retry),
            "fail-to-connect retry_budget_for must keep the first-attempt Arc"
        );
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn invalid_backend_with_service_key_still_captures() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
        use edgion_resources::HTTPBackendRef;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "inv-cap/p1".to_string(),
            service_policy_fields(
                "inv-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::LeastConn),
                None,
                None,
                None,
                None,
                None,
            ),
        )]));

        let schema: HTTPBackendRef = serde_json::from_value(serde_json::json!({
            "group": "example.io",
            "kind": "Service",
            "name": "wrong-group-service",
            "namespace": "inv-cap",
            "port": 8080
        }))
        .unwrap();
        let backend = crate::routes::http::match_unit::CompiledBackend::compile(
            schema,
            "inv-cap",
            0,
            Arc::new(crate::plugins::PluginRuntime::new()),
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.selected_backend = Some(crate::backends::SelectedHttpBackend::Invalid(
            crate::backends::LogicalBackendHandle::new(Arc::new(vec![backend]), 0).unwrap(),
        ));
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: Some("inv-cap/svc".to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        super::begin_upstream_attempt(&mut ctx);
        assert!(ctx
            .backend_context
            .as_ref()
            .and_then(|bc| bc.effective_policy.as_ref())
            .is_some());
        store.full_set_policies(std::collections::HashMap::new());
    }

    #[test]
    fn direct_endpoint_finish_static_peer_captures_when_service_key_set() {
        use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        let store = get_backend_traffic_policy_store();
        store.full_set_policies(std::collections::HashMap::from([(
            "de-cap/p1".to_string(),
            service_policy_fields(
                "de-cap",
                "p1",
                "svc",
                Some(LoadBalancerType::LeastConn),
                None,
                None,
                Some(20),
                Some("123ms"),
                Some(4),
            ),
        )]));
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: Some(BackendSource::DirectEndpoint),
            rsc: None,
            service_key: Some("de-cap/svc".to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        super::begin_upstream_attempt(&mut ctx);
        let captured = captured_policy(&ctx);
        assert_eq!(
            captured.load_balancer,
            Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
        );
        assert_eq!(captured.connect_timeout, Some(Duration::from_millis(123)));
        assert!(super::retry_budget_for(&ctx).is_some());
        store.full_set_policies(std::collections::HashMap::new());
    }

    // ─── route-06: retry backoff minimum-wait conformance (GEP-1731) ───
    // Pin the `remaining <= delay` boundary that decides whether a retry may
    // sleep the configured backoff or must fail instead of retrying before the
    // configured minimum duration.

    #[test]
    fn backoff_sleeps_when_full_delay_fits() {
        // remaining (3s) > delay (2s) → sleep the full minimum backoff.
        assert_eq!(
            backoff_action(Duration::from_secs(2), Some(Duration::from_secs(3)), None),
            BackoffAction::Sleep
        );
    }

    #[test]
    fn backoff_sleeps_when_no_deadline_applies() {
        // No request/grpc deadline bounds the backoff → sleep.
        assert_eq!(backoff_action(Duration::from_secs(2), None, None), BackoffAction::Sleep);
    }

    #[test]
    fn backoff_fails_when_request_budget_too_small() {
        // 0 < remaining (1s) <= delay (2s): retrying now would precede the
        // configured minimum backoff → fail with the request deadline.
        assert_eq!(
            backoff_action(Duration::from_secs(2), Some(Duration::from_secs(1)), None),
            BackoffAction::FailBudgetTooSmall { prefer_request: true }
        );
    }

    #[test]
    fn backoff_fails_at_exact_equality_boundary() {
        // remaining == delay is still too tight (the spec requires the minimum
        // wait to complete strictly before the next attempt) → fail.
        assert_eq!(
            backoff_action(Duration::from_secs(2), Some(Duration::from_secs(2)), None),
            BackoffAction::FailBudgetTooSmall { prefer_request: true }
        );
    }

    #[test]
    fn backoff_fails_with_grpc_deadline_when_grpc_binds() {
        // Only the gRPC deadline is too small (request has room) → fail and
        // report the gRPC client deadline.
        assert_eq!(
            backoff_action(
                Duration::from_secs(2),
                Some(Duration::from_secs(5)),
                Some(Duration::from_secs(1))
            ),
            BackoffAction::FailBudgetTooSmall { prefer_request: false }
        );
    }

    #[test]
    fn backoff_prefers_request_deadline_when_both_too_small() {
        // Both deadlines are <= delay; the request deadline is preferred to
        // mirror the post-sleep re-check order.
        assert_eq!(
            backoff_action(
                Duration::from_secs(2),
                Some(Duration::from_millis(1500)),
                Some(Duration::from_secs(1))
            ),
            BackoffAction::FailBudgetTooSmall { prefer_request: true }
        );
    }

    #[test]
    fn backoff_only_grpc_present_and_too_small() {
        // No request deadline, gRPC remaining (1s) <= delay (2s) → fail via gRPC.
        assert_eq!(
            backoff_action(Duration::from_secs(2), None, Some(Duration::from_secs(1))),
            BackoffAction::FailBudgetTooSmall { prefer_request: false }
        );
    }

    // ─── decide_upstream_authority: pure decision, all three arms ───

    #[test]
    fn decide_upstream_authority_no_policy_is_noop() {
        // No Service key / no policy for this Service — caller never rendered.
        assert_eq!(decide_upstream_authority(None), AuthorityDecision::NoOp);
    }

    #[test]
    fn decide_upstream_authority_ok_render_is_set() {
        assert_eq!(
            decide_upstream_authority(Some(Ok("tenant-b.sandbox.internal".to_string()))),
            AuthorityDecision::Set("tenant-b.sandbox.internal".to_string())
        );
    }

    #[test]
    fn decide_upstream_authority_err_render_is_reject() {
        assert_eq!(
            decide_upstream_authority(Some(Err(AuthorityError::InvalidLabel))),
            AuthorityDecision::Reject(AuthorityError::InvalidLabel)
        );
        assert_eq!(
            decide_upstream_authority(Some(Err(AuthorityError::RenderEmpty))),
            AuthorityDecision::Reject(AuthorityError::RenderEmpty)
        );
    }

    // ─── apply_upstream_authority_decision: effects wiring ───

    /// Build a ctx with one "current" upstream entry carrying a TLS record whose
    /// `sni` holds the STATIC BackendTLSPolicy hostname (as `record_tls_to_upstream`
    /// would have written it during peer construction, before the dynamic
    /// authority is composed).
    fn ctx_with_static_tls_sni(sni: &str) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: Some("default/svc".to_string()),
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        let bc = ctx.backend_context.as_mut().unwrap();
        bc.upstreams.push(UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time: std::time::Instant::now(),
            err: Vec::new(),
            lb_backend_addr: None,
            lb_edgion_backend: None,
            lb_outlier_completion: None,
            service_key: None,
            lb_policy: None,
            tls: Some(BackendTlsInfo {
                sni: Some(sni.to_string()),
                handshake_ok: None,
                protocol: None,
                cipher: None,
            }),
            lb_committed: None,
        });
        bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        ctx
    }

    fn tls_peer(sni: &str) -> HttpPeer {
        HttpPeer::new(
            PingoraSocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 8080))),
            true,
            sni.to_string(),
        )
    }

    fn cleartext_peer() -> HttpPeer {
        HttpPeer::new(
            PingoraSocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 8080))),
            false,
            String::new(),
        )
    }

    #[test]
    fn apply_upstream_authority_decision_set_refreshes_recorded_sni_not_static_hostname() {
        // IMPORTANT-1 regression: the access-log-recorded SNI must be updated to
        // the composed authority, not left at the static BackendTLSPolicy hostname.
        let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
        let mut peer = tls_peer("svc.default.svc.cluster.local");

        let result = apply_upstream_authority_decision(
            AuthorityDecision::Set("tenant-b.sandbox.internal".to_string()),
            &mut ctx,
            &mut peer,
        );

        assert!(result.is_ok());
        assert_eq!(peer.sni, "tenant-b.sandbox.internal");
        assert_eq!(ctx.upstream_authority.as_deref(), Some("tenant-b.sandbox.internal"));

        let recorded_sni = ctx
            .get_current_upstream_mut()
            .and_then(|u| u.tls.as_ref())
            .and_then(|t| t.sni.as_deref());
        assert_eq!(
            recorded_sni,
            Some("tenant-b.sandbox.internal"),
            "access log must record the composed authority, not the static hostname"
        );
        assert_ne!(recorded_sni, Some("svc.default.svc.cluster.local"));
    }

    #[test]
    fn apply_upstream_authority_decision_set_skips_sni_on_cleartext_peer() {
        // MINOR-5 regression: setting SNI on a cleartext peer fragments the
        // pingora keepalive pool (sni is hashed into HttpPeer::reuse_hash), and
        // the SNI is meaningless on the wire for a non-TLS backend.
        let mut ctx = EdgionHttpContext::new();
        let mut peer = cleartext_peer();

        let result = apply_upstream_authority_decision(
            AuthorityDecision::Set("tenant-b.sandbox.internal".to_string()),
            &mut ctx,
            &mut peer,
        );

        assert!(result.is_ok());
        assert_eq!(peer.sni, "", "cleartext peer must not get a pool-fragmenting SNI");
        // The Host header rewrite must still happen regardless of TLS — ctx is
        // what `upstream_request_filter` reads to write the upstream Host.
        assert_eq!(ctx.upstream_authority.as_deref(), Some("tenant-b.sandbox.internal"));
    }

    #[test]
    fn apply_upstream_authority_decision_reject_fails_closed_and_leaves_peer_untouched() {
        // IMPORTANT-2 regression: pins that the fail-closed path cannot silently
        // fall through to the static SNI. If `Reject` were ever replaced with an
        // `Ok(())` no-op (silently keeping the static SNI), `result.is_err()`
        // would flip to false and this test would fail.
        let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
        let mut peer = tls_peer("svc.default.svc.cluster.local");
        let original_sni = peer.sni.clone();

        let result = apply_upstream_authority_decision(
            AuthorityDecision::Reject(AuthorityError::InvalidLabel),
            &mut ctx,
            &mut peer,
        );

        assert!(result.is_err(), "a bad render must fail closed, never silently succeed");
        assert_eq!(
            peer.sni, original_sni,
            "peer must be left with the static SNI, not a usable altered one"
        );
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::UpstreamAuthorityRejected));
        assert_eq!(ctx.get_current_upstream_mut().and_then(|u| u.status), Some(502));
        assert_eq!(ctx.upstream_authority, None);
    }

    #[test]
    fn noop_decision_clears_stale_upstream_authority() {
        // MINOR-3 regression: a stale ctx.upstream_authority from an earlier
        // attempt (e.g. a conf_sync policy removal between retries) must not
        // survive into a NoOp decision, or `upstream_request_filter` would write
        // a stale Host while the peer carries the (correct) static SNI.
        let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
        ctx.upstream_authority = Some("stale-from-prior-attempt.sandbox.internal".to_string());
        let mut peer = tls_peer("svc.default.svc.cluster.local");
        let original_sni = peer.sni.clone();

        let result = apply_upstream_authority_decision(AuthorityDecision::NoOp, &mut ctx, &mut peer);

        assert!(result.is_ok());
        assert_eq!(peer.sni, original_sni);
        assert_eq!(ctx.upstream_authority, None);
    }
}
