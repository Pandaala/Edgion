use super::EdgionHttpProxy;
use crate::ai::protocols::error::render_ai_error;
use crate::ai::runtime::dispatch::{
    ai_predispatch_deadline, commit_ai_attempt_context, prepare_first_attempt, AiDispatchError,
    AiPeerPredispatchFailure, PreparedAiAttempt,
};
use crate::ai::AiProxyError;
use crate::backends::peer_builder::{
    apply_backend_app_protocol, create_tls_peer, extract_tls_config, parse_backend_app_protocol,
};
use crate::backends::policy::backend_traffic::authority::{render_authority, AuthorityError};
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;
use crate::backends::policy::backend_traffic::RetryBudget;
use crate::backends::runtime::{CompiledBackendTarget, CompiledServiceTarget};
use crate::backends::ServiceBackendKind;
use crate::backends::{get_peer, try_get_peer};
use crate::backends::{SelectedHttpBackend, SelectedLogicalBackend, SelectedServiceBackend};
use crate::ctx::BackendSource;
use crate::observe::metrics::record_authority_rejected;
use crate::plugins::http::common::outbound::egress::{default_block_private, filter_resolved_addrs, EgressPolicy};
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::grpc::handle_grpc_upstream;
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
    // endpoint (`lb_backend`) is now known but Pingora has NOT yet
    // connected the returned peer — this is the pre-connect admission point, and
    // the counter increment IS the acquire. If the endpoint is at cap, shed with
    // a non-retrying 503 that mirrors the established `end_response_503` +
    // fail-closed-error shed (see `get_peer_with_sp_fallback`): the error is
    // returned out of `upstream_peer` WITHOUT setting the pingora retry flag, so
    // there is no shed-induced retry storm to another endpoint. Because the shed
    // returns before any connect, it reaches none of the outlier-detection
    // `record_failure` sites — a busy-but-healthy endpoint is never ejected by
    // its own shed. DirectEndpoint retains its pinned Service identity and
    // effective policy, but has no selected `lb_backend`; ExternalJump has
    // neither Service identity nor `lb_backend`. This is therefore a no-op for
    // both bypass paths.
    if let Some(service_key) = ctx.backend_context.as_ref().and_then(|bc| bc.service_key.clone()) {
        if let Some(max) = selected_effective_policy(ctx).and_then(|policy| policy.circuit_breaker_limit) {
            if let Some(chosen_backend) = ctx.get_current_upstream().and_then(|u| u.lb_backend.clone()) {
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
    crate::plugins::http::grpc_json_transcoder::validate_request(session.req_header(), ctx)?;
    if ctx.grpc_transcoding.is_some() || ctx.is_grpc_route_matched {
        crate::plugins::http::grpc_json_transcoder::require_h2(&mut peer);
    }
    if ctx.grpc_transcoding.is_some() {
        let cap = std::time::Duration::from_secs(30);
        peer.options.read_timeout = Some(peer.options.read_timeout.unwrap_or(cap).min(cap));
        peer.options.write_timeout = Some(peer.options.write_timeout.unwrap_or(cap).min(cap));
    }

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
/// A no-op when the chosen backend has no Service key (ExternalJump) or when
/// the Service carries no `upstreamAuthority` policy, so an unconfigured
/// Service behaves byte-for-byte as today. DirectEndpoint retains its pinned
/// Service slot and therefore receives the same captured authority policy as
/// the ordinary Service path. Both early-return paths clear
/// `ctx.upstream_authority` so a conf_sync policy removal between retry
/// attempts of the same request cannot leave a stale composed authority (and
/// therefore a stale upstream `Host`) from a previous attempt.
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
    if ctx.has_strict_jump_conflict() {
        return Err(PingoraError::explain(
            ErrorType::HTTPStatus(500),
            "conflicting upstream selection",
        ));
    }
    finalize_request_uri(session, ctx)?;

    // AI forwarding owns a typed external peer path and never writes the
    // generic first-write-wins upstream-jump slot.
    if ctx.has_ai_attempt_plan() {
        let entropy = uuid::Uuid::new_v4().as_u128();
        let route_retry_cap = ctx.retry_attempts();
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.drain_pending().await;
        }
        loop {
            let mut target = match prepare_first_attempt(session, ctx, entropy, route_retry_cap, ctx.egc.retry_attempts)
            {
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
                .admit(crate::ai::runtime::attempt_runtime::AiAttemptAdmissionInput {
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
                crate::ai::runtime::attempt_runtime::AiAttemptAdmission::Ready(runtime) => {
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
                crate::ai::runtime::attempt_runtime::AiAttemptAdmission::Skip => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::runtime::dispatch::handle_ai_predispatch_failure(ctx, true) {
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
                crate::ai::runtime::attempt_runtime::AiAttemptAdmission::SkipBackend => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::runtime::dispatch::handle_ai_predispatch_service_failure(ctx) {
                        end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
                crate::ai::runtime::attempt_runtime::AiAttemptAdmission::DependencyDenied => {
                    if let Some(response) = stop_ai_predispatch_if_expired(ctx).await {
                        return Err(response);
                    }
                    if !crate::ai::runtime::dispatch::handle_ai_predispatch_failure(ctx, true) {
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
            .filter(|backend| backend.ref_denied.is_none())
            .filter_map(sticky_candidate);
        session_persistence::sp_resolve_typed_candidates(ctx, candidates, sp_token.as_deref())
    };
    let sticky_backend = sp_idx.and_then(|idx| {
        let cb = &route_unit.backends[idx];
        if cb.ref_denied.is_none() {
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
    // round-robin only on a genuine miss; without InternalJump, plain weighted
    // round-robin.
    let backend_ref = match sticky_backend {
        Some(b) => b,
        None => match pick_backend_ref(&route_unit, ctx.jump.as_deref()) {
            Ok(b) => b,
            Err(e) => {
                ctx.record_error(backend_select_status(&e));
                end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        },
    };

    // Preserve a rejected slot's diagnostic index without fabricating a Service.
    match backend_ref.target() {
        CompiledBackendTarget::Invalid { .. } => {
            let slot = backend_ref.slot();
            ctx.selected_backend = Some(SelectedHttpBackend::Invalid(backend_ref));
            ctx.backend_context = Some(crate::BackendContext {
                source: None,
                rsc: None,
                service_key: None,
                service_kind: None,
                upstreams: Vec::new(),
                current_upstream_id: None,
                backend_ref_idx: Some(slot),
                effective_policy: None,
            });
            ctx.record_error(EdgionErrorCode::BackendUnsupportedKind);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
        CompiledBackendTarget::EdgionBackend(_) => {
            ctx.record_error(EdgionErrorCode::UpstreamNotBackendRefs);
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
        CompiledBackendTarget::Service(_) => {}
    }
    let backend_tls_policy = crate::backends::query_backend_tls_policy_for_logical_backend(&backend_ref);

    let backend_plugin_runtime = Arc::clone(&backend_ref.compiled().plugin_runtime);
    if !backend_plugin_runtime.backend_compatible(ctx) {
        if ctx.grpc_transcoding.is_some() {
            return Err(crate::plugins::http::grpc_json_transcoder::failure());
        }
        return Err(PingoraError::explain(
            ErrorType::HTTPStatus(500),
            "incompatible selected backend plugins",
        ));
    }
    backend_plugin_runtime.run_backend_request_plugins(session, ctx).await;
    if ctx.plugin_terminated() {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Err(PingoraError::new(ErrorType::InternalError));
    }

    if ctx.has_strict_jump_conflict() {
        return Err(PingoraError::explain(
            ErrorType::HTTPStatus(500),
            "conflicting upstream selection",
        ));
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
) -> Option<session_persistence::StickyCandidate<'_>> {
    let service = backend.permitted_service()?;
    Some(session_persistence::StickyCandidate {
        slot: backend.slot(),
        service_key: service.key(),
        section: service.section(),
        scope: None,
    })
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
/// round-robin only when the named target is genuinely absent; any other lookup
/// error propagates. When `ctx.jump` is None or carries a non-Internal variant,
/// plain weighted round-robin.
#[inline]
fn pick_backend_ref(
    unit: &std::sync::Arc<crate::routes::http::HttpRouteRuleUnit>,
    jump: Option<&crate::UpstreamJumpTarget>,
) -> Result<crate::backends::LogicalBackendHandle, EdError> {
    if let Some(crate::UpstreamJumpTarget::Internal(j)) = jump {
        match RouteRules::find_backend_by_name(unit, &j.backend_ref_name, j.backend_ref_namespace.as_deref()) {
            Ok(b) => return Ok(b),
            // Only a genuine miss falls through to weighted RR. Every other
            // outcome — notably `RefDenied` — stays typed and fails closed:
            // silently selecting a legal sibling would turn a denied
            // cross-namespace reference into an undetectable misroute.
            Err(EdError::BackendNotFound()) => {}
            Err(e) => return Err(e),
        }
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

/// Bound the entire TCP/TLS establishment, not backend request I/O. A zero
/// remaining budget is expired, unlike a disabled configured connect timeout.
/// Deadline owners win ties, with the route request ahead of the gRPC client.
#[inline]
fn fold_connect_timeouts(
    configured: Option<std::time::Duration>,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> Option<(std::time::Duration, crate::ctx::ConnectTimeoutSource)> {
    use crate::ctx::ConnectTimeoutSource;
    let mut best = None;
    for (candidate, source) in [
        (request_remaining, ConnectTimeoutSource::RequestDeadline),
        (grpc_remaining, ConnectTimeoutSource::GrpcDeadline),
        (configured.filter(|d| !d.is_zero()), ConnectTimeoutSource::Configured),
    ] {
        if let Some(d) = candidate {
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, source));
            }
        }
    }
    best
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

#[cfg(test)]
include!("../../../../tests/support/routes/http/proxy_http/pg_upstream_peer_test/1088_function_item.rs");

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
pub fn configure_peer_timeouts(_edgion_http: &EdgionHttpProxy, peer: &mut Box<HttpPeer>, ctx: &mut EdgionHttpContext) {
    let backend_timeout = &ctx.egc.parsed_timeouts.backend;
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

    // Resolve the configured connect bound from the captured policy generation.
    // End-to-end deadlines may tighten it; backendRequest remains I/O-only.
    let captured_timeout = selected_effective_policy(ctx).and_then(|policy| policy.connect_timeout);
    let selected_is_logical_backend = ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai);
    let connect_deadline = choose_typed_connect_deadline(
        selected_is_logical_backend,
        captured_timeout,
        captured_timeout,
        backend_timeout.connect_timeout,
    );
    let now = std::time::Instant::now();
    let request_remaining = ctx
        .request_timeout()
        .map(|req| req.saturating_sub(now.saturating_duration_since(ctx.start_time)));
    let grpc_remaining = ctx
        .request_info
        .grpc_client_deadline()
        .map(|deadline| deadline.saturating_duration_since(now));
    let connect = fold_connect_timeouts(connect_deadline, request_remaining, grpc_remaining);
    ctx.connect_timeout_source = connect.map(|(_, source)| source);
    peer.options.connection_timeout = connect.map(|(duration, _)| duration);
    // Pingora applies connection_timeout separately to TCP and TLS. This outer
    // bound prevents the two phases from each spending the remaining budget.
    peer.options.total_connection_timeout = peer.options.connection_timeout;

    // Apply request_timeout as a ceiling on read/write timeout.
    // Gateway API spec: request_timeout is end-to-end; remaining time = timeout - elapsed.
    // Filter out zero remaining time to avoid setting Duration::ZERO on the peer (undefined behaviour).
    let request_remaining = request_remaining.filter(|r| !r.is_zero());

    // Client-supplied `grpc-timeout` is another upper bound on read/write —
    // an intermediary that ignores it leaves upstream work running after the
    // client has given up. `grpc_remaining` returns the deadline minus now,
    // so we apply the same zero-guard as request_remaining.
    let grpc_remaining = grpc_remaining.filter(|r| !r.is_zero());

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
    // A missing policy is also a captured result. Never recapture it after a
    // hot reload adds a policy between attempts of this request.
    if ctx.try_cnt == 1 {
        capture_service_effective_policy(ctx);
    }

    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
    }

    // Retry-budget denominator (D1a): every upstream attempt to a Service — the
    // first attempt AND every retry (connect-fail, status-code, per-try timeout,
    // keepalive reuse) — counts toward the per-backend request volume, so the
    // percentage cap is `retries <= percent% * total_attempts`. The first
    // attempt captures policy through the permitted selected Service proof
    // and checks the context projection. Later attempts reuse the captured
    // Arc or absence in backend_context. DirectEndpoint uses its pinned route
    // slot's Service proof; ExternalJump has no Service budget.
    if let Some(budget) = retry_budget_for(ctx) {
        budget.on_attempt();
    }
}

/// Borrow the retry budget captured for this request's selected target.
/// Service policy was captured from the permitted HTTP/GRPC proof, or the
/// DirectEndpoint pinned route slot, after checking its context projection.
/// `backend_context` stores that result; its key does not authorize a lookup.
/// AI uses its separately captured policy and ExternalJump has no Service budget.
/// Shared by the denominator hook and the three retry gates (`pg_fail_to_connect`,
/// `pg_error_while_proxy` Case 1.5, and `pg_upstream_response_filter`).
#[inline]
pub(super) fn retry_budget_for(ctx: &EdgionHttpContext) -> Option<std::sync::Arc<RetryBudget>> {
    selected_effective_policy(ctx)
        .and_then(|policy| policy.retry_budget.as_ref())
        .map(std::sync::Arc::clone)
}

fn selected_service_target(ctx: &EdgionHttpContext) -> Option<&CompiledServiceTarget> {
    match ctx.jump.as_deref() {
        Some(UpstreamJumpTarget::External(_)) => return None,
        Some(UpstreamJumpTarget::Direct(preset)) => {
            return ctx
                .route_unit
                .as_ref()?
                .backends
                .get(preset.backend_ref_idx)?
                .permitted_service()
        }
        _ => {}
    }
    if let Some(grpc) = ctx.selected_grpc_backend.as_ref() {
        return grpc.compiled().permitted_service();
    }
    match ctx.selected_backend.as_ref()? {
        SelectedHttpBackend::Service(selected) => selected.logical().service_target(),
        SelectedHttpBackend::Ai(_) | SelectedHttpBackend::Invalid(_) => None,
    }
}

fn capture_service_effective_policy(ctx: &mut EdgionHttpContext) {
    capture_service_effective_policy_with(ctx, |target| {
        get_backend_traffic_policy_store().snapshot().get_effective(target)
    });
}

fn capture_service_effective_policy_with(
    ctx: &mut EdgionHttpContext,
    mut lookup: impl FnMut(
        &edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey,
    ) -> Option<Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>>,
) {
    let Some(context) = ctx.backend_context.as_ref() else {
        return;
    };
    if context.effective_policy.is_some() {
        return;
    }
    let Some(service) = selected_service_target(ctx) else {
        return;
    };
    if !crate::backends::service_context_matches(Some(context), service) {
        return;
    }
    let policy = lookup(service.policy_target());
    if let Some(context) = ctx.backend_context.as_mut() {
        context.effective_policy = policy;
    }
}

fn selected_effective_policy(
    ctx: &EdgionHttpContext,
) -> Option<&std::sync::Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>> {
    if ctx.selected_backend.as_ref().is_some_and(SelectedHttpBackend::is_ai) {
        return ctx.ai_state().and_then(|state| state.effective_policy.as_ref());
    }
    ctx.backend_context.as_ref()?.effective_policy.as_ref()
}

#[cfg(test)]
include!("../../../../tests/support/routes/http/proxy_http/pg_upstream_peer_test/1312_function_item.rs");

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
    let compiled_target = match target.selected.logical().target() {
        CompiledBackendTarget::EdgionBackend(target) => target,
        CompiledBackendTarget::Service(_) | CompiledBackendTarget::Invalid { .. } => {
            return Err(AiPeerPredispatchFailure::Endpoint);
        }
    };
    let (namespace, name) = (compiled_target.namespace(), compiled_target.name());
    ctx.backend_context = Some(crate::ctx::BackendContext {
        source: Some(BackendSource::AiBackend),
        rsc: Some(crate::Rsc::new("EdgionBackend", namespace, name)),
        service_key: Some(compiled_target.key().to_owned()),
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
    let Some(UpstreamJumpTarget::Direct(direct)) = ctx.jump.as_deref() else {
        // invariant: caller (line 146) only reaches this helper after matching Direct{..};
        //   other variants are dispatched to sibling helpers
        unreachable!("caller must dispatch only on ctx.jump == Some(Direct)");
    };
    let addr = PingoraSocketAddr::Inet(direct.addr);
    let policy = direct.tls_policy.clone();
    let gateway_key = ctx.gateway_info.as_deref().map(|gateway| gateway.gateway_key());
    let peer = crate::backends::validate_endpoint_backend_addr(&addr, ctx.allow_loopback_upstream)
        .and_then(|()| crate::backends::peer_builder::peer_from_backend_tls_lookup(addr, &policy, gateway_key));
    let mut peer = match peer {
        Ok(peer) => peer,
        Err(status) => {
            ctx.record_error(status);
            return Err(PingoraError::explain(
                ErrorType::ConnectError,
                "direct endpoint unavailable",
            ));
        }
    };
    let app_protocol = direct.app_protocol.as_deref().and_then(parse_backend_app_protocol);
    apply_backend_app_protocol(&mut peer, app_protocol, ctx);
    let result = finish_static_peer(edgion_http, ctx, peer);
    let (use_tls, sni) = extract_tls_config(&policy);
    crate::backends::peer_builder::record_tls_to_upstream(ctx, use_tls, &sni);

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
    let Some(UpstreamJumpTarget::External(external)) = ctx.jump.as_deref() else {
        // invariant: caller (line 147) only reaches this helper after matching External(..);
        //   other variants are dispatched to sibling helpers
        unreachable!("caller must dispatch only on ctx.jump == Some(External)");
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

    if let Some(UpstreamJumpTarget::Direct(preset)) = ctx.jump.as_deref() {
        let (name, namespace, kind, slot) = ctx
            .route_unit
            .as_ref()
            .and_then(|unit| direct_backend_metadata(&unit.backends, preset.backend_ref_idx))
            .ok_or_else(|| PingoraError::new(ErrorType::InternalError))?;
        let key = format!("{namespace}/{name}");
        ctx.init_backend_context(crate::Rsc::new("Service", namespace, name), key, kind, Some(slot));
        return Ok(());
    }
    if let Some(SelectedHttpBackend::Invalid(logical)) = ctx.selected_backend.as_ref() {
        // Keep descriptor diagnostics without minting a Service resource identity.
        ctx.backend_context = Some(crate::BackendContext {
            source: None,
            rsc: None,
            service_key: None,
            service_kind: Some(ServiceBackendKind::Unsupported(
                logical
                    .compiled()
                    .kind
                    .clone()
                    .unwrap_or_else(|| "unsupported backend reference".to_string()),
            )),
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: Some(logical.slot()),
            effective_policy: None,
        });
        return Ok(());
    }
    let service = selected_service_target(ctx).ok_or_else(|| PingoraError::new(ErrorType::InternalError))?;
    let name = service.name().to_owned();
    let namespace = service.namespace().to_owned();
    let key = service.key().to_owned();
    let slot = match ctx.jump.as_deref() {
        Some(UpstreamJumpTarget::Direct(preset)) => Some(preset.backend_ref_idx),
        _ => ctx
            .selected_grpc_backend
            .as_ref()
            .map(|backend| backend.slot())
            .or_else(|| ctx.selected_backend.as_ref().map(|backend| backend.slot())),
    };
    ctx.init_backend_context(
        crate::Rsc::new("Service", namespace, name),
        key,
        ServiceBackendKind::Service,
        slot,
    );
    Ok(())
}

fn direct_backend_metadata(
    backends: &[crate::routes::http::match_unit::CompiledBackend],
    slot: usize,
) -> Option<(String, String, ServiceBackendKind, usize)> {
    let backend = backends.get(slot)?;
    let service = backend.permitted_service()?;
    Some((
        service.name().to_owned(),
        service.namespace().to_owned(),
        ServiceBackendKind::Service,
        backend.slot(),
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
    if !want_raw && ctx.path() != session.req_header().uri.path() {
        let path_only = ctx
            .uri(session.req_header())
            .parse::<http::Uri>()
            .map_err(|e| PingoraError::because(ErrorType::InternalError, "compose normalized uri", e))?;
        crate::ctx::set_path_only_request_target(session.req_header_mut(), path_only)
            .map_err(|e| PingoraError::because(ErrorType::InternalError, "retain request target", e))?;
    }
    ctx.uri_finalized = true;
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
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_upstream_peer_test.rs"]
mod tests;
