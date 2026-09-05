use super::pg_upstream_response_body_filter::{enforce_stream_processor_framing, run_stream_processor_headers};
use super::EdgionHttpProxy;
use crate::routes::http::session_persistence;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::{EdgionHttpContext, ResponseStageDenial, SessionPersistenceState};
use edgion_resources::resources::http_route::SessionPersistenceType;
use edgion_resources::EdgionErrorCode;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::time::{Duration, Instant, SystemTime};

const MAX_PROVIDER_RESET_HINT_BYTES: usize = 64;
const OPENAI_RESET_HEADERS: [&str; 2] = ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"];

/// Whether this response header is an INTERIM one that both response-stage
/// entry points must forward untouched.
///
/// 100-Continue / 102-Processing / 103-Early-Hints arrive as their own
/// `HttpTask::Header` on h1 upstreams, and pingora's `h1_response_filter` runs
/// every header task through both `upstream_response_filter` and
/// `response_filter` — its own `is_informational()` check gates only the
/// chunked-encoding upgrade, not the filter calls. A final header always
/// follows, and it is the one that carries the response-phase bookkeeping and
/// the plugin chains.
///
/// `101 Switching Protocols` is EXCLUDED: it is terminal, not interim. After a
/// 101 the h1 client stops reading response headers, so there is no final header
/// to defer to and everything must run on the 101 itself. This matches pingora's
/// own `is_informational_except_101` distinction.
#[inline]
pub(super) fn is_interim_informational(status: http::StatusCode) -> bool {
    status.is_informational() && status.as_u16() != 101
}

#[inline]
pub(super) fn has_final_response_written(session: &Session) -> bool {
    session
        .as_ref()
        .response_written()
        .is_some_and(|header| !is_interim_informational(header.status))
}

#[inline]
pub(super) fn select_normalized_upstream_candidate(ctx: &mut EdgionHttpContext, status: u16) {
    ctx.pre_plugin_response_status = Some(status);
    ctx.select_final_downstream_status(status);
}

/// Whether the response-stage plugin chains have already produced a terminal
/// outcome, so no further chain may run: either a stashed local reply, or a
/// denial that could not be stashed and will abort the exchange.
#[inline]
pub(super) fn response_stage_settled(ctx: &EdgionHttpContext) -> bool {
    ctx.response_stage_settled()
}

/// Raise the abort for a response-stage denial the stage could not deliver.
///
/// `HTTPStatus` is deliberate: `error_while_proxy` leaves it non-retryable and
/// un-attributed to the backend, and `fail_to_proxy` writes it downstream
/// because nothing has been committed yet. A gRPC denial carries 200 — the wire
/// status of a gRPC error — and `fail_to_proxy` writes its exact trailers from
/// the context's response denial before the generic status handling would see it.
#[inline]
pub(super) fn response_denial_error(denial: &ResponseStageDenial) -> Box<pingora_core::Error> {
    let status = match denial {
        ResponseStageDenial::Http(status) => *status,
        ResponseStageDenial::Grpc { .. } => 200,
    };
    pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(status),
        "edgion response-stage plugin denial",
    )
}

#[inline]
fn release_late_request_body_view(ctx: &mut EdgionHttpContext) {
    // The final response-header plugin chain is the last supported late
    // request-body consumer. Release its view here so an SSE/large download
    // cannot retain spilled-body file descriptors or materialized bytes for
    // the lifetime of the response. Retry early returns deliberately occur
    // before this point and keep the view for the next attempt.
    ctx.request_body.release_view();
}

#[inline]
fn normalize_final_response_headers(
    response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
    server_headers: &ServerHeaderOpts,
) {
    // Provider translators may rebuild the header map. Restore gateway-owned
    // server headers afterward so synchronous plugins observe Edgion's values.
    run_stream_processor_headers(response, ctx);
    server_headers.apply_to_response(response);
}

