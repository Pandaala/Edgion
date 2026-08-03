use super::EdgionHttpProxy;
use crate::routes::http::session_persistence;
use crate::{EdgionHttpContext, ResponseStageDenial, SessionPersistenceState};
use edgion_resources::resources::http_route::SessionPersistenceType;
use edgion_resources::EdgionStatus;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::time::Instant;

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

/// Whether the response-stage plugin chains have already produced a terminal
/// outcome, so no further chain may run: either a stashed local reply, or a
/// denial that could not be stashed and will abort the exchange.
#[inline]
pub(super) fn response_stage_settled(ctx: &EdgionHttpContext) -> bool {
    ctx.local_reply_header.is_some() || ctx.response_denial.is_some()
}

/// Raise the abort for a response-stage denial the stage could not deliver.
///
/// `HTTPStatus` is deliberate: `error_while_proxy` leaves it non-retryable and
/// un-attributed to the backend, and `fail_to_proxy` writes it downstream
/// because nothing has been committed yet. A gRPC denial carries 200 — the wire
/// status of a gRPC error — and `fail_to_proxy` writes its exact trailers from
/// `ctx.response_denial` before the generic status handling would see it.
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

    // filter-04b: clear any stale local-reply stash from a previous upstream
    // attempt before this attempt's response-phase plugins run.
    ctx.local_reply_header = None;
    ctx.local_reply_body = None;
    ctx.local_reply_written = false;

    let status_code = upstream_response.status.as_u16();
    ctx.request_info.status = Some(status_code);

    // Session-persistence cookie/header injection runs AFTER the retry-on-status
    // decision below — see the relocated block — so a response that will be
    // discarded for retry does not consume the inject state or carry a pointless
    // Set-Cookie.

    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // Record header time (time from this upstream's start_time to receiving response header)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ht = Instant::now()
            .saturating_duration_since(upstream.start_time)
            .as_millis() as u64;
        upstream.ht = Some(ht);
        upstream.status = Some(status_code);
    }

    // Record every completed upstream response before retry-on-status can
    // return early. The corresponding HTTPStatus control-flow error is not a
    // local-origin failure and is filtered out by error_while_proxy.
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
            let cb_store = crate::backends::get_outlier_detection_store();
            match crate::backends::FailureClass::from_http_status(status_code) {
                Some(class) => cb_store.record_failure_by_addr(service_key, addr, class),
                None => cb_store.record_success_by_addr(service_key, addr),
            }
        }
    }

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

                    let buffer_ok = !session.as_ref().retry_buffer_truncated();
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
                                ctx.add_error(EdgionStatus::RetryBudgetExhausted);
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

    // Inject session persistence cookie/header if needed. Runs here, AFTER the
    // retry-on-status decision, so a response about to be discarded for retry
    // neither consumes the inject state nor carries a pointless Set-Cookie.
    // 101 Switching Protocols is terminal (true interim 1xx early-return at the
    // top); the `!is_informational()` guard excludes it from SP injection —
    // a protocol-upgrade response does not need a sticky Set-Cookie.
    if !upstream_response.status.is_informational() {
        match ctx.sp_state {
            SessionPersistenceState::InjectNew | SessionPersistenceState::InjectUpdate => {
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
                    // (`::1:port` vs `[::1]:port`), and `resolve_target_port_key`
                    // may rewrite the peer port away from the cell port — either
                    // makes every request re-issue `InjectNew` and silently
                    // degrade stickiness to plain LB.
                    let backend_addr = ctx.get_current_upstream().and_then(|u| match u.lb_backend_addr {
                        Some(SocketAddr::Inet(addr)) => Some(addr.to_string()),
                        _ => None,
                    });

                    if let Some(addr) = backend_addr {
                        let addrhash = session_persistence::generate_session_token(&addr);
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
                ctx.sp_state = SessionPersistenceState::Inactive;
            }
            SessionPersistenceState::Inactive => {} // no injection needed
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
        if let Some(backend) = ctx.selected_grpc_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_sync(session, ctx, upstream_response);
        } else if let Some(backend) = ctx.selected_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }
    }

    // Gateway-level plugins (response onion: before GatewayClass-global).
    if !response_stage_settled(ctx) {
        if let Some(plugins) = ctx.resolved_gateway_plugins.as_ref().map(std::sync::Arc::clone) {
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
        if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(std::sync::Arc::clone) {
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

    release_late_request_body_view(ctx);

    // A denial no local reply could carry aborts here: returning Ok would hand
    // the untouched upstream response to `response_filter` and on to the client.
    if let Some(denial) = ctx.response_denial.as_ref() {
        return Err(response_denial_error(denial));
    }

    Ok(())
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

#[cfg(all(test, feature = "body-buffering"))]
mod tests {
    use super::release_late_request_body_view;
    use crate::request_body::BodyStorage;
    use crate::EdgionHttpContext;
    use bytes::Bytes;
    use pingora_core::protocols::http::RequestBodyBuffer;

    #[tokio::test]
    async fn final_response_header_stage_releases_request_body_view() {
        let (mut storage, view) = BodyStorage::new(64, 64);
        storage.write(&Bytes::from_static(b"cached")).await.unwrap();
        storage.finish().await.unwrap();
        let mut request_body = crate::request_body::RequestBodyState::default();
        request_body.install(view);
        request_body.overwrite(Bytes::from_static(b"edited")).unwrap();

        let mut ctx = EdgionHttpContext {
            request_body,
            ..Default::default()
        };
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
        release_late_request_body_view(&mut ctx);

        assert!(!ctx.request_body.has_cached_body());
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
    }
}
