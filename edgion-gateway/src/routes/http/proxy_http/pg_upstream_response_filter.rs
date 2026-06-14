use super::EdgionHttpProxy;
use crate::routes::http::session_persistence;
use crate::{EdgionHttpContext, SessionPersistenceState};
use edgion_resources::resources::http_route::SessionPersistenceType;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::time::Instant;

#[inline]
pub fn upstream_response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Interim 1xx informational responses (100-Continue, 102-Processing,
    // 103-Early-Hints) are delivered as separate HttpTask::Header on h1 upstreams
    // (pingora's h1 client re-reads a response header for 100..=199), so each one
    // runs through this filter. They are interim, not the final response: all
    // response-phase bookkeeping — status/ht recording, retry-on-status,
    // circuit-breaker success/failure, session-persistence injection, pending
    // header-op application, and the sync response plugins — must be deferred to
    // the final header task, otherwise the 1xx pollutes status/CB/ht and may even
    // misfire retry-on-status. Forward the 1xx unmodified and let the final header
    // do all of it. (h2 upstreams never surface 1xx here.)
    //
    // 101 Switching Protocols is deliberately EXCLUDED: it is terminal, not
    // interim. After a 101 the h1 client stops reading response headers
    // (`should_read_resp_header()` is false for 101), so there is no final header
    // to defer to — the upgrade's ht/CB/ResponseHeaderModifier/plugin bookkeeping
    // must run on the 101 itself. This matches pingora's own
    // `is_informational_except_101` distinction.
    if upstream_response.status.is_informational() && upstream_response.status.as_u16() != 101 {
        return Ok(());
    }

    // filter-04b: clear any stale local-reply stash from a previous upstream
    // attempt before this attempt's response-phase plugins run.
    ctx.local_reply_header = None;
    ctx.local_reply_body = None;
    ctx.local_reply_written = false;

    // Record status code
    let status_code = upstream_response.status.as_u16();
    ctx.request_info.status = Some(status_code);

    // Session-persistence cookie/header injection runs AFTER the retry-on-status
    // decision below — see the relocated block — so a response that will be
    // discarded for retry does not consume the inject state or carry a pointless
    // Set-Cookie.

    // Apply custom server headers (including Server header)
    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // Record header time (time from this upstream's start_time to receiving response header)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ht = Instant::now()
            .saturating_duration_since(upstream.start_time)
            .as_millis() as u64;
        upstream.ht = Some(ht);
        upstream.status = Some(status_code);
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
    // The `!is_informational()` guard now filters exactly one case: a terminal
    // 101 Switching Protocols (the only 1xx reaching this point — true interim
    // 100/102/103 early-return at the top). This deliberately changes the
    // pre-1xx-guard behavior, which DID inject on a 101: a protocol-upgrade
    // response (e.g. WebSocket) no longer receives a sticky Set-Cookie. Injecting
    // on the final (non-1xx) response is the normal sticky-session path.
    if !upstream_response.status.is_informational() {
        match ctx.sp_state {
            SessionPersistenceState::InjectNew | SessionPersistenceState::InjectUpdate => {
                let sp_config = ctx
                    .route_unit
                    .as_ref()
                    .and_then(|ru| ru.rule.session_persistence.clone());

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
                        let token = session_persistence::generate_session_token(&addr);

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
                                let cookie_value = session_persistence::build_set_cookie_value(&sp, &token, is_tls);
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

    // Record outlier-detection success/failure, classifying 5xx into gateway
    // (502/503/504) vs application (other >=500) buckets.
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
            let cb_store = crate::backends::get_circuit_breaker_store();
            match crate::backends::FailureClass::from_http_status(status_code) {
                Some(class) => cb_store.record_failure_by_addr(service_key, addr, class),
                None => cb_store.record_success_by_addr(service_key, addr),
            }
        }
    }

    // filter-04c: apply request-stage-queued response-header ops onto the
    // upstream header NON-DESTRUCTIVELY (queue kept intact). On the normal
    // forward path this is the single application point, replacing the former
    // per-runner pre-drain. Placed before the sync runners so response-stage
    // plugins still observe the merged headers. If a response-phase plugin then
    // replaces the response with a local reply, the still-intact queue is
    // re-applied onto the fresh reply by build_response_local_reply.
    ctx.apply_pending_response_header_ops(upstream_response);

    // Run rule-level upstream_response edgion_plugins (sync)
    if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit
            .plugin_runtime
            .run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    // Run backend-level upstream_response edgion_plugins (sync)
    if ctx.local_reply_header.is_none() {
        if let Some(backend) = ctx.selected_backend.clone() {
            backend
                .plugin_runtime
                .run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }
    }

    // Run global plugins (D3-A gap fill from unified snapshot Phase 2).
    // Order: route → backend → global (onion: response stages go inner → outer).
    if ctx.local_reply_header.is_none() {
        if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(std::sync::Arc::clone) {
            for runtime in plugins.iter() {
                if ctx.local_reply_header.is_some() {
                    break;
                }
                if runtime.upstream_response_plugins_count() == 0 {
                    continue;
                }
                runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
            }
        }
    }

    Ok(())
}