#[inline]
pub fn upstream_response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // All response-phase bookkeeping — status/ht recording, retry-on-status,
    // outlier-detection success/failure, session-persistence injection, pending
    // header-op application, and the sync response plugins — is deferred to the
    // final header task, otherwise an interim 1xx pollutes status/CB/ht and may
    // even misfire retry-on-status. Forward it unmodified; see
    // `is_interim_informational` for why 101 is not one of these.
    if is_interim_informational(upstream_response.status) {
        return Ok(());
    }

    // filter-04b: clear any stale response override from a previous upstream
    // attempt before this attempt's response-phase plugins run.
    ctx.reset_response_override();

    let status_code = upstream_response.status.as_u16();

    // Session-persistence cookie/header injection runs AFTER the retry-on-status
    // decision below — see the relocated block — so a response that will be
    // discarded for retry does not consume the inject state or carry a pointless
    // Set-Cookie.

    // One clock read serves both header-time accounting and the LB sample, so
    // `ht` and the score are provably the same measurement.
    let header_at = Instant::now();

    // Record header time (time from this upstream's start_time to receiving response header)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ht = header_at.saturating_duration_since(upstream.start_time).as_millis() as u64;
        upstream.ht = Some(ht);
        upstream.status = Some(status_code);
    }

    // Record the LB latency sample at response HEADERS, not at request end.
    //
    // Two defects this fixes, both documented in
    // `skills/12-algorithms/lb-peak-ewma.md`: settling at request end folded
    // response-body streaming time into the "latency" estimate (defect 2), so
    // an SSE stream scored in the tens of seconds; and a retried attempt
    // discarded its sample entirely (defect 4), leaving a backend that answers
    // slowly and then fails with a pristine score while one that answers slowly
    // and succeeds was penalized permanently.
    //
    // The sample is recorded before the retry decision is known, because
    // time-to-response-headers is a self-contained measurement of what this
    // backend did and nothing afterwards makes it untrue. A connect-level
    // failure that never produces headers records nothing; outlier detection
    // owns that.
    //
    // Whether it may *lower* the score is decided by
    // `lb::sample::is_useful_work` — 5xx and any route-declared retryable
    // status are recorded peak-only, so a backend failing fast cannot acquire
    // the best score in the pool. That predicate is deliberately NOT
    // `FailureClass::from_http_status`, which is Envoy-shaped for ejection
    // policy and classifies 429 the opposite way.
    //
    // Safe above the `has_ai_attempt_plan()` branch below: an AI attempt is
    // never LB-selected (`SelectedHttpBackend::Ai` is disjoint from `Service`,
    // and `backends/mod.rs` is the sole writer of `lb_edgion_backend`), so both
    // this block and the outlier block are inert on that path. If AI ever gains
    // an LB-selected backend, revisit — a 429 would then be "useful work" for
    // the score and a credential failure for the AI runtime at the same time.
    record_lb_latency_sample(ctx, status_code, header_at);

    // Record every completed upstream response before retry-on-status can
    // return early. The corresponding HTTPStatus control-flow error is not a
    // local-origin failure and is filtered out by error_while_proxy.
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(backend), Some(token)) = (
            upstream.lb_edgion_backend.as_deref(),
            upstream.lb_outlier_completion.as_ref(),
        ) {
            let cb_store = crate::backends::get_outlier_detection_store();
            match crate::backends::FailureClass::from_http_status(status_code) {
                Some(class) => cb_store.record_failure_for_backend_with_token(backend, token, class),
                None => cb_store.record_success_for_backend_with_token(backend, token),
            }
        }
    }

    if ctx.has_ai_attempt_plan() {
        let wants_retry = status_code != 101
            && ctx
                .route_unit
                .as_ref()
                .and_then(|route| route.rule.parsed_retry.as_ref())
                .and_then(|retry| retry.codes.as_ref())
                .is_some_and(|codes| codes.contains(&status_code));
        if status_code == 429 {
            let now = SystemTime::now();
            let retry_after = upstream_response
                .headers
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= MAX_PROVIDER_RESET_HINT_BYTES)
                .map(str::to_owned);
            let reset_hint = ctx
                .ai_state()
                .and_then(|state| state.request.as_ref())
                .and_then(|ai| provider_reset_hint(&upstream_response.headers, ai.provider, now));
            let cooldown = ctx.ai_state_mut().and_then(|state| {
                state.attempt_runtime.active_mut().and_then(|active| {
                    retry_after
                        .as_deref()
                        .and_then(|value| active.apply_retry_after(value, now))
                        .or_else(|| reset_hint.and_then(|hint| active.apply_cooldown(hint)))
                })
            });

            // Only an accepted, bounded hint is forwarded. Retried responses
            // are discarded later in this hook; a final 429 therefore exposes
            // the same normalized delta-seconds used for credential cooldown.
            upstream_response.remove_header("retry-after");
            if let Some(cooldown) = cooldown {
                let seconds = crate::ai::attempt_runtime::cooldown_delta_seconds(cooldown);
                let _ = upstream_response.insert_header("retry-after", seconds.to_string());
            }
        }
        let failure_class = crate::backends::FailureClass::from_http_status(status_code)
            .or_else(|| (status_code == 429 || wants_retry).then_some(crate::backends::FailureClass::Application));
        if let Some(active) = ctx.ai_state_mut().and_then(|state| state.attempt_runtime.active_mut()) {
            active.record_response(status_code, failure_class);
        }
        let buffer_ok = session.request_relay_retry_state().can_start_next_attempt();
        let gates_allow_retry = !ctx.request_timeout_exceeded()
            && !ctx.grpc_deadline_exceeded()
            && !ctx.response_write_started()
            && !has_final_response_written(session)
            && buffer_ok;
        match crate::ai::dispatch::handle_ai_response(ctx, wants_retry, gates_allow_retry)
            .map_err(|_| pingora_core::Error::new(pingora_core::ErrorType::InternalError))?
        {
            crate::ai::dispatch::AiRetryDecision::Retry => {
                if let Some(state) = ctx.ai_state_mut() {
                    state
                        .attempt_runtime
                        .finish_failure(crate::ai::attempt_runtime::AiAttemptFailure::Known(
                            failure_class.unwrap_or(crate::backends::FailureClass::Application),
                        ));
                }
                ctx.retry_on_status = true;
                if let Some(upstream) = ctx.get_current_upstream_mut() {
                    upstream.err.push("AiRetryOnStatus".to_string());
                }
                return Err(pingora_core::Error::explain(
                    pingora_core::ErrorType::HTTPStatus(status_code),
                    "AI retry on upstream status",
                ));
            }
            crate::ai::dispatch::AiRetryDecision::BudgetDenied => {
                if let Some(state) = ctx.ai_state_mut() {
                    state
                        .attempt_runtime
                        .finish_failure(crate::ai::attempt_runtime::AiAttemptFailure::Known(
                            failure_class.unwrap_or(crate::backends::FailureClass::Application),
                        ));
                }
                crate::observe::metrics::record_retry_budget_denied();
                ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                return Err(pingora_core::Error::explain(
                    pingora_core::ErrorType::HTTPStatus(503),
                    "AI retry budget exhausted",
                ));
            }
            crate::ai::dispatch::AiRetryDecision::Final => {}
        }
    } else {
        // Check if status code triggers retry based on rule.retry.codes
        if let Some(route_unit) = ctx.route_unit.as_ref() {
            if let Some(parsed_retry) = route_unit.rule.parsed_retry.as_ref() {
                if let Some(codes) = parsed_retry.codes.as_ref() {
                    if codes.contains(&status_code) {
                        // Determine max_retries: rule.retry.attempts > global
                        let max_retries = parsed_retry
                            .attempts
                            .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

                        // Check request timeout. Either the route-level
                        // `request_timeout` or a client-supplied `grpc-timeout`
                        // having expired disqualifies a status-code retry.
                        let request_timeout_exceeded = route_unit
                            .rule
                            .parsed_timeouts
                            .as_ref()
                            .and_then(|t| t.request_timeout.as_duration())
                            .map(|timeout| Instant::now().saturating_duration_since(ctx.start_time) >= timeout)
                            .unwrap_or(false);
                        let deadline_exceeded = request_timeout_exceeded || ctx.grpc_deadline_exceeded();

                        let buffer_ok = session.request_relay_retry_state().can_start_next_attempt();
                        if ctx.try_cnt <= max_retries && !deadline_exceeded && buffer_ok {
                            // Retry budget (retryConstraint): composes AFTER the
                            // maxRetries/deadline/buffer checks above (D3). This is the
                            // point where a status-code retry is DECIDED, so the budget
                            // is consulted here. On refusal, `retry_on_status` is left
                            // false (making pg_error_while_proxy Case 1 a no-op so no
                            // already-started response is corrupted) and a deterministic
                            // 503 is raised as a synthetic HTTPStatus error. The
                            // per-upstream entry keeps the backend's real observed status
                            // (stamped earlier); the 503 is a request-level outcome.
                            if let Some(budget) = super::pg_upstream_peer::retry_budget_for(ctx) {
                                if !budget.try_retry() {
                                    crate::observe::metrics::record_retry_budget_denied();
                                    ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                                    return Err(pingora_core::Error::explain(
                                        pingora_core::ErrorType::HTTPStatus(503),
                                        "retry budget exhausted",
                                    ));
                                }
                            }
                            ctx.retry_on_status = true;
                            // Stamp the attempt's per-upstream log entry with an
                            // explicit retry reason so access-log aggregation can
                            // count retries by reason instead of inferring this
                            // case from the entry's status code.
                            if let Some(upstream) = ctx.get_current_upstream_mut() {
                                upstream.err.push("RetryOnStatus".to_string());
                            }
                            return Err(pingora_core::Error::explain(
                                pingora_core::ErrorType::HTTPStatus(status_code),
                                format!("retry on status {}", status_code),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Private response processors see only the final, non-retried response.
    // They may normalize a provider status/content type and retain the body
    // mode they will use later. Per-attempt upstream bookkeeping above keeps
    // the provider's raw status; the request-level status is finalized later.
    normalize_final_response_headers(upstream_response, ctx, &edgion_http.server_header_opts);
    let normalized_status = upstream_response.status.as_u16();
    select_normalized_upstream_candidate(ctx, normalized_status);

    // Inject session persistence cookie/header if needed. Runs here, AFTER the
    // retry-on-status decision, so a response about to be discarded for retry
    // neither consumes the inject state nor carries a pointless Set-Cookie.
    // 101 Switching Protocols is terminal (true interim 1xx early-return at the
    // top); the `!is_informational()` guard excludes it from SP injection —
    // a protocol-upgrade response does not need a sticky Set-Cookie.
    if !upstream_response.status.is_informational() {
        match ctx.session_persistence().map(|persistence| persistence.state.clone()) {
            Some(SessionPersistenceState::InjectNew | SessionPersistenceState::InjectUpdate) => {
                // Read the effective rule config (HTTP or gRPC route unit) so
                // GRPCRoute responses inject tokens too. Placement is unchanged:
                // still after the retry-on-status decision, so a discarded
                // response never consumes the inject state.
                let sp_config = ctx.effective_session_persistence().cloned();

                if let Some(sp) = sp_config {
                    // Derive the token from the LB cell address (`lb_backend_addr`),
                    // formatted with `std::net::SocketAddr::to_string()`, so it is
                    // byte-identical to the affinity-map key built in
                    // `lb::runtime_state::build_sp_affinity_map`. Using the peer
                    // ip/port instead would mismatch the map in two ways: IPv6
                    // `IpAddr::to_string()` drops the brackets the map key carries
                    // (`::1:port` vs `[::1]:port`). The selected slot already
                    // carries the proven serving port, so rewriting the peer
                    // address would make every request re-issue `InjectNew` and
                    // silently degrade stickiness to plain LB.
                    let backend_addr = ctx.get_current_upstream().and_then(|u| match u.lb_backend_addr {
                        Some(SocketAddr::Inet(addr)) => Some(addr.to_string()),
                        _ => None,
                    });

                    if let Some(addr) = backend_addr {
                        let token_scope = ctx
                            .session_persistence()
                            .and_then(|persistence| persistence.token_scope.as_deref());
                        let addrhash = session_persistence::generate_scoped_session_token(token_scope, &addr);
                        // Freeze the absolute expiry at issuance: compute the ttl
                        // once (GEP-2257) and feed both the wire token and the
                        // Cookie Max-Age so the browser hint can never desync from
                        // the gateway-enforced expiry.
                        let now = session_persistence::now_unix_secs();
                        let ttl_secs = sp
                            .absolute_timeout
                            .as_deref()
                            .and_then(edgion_resources::resources::http_route::parse_gep2257_duration_secs);
                        let token = session_persistence::build_wire_token(&addrhash, now, ttl_secs);

                        match session_persistence::get_persistence_type(&sp) {
                            SessionPersistenceType::Cookie => {
                                let is_tls = session.as_downstream().digest().is_some_and(|d| d.ssl_digest.is_some());
                                // NOTE (filter-04c): this Set-Cookie is appended
                                // directly onto the upstream header, NOT via
                                // pending_response_header_ops. If a response-phase
                                // plugin replaces this response with a local reply,
                                // build_response_local_reply discards the upstream
                                // header, so sticky-session continuity is
                                // intentionally LOST on a replaced response — matching
                                // Envoy (no cookie carry-over onto a local/error
                                // reply). Accepted as designed; see
                                // skills/04-review/architecture/local-reply-drops-sticky-set-cookie.md
                                let cookie_value =
                                    session_persistence::build_set_cookie_value(&sp, &token, is_tls, ttl_secs);
                                let _ = upstream_response.append_header("Set-Cookie", &cookie_value);
                            }
                            SessionPersistenceType::Header => {
                                let header_name = session_persistence::get_session_name(&sp).to_owned();
                                let _ = upstream_response.append_header(header_name, &token);
                            }
                        }
                    }
                }
                if let Some(persistence) = ctx.session_persistence_mut() {
                    persistence.state = SessionPersistenceState::Inactive;
                }
            }
            Some(SessionPersistenceState::Inactive) | None => {} // no injection needed
        }
    }

    // filter-04c: apply request-stage-queued response-header ops onto the
    // upstream header NON-DESTRUCTIVELY (queue kept intact). Placed before the
    // sync runners so response-stage plugins observe the merged headers. If a
    // response-phase plugin replaces the response with a local reply, the
    // still-intact queue is re-applied by build_response_local_reply.
    ctx.apply_pending_response_header_ops(upstream_response);

    // Run rule-level upstream_response edgion_plugins (sync) for the matched
    // route type. A GRPCRoute request stores its compiled chain separately from
    // HTTPRoute; consulting only `route_unit` silently skips every gRPC
    // response-stage filter while the corresponding request-stage chain runs.
    if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
        grpc_route_unit
            .plugin_runtime
            .run_upstream_response_plugins_sync(session, ctx, upstream_response);
    } else if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit
            .plugin_runtime
            .run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    // Run backend-level upstream_response edgion_plugins (sync) for the same
    // matched route type.
    if !response_stage_settled(ctx) {
        if let Some(runtime) = ctx
            .selected_grpc_backend
            .as_ref()
            .map(crate::routes::grpc::SelectedGrpcBackend::plugin_runtime)
        {
            runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
        } else if let Some(runtime) = ctx
            .selected_backend
            .as_ref()
            .map(crate::backends::SelectedHttpBackend::plugin_runtime)
        {
            runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_gateway_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_plugins_count() == 0 {
                    continue;
                }
                runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
            }
        }
    }

    // Run global plugins last (onion: response stages go inner → outer).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_global_plugins() {
            for runtime in plugins.iter() {
                if response_stage_settled(ctx) {
                    break;
                }
                if runtime.upstream_response_plugins_count() == 0 {
                    continue;
                }
                runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
            }
        }
    }

    // H1 chooses chunked framing after this hook, so stale length metadata
    // must be gone here even if a pending operation or sync plugin restored it.
    if !response_stage_settled(ctx) {
        enforce_stream_processor_framing(upstream_response, ctx);
    }

    release_late_request_body_view(ctx);

    // A denial no local reply could carry aborts here: returning Ok would hand
    // the untouched upstream response to `response_filter` and on to the client.
    if let Some(denial) = ctx.response_denial() {
        return Err(response_denial_error(denial));
    }

    Ok(())
}

/// Fold this attempt's time-to-headers into the Peak-EWMA score.
///
/// Isolated so the call site is unit-testable without a Pingora session.
/// Returns whether a value was stored. Non-Ewma policies, missing backends,
/// and skipped peak-only samples all return `false`.
fn record_lb_latency_sample(ctx: &EdgionHttpContext, status: u16, header_at: Instant) -> bool {
    let Some(upstream) = ctx.get_current_upstream() else {
        return false;
    };
    if !matches!(
        upstream.lb_policy,
        Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma)
    ) {
        return false;
    }
    let Some(backend) = upstream.lb_edgion_backend.as_ref() else {
        return false;
    };
    let latency_us = header_at.saturating_duration_since(upstream.start_time).as_micros() as u64;
    let retry_codes = ctx
        .route_unit
        .as_ref()
        .and_then(|route| route.rule.parsed_retry.as_ref())
        .and_then(|retry| retry.codes.as_deref());
    crate::lb::sample::record_response_sample(
        backend,
        status,
        retry_codes,
        latency_us,
        crate::lb::ewma::score::units_at(header_at),
    )
}

fn provider_reset_hint(
    headers: &http::HeaderMap,
    provider: crate::ai::AiProviderLabel,
    now: SystemTime,
) -> Option<Duration> {
    let names: &[&str] = match provider {
        crate::ai::AiProviderLabel::OpenAi | crate::ai::AiProviderLabel::OpenAiCompatible => &OPENAI_RESET_HEADERS,
        crate::ai::AiProviderLabel::Anthropic => &[],
    };
    names
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .filter(|value| value.len() <= MAX_PROVIDER_RESET_HINT_BYTES)
        .filter_map(|value| edgion_common::time::parse_rate_limit_reset(value, now))
        .filter(|duration| !duration.is_zero())
        .max()
}

#[cfg(test)]
mod provider_reset_tests {
    use super::{provider_reset_hint, MAX_PROVIDER_RESET_HINT_BYTES};
    use crate::ai::AiProviderLabel;
    use http::{HeaderMap, HeaderValue};
    use std::time::{Duration, SystemTime};

    #[test]
    fn openai_reset_headers_accept_delta_and_choose_the_later_bound() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("250ms"));
        headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("2s"));
        assert_eq!(
            provider_reset_hint(&headers, AiProviderLabel::OpenAi, SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            provider_reset_hint(&headers, AiProviderLabel::OpenAiCompatible, SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            provider_reset_hint(&headers, AiProviderLabel::Anthropic, SystemTime::UNIX_EPOCH),
            None,
            "provider-specific headers must not bleed across protocols"
        );
    }

    #[test]
    fn openai_reset_headers_accept_absolute_epoch_and_reject_unsafe_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("1700000030"));
        assert_eq!(
            provider_reset_hint(&headers, AiProviderLabel::OpenAi, now),
            Some(Duration::from_secs(30))
        );

        headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("invalid"));
        headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("1699999999"));
        assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);

        let oversized = "1".repeat(MAX_PROVIDER_RESET_HINT_BYTES + 1);
        headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_str(&oversized).unwrap());
        assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);
    }
}

