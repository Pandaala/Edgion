use super::EdgionHttpProxy;
use crate::core::gateway::routes::http::session_persistence;
use crate::types::resources::http_route::SessionPersistenceType;
use crate::types::{EdgionHttpContext, SessionPersistenceState};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

#[inline]
pub fn upstream_response_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    upstream_response: &mut ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Record status code
    let status_code = upstream_response.status.as_u16();
    ctx.request_info.status = Some(status_code);

    // Inject session persistence cookie/header if needed.
    match ctx.sp_state {
        SessionPersistenceState::InjectNew | SessionPersistenceState::InjectUpdate => {
            let sp_config = ctx
                .route_unit
                .as_ref()
                .and_then(|ru| ru.rule.session_persistence.clone());

            if let Some(sp) = sp_config {
                // Use ip:port from current upstream (backfilled by backfill_upstream_addr)
                let backend_addr = ctx.get_current_upstream().and_then(|u| match (&u.ip, u.port) {
                    (Some(ip), Some(port)) => Some(format!("{}:{}", ip, port)),
                    _ => None,
                });

                if let Some(addr) = backend_addr {
                    let token = session_persistence::generate_session_token(&addr);

                    match session_persistence::get_persistence_type(&sp) {
                        SessionPersistenceType::Cookie => {
                            let is_tls = session
                                .as_downstream()
                                .digest()
                                .map_or(false, |d| d.ssl_digest.is_some());
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

    // Apply custom server headers (including Server header)
    edgion_http.server_header_opts.apply_to_response(upstream_response);

    // Record header time (time from this upstream's start_time to receiving response header)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ht = upstream.start_time.elapsed().as_millis() as u64;
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

                    // Check request timeout
                    let timeout_exceeded = route_unit
                        .rule
                        .parsed_timeouts
                        .as_ref()
                        .and_then(|t| t.request_timeout.as_duration())
                        .map(|timeout| ctx.start_time.elapsed() >= timeout)
                        .unwrap_or(false);

                    let buffer_ok = !session.as_ref().retry_buffer_truncated();
                    if ctx.try_cnt <= max_retries && !timeout_exceeded && buffer_ok {
                        ctx.retry_on_status = true;
                        return Err(pingora_core::Error::explain(
                            pingora_core::ErrorType::HTTPStatus(status_code),
                            format!("retry on status {}", status_code),
                        ));
                    }
                }
            }
        }
    }

    // Record circuit breaker success/failure based on status code
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
            let cb_store = crate::core::gateway::backends::get_circuit_breaker_store();
            if status_code >= 500 {
                cb_store.record_failure_by_addr(service_key, addr);
            } else {
                cb_store.record_success_by_addr(service_key, addr);
            }
        }
    }

    // Run rule-level upstream_response edgion_plugins (sync)
    if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit
            .rule
            .plugin_runtime
            .run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    // Run backend-level upstream_response edgion_plugins (sync)
    if let Some(backend) = ctx.selected_backend.clone() {
        backend
            .plugin_runtime
            .run_upstream_response_plugins_sync(session, ctx, upstream_response);
    }

    // Run global plugins (D3-A gap fill from unified snapshot Phase 2).
    // Order: route → backend → global (onion: response stages go inner → outer).
    if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(std::sync::Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.upstream_response_plugins_count() == 0 {
                continue;
            }
            runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }
    }

    Ok(())
}