#[cfg(test)]
mod interim_status_tests {
    use super::is_interim_informational;
    use http::StatusCode;

    /// Both response-stage entry points forward these untouched and let the
    /// final header do the work. On the async side the guard is load-bearing for
    /// more than tidiness: a plugin denial here would arm the response-stage
    /// abort and kill an exchange whose real response has not arrived.
    #[test]
    fn interim_informational_covers_100_and_103_but_not_101() {
        assert!(is_interim_informational(StatusCode::CONTINUE));
        assert!(is_interim_informational(StatusCode::PROCESSING));
        assert!(is_interim_informational(StatusCode::from_u16(103).unwrap()));

        // 101 is terminal: the h1 client stops reading response headers after
        // it, so there is no final header to defer to.
        assert!(!is_interim_informational(StatusCode::SWITCHING_PROTOCOLS));
    }

    #[test]
    fn final_statuses_are_never_interim() {
        for code in [200u16, 204, 304, 403, 500, 502] {
            assert!(!is_interim_informational(StatusCode::from_u16(code).unwrap()), "{code}");
        }
    }
}

#[cfg(test)]
mod lb_sample_tests {
    use super::record_lb_latency_sample;
    use crate::lb::edgion_backend::EdgionBackend;
    use crate::lb::ewma::score::{units_at, DEFAULT_RTT_US};
    use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
    use crate::{BackendContext, EdgionHttpContext, MatchInfo, Rsc};
    use edgion_resources::resources::http_route::ParsedRouteRetry;
    use edgion_resources::resources::lb_policy::ParsedLBPolicy;
    use edgion_resources::{HTTPRouteMatch, HTTPRouteRule};
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn make_backend() -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 18080))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    fn ctx_with_attempt(
        backend: Arc<EdgionBackend>,
        policy: Option<ParsedLBPolicy>,
        start_time: Instant,
        retry_codes: Option<Vec<u16>>,
    ) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy: None,
        });
        let bc = ctx.backend_context.as_mut().unwrap();
        bc.upstreams.push(crate::UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time,
            err: Vec::new(),
            lb_backend_addr: None,
            lb_edgion_backend: Some(backend),
            lb_outlier_completion: None,
            service_key: None,
            lb_policy: policy,
            tls: None,
            lb_committed: None,
        });
        bc.current_upstream_id = Some(0);
        if let Some(codes) = retry_codes {
            ctx.route_unit = Some(Arc::new(route_unit_with_retry_codes(codes)));
        }
        ctx
    }

    fn route_unit_with_retry_codes(codes: Vec<u16>) -> HttpRouteRuleUnit {
        HttpRouteRuleUnit {
            resource_key: "test-ns/lb-sample".to_string(),
            matched_info: MatchInfo {
                rsc: Rsc::new("HTTPRoute", "test-ns", "lb-sample"),
                rule_id: 0,
                match_id: 0,
                sv: 0,
                m: HTTPRouteMatch {
                    path: None,
                    headers: None,
                    query_params: None,
                    method: None,
                },
                labels_meta: None,
                annotations_meta: None,
            },
            rule: Arc::new(HTTPRouteRule {
                name: None,
                resolved_terminal_route_uid: None,
                resolved_terminal_rule_identity: None,
                matches: None,
                filters: None,
                backend_refs: None,
                timeouts: None,
                retry: None,
                session_persistence: None,
                parsed_timeouts: None,
                parsed_retry: Some(ParsedRouteRetry {
                    attempts: None,
                    backoff: None,
                    codes: Some(codes),
                }),
                parsed_forward_raw_path: None,
                parsed_allow_non_idempotent_retry: None,
                parsed_mirror_tuning: None,
            }),
            route: None,
            path_regex: None,
            parent_refs: None,
            compiled_header_regexes: vec![],
            compiled_query_regexes: vec![],
            has_query_matchers: false,
            specificity: MatchSpecificity {
                has_method: false,
                header_count: 0,
                query_count: 0,
                creation_ts_ms: 0,
                route_key: "test-ns/lb-sample".to_string(),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(crate::plugins::PluginRuntime::new()),
            backends: Arc::new(vec![]),
            backend_finder: Arc::new(crate::lb::BackendSelector::new()),
        }
    }

    #[test]
    fn ewma_fast_success_is_recorded_at_header_time() {
        let backend = make_backend();
        // A future Instant makes `units_at(header_at)` far from process start.
        // Passing `0` instead would decay this 50 ms peak back to default.
        let header_at = Instant::now() + Duration::from_secs(20);
        let start_time = header_at - Duration::from_millis(50);
        let ctx = ctx_with_attempt(backend.clone(), Some(ParsedLBPolicy::Ewma), start_time, None);

        assert!(record_lb_latency_sample(&ctx, 200, header_at));
        assert_eq!(backend.latency_estimate(units_at(header_at)), 50_000);
    }

    #[test]
    fn non_ewma_policy_does_not_score() {
        let backend = make_backend();
        let header_at = Instant::now();
        let ctx = ctx_with_attempt(
            backend.clone(),
            Some(ParsedLBPolicy::LeastConn),
            header_at - Duration::from_millis(50),
            None,
        );

        assert!(!record_lb_latency_sample(&ctx, 200, header_at));
        assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
    }

    #[test]
    fn fast_5xx_does_not_lower_the_score() {
        let backend = make_backend();
        let header_at = Instant::now();
        let ctx = ctx_with_attempt(
            backend.clone(),
            Some(ParsedLBPolicy::Ewma),
            header_at - Duration::from_micros(2_000),
            None,
        );

        assert!(!record_lb_latency_sample(&ctx, 503, header_at));
        assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
    }

    #[test]
    fn slow_5xx_still_raises_the_score() {
        let backend = make_backend();
        let header_at = Instant::now();
        let ctx = ctx_with_attempt(
            backend.clone(),
            Some(ParsedLBPolicy::Ewma),
            header_at - Duration::from_secs(8),
            None,
        );

        assert!(record_lb_latency_sample(&ctx, 503, header_at));
        assert_eq!(backend.latency_estimate(units_at(header_at)), 8_000_000);
    }

    #[test]
    fn retryable_429_is_read_from_the_route_and_is_peak_only() {
        let backend = make_backend();
        let header_at = Instant::now();
        let ctx = ctx_with_attempt(
            backend.clone(),
            Some(ParsedLBPolicy::Ewma),
            header_at - Duration::from_micros(2_000),
            Some(vec![429]),
        );

        assert!(!record_lb_latency_sample(&ctx, 429, header_at));
        assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_final_response_headers, release_late_request_body_view};
    use crate::ctx::ResponseStreamKind;
    use crate::plugins::runtime::traits::{PluginSession, StreamingResponseProcessor};
    use crate::request_body::BodyStorage;
    use crate::runtime::server::server_header::ServerHeaderOpts;
    use crate::EdgionHttpContext;
    use async_trait::async_trait;
    use bytes::Bytes;
    use pingora_core::protocols::http::RequestBodyBuffer;
    use pingora_http::ResponseHeader;
    use pingora_proxy::ResponseBodySink;

    struct ProviderHeaderReset;

    #[async_trait]
    impl StreamingResponseProcessor for ProviderHeaderReset {
        fn name(&self) -> &str {
            "provider-header-reset"
        }

        fn on_response_header(&mut self, response: &mut ResponseHeader) {
            response.remove_header("server");
            response.remove_header("strict-transport-security");
        }

        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn PluginSession,
        ) {
        }
    }

    #[test]
    fn gateway_server_headers_are_applied_after_provider_header_reset() {
        let mut tls_response = ResponseHeader::build(200, None).unwrap();
        tls_response.insert_header("server", "provider").unwrap();
        tls_response
            .insert_header("strict-transport-security", "max-age=1")
            .unwrap();
        let mut tls_ctx = EdgionHttpContext::default();
        tls_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));

        normalize_final_response_headers(&mut tls_response, &mut tls_ctx, &ServerHeaderOpts::default());

        assert_eq!(tls_response.headers["server"], "Edgion");
        assert_eq!(
            tls_response.headers["strict-transport-security"],
            "max-age=63072000; includeSubDomains; preload"
        );

        let mut plain_response = ResponseHeader::build(200, None).unwrap();
        plain_response.insert_header("server", "provider").unwrap();
        plain_response
            .insert_header("strict-transport-security", "max-age=1")
            .unwrap();
        let mut plain_ctx = EdgionHttpContext::default();
        plain_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));
        let mut plain_headers = ServerHeaderOpts::default();
        plain_headers.remove_hsts();

        normalize_final_response_headers(&mut plain_response, &mut plain_ctx, &plain_headers);

        assert_eq!(plain_response.headers["server"], "Edgion");
        assert!(plain_response.headers.get("strict-transport-security").is_none());
    }

    #[tokio::test]
    async fn final_response_header_stage_releases_request_body_view() {
        let (mut storage, view) = BodyStorage::new(64, 64);
        storage.write(&Bytes::from_static(b"cached")).await.unwrap();
        storage.finish().await.unwrap();
        let mut request_body = crate::request_body::RequestBodyState::default();
        request_body.install(view);
        request_body.overwrite(Bytes::from_static(b"edited")).unwrap();

        let mut ctx = EdgionHttpContext::default();
        ctx.request_body = request_body;
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
        release_late_request_body_view(&mut ctx);

        assert!(!ctx.request_body.has_cached_body());
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
    }
}
