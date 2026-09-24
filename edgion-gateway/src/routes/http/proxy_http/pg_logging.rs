use super::EdgionHttpProxy;
use crate::ai::terminal::{ResponseClose, ResponseTerminalState};
use crate::ai::{estimated_cost_usd, GenAiAttemptUsageSource, GenAiSummary, ReportedAiUsage};
use crate::cli::config::{get_backend_requests_config, get_genai_tokens_config, get_request_duration_config};
use crate::ctx::ResponseStreamKind;
use crate::link_sys::LogMeta;
use crate::observe::metrics::{
    global_metrics, record_backend_request, record_genai_tokens, record_request_duration, status_group,
    GenAiUsageSource, TestData, TestType,
};
use crate::observe::AccessLogEntry;
use crate::runtime::GatewayInfo;
use crate::EdgionHttpContext;
use pingora_core::{Error as PingoraError, ErrorSource};
use pingora_proxy::Session;
use std::time::Instant;

#[inline]
pub async fn logging(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    _e: Option<&PingoraError>,
    ctx: &mut EdgionHttpContext,
) {
    // Pingora invokes this exactly once from `finish` as the universal terminal
    // hook, including fatal errors and locally terminated requests.
    // Acquire before any await so unwind or cancellation inside logging also
    // releases the active-context gauge charge.
    let _active_context_release = global_metrics().active_context_release_guard();

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
        let wpt = session.upstream_write_pending_time();
        if !wpt.is_zero() {
            upstream.wpt = Some(wpt.as_millis() as u64);
        }
    }
    global_metrics().add_request_bytes(session.body_bytes_read() as u64);
    global_metrics().add_response_bytes(session.upstream_body_bytes_received() as u64);
    // g1-007: balance the LB runtime ledger using the explicit fields written by
    // `connected_to_upstream` (see `lb_runtime_cleanup` for the full contract).
    lb_runtime_cleanup(ctx);

    let session_status = session.response_written().map(|header| header.status.as_u16());
    ctx.finalize_downstream_status(session_status);

    let ai_ttft_ms = ctx.get_current_upstream().and_then(|upstream| upstream.bt);
    if let Some(state) = ctx.ai_state_mut() {
        state.attempt_runtime.finalize(ai_ttft_ms).await;
    }
    finalize_ai_usage(ctx);
    finalize_ai_quota(ctx).await;
    record_request_metrics(ctx, _e);

    finalize_response_terminal_for_logging(ctx, _e.map(PingoraError::esource).cloned());

    // Streamed body handlers whose pump never reached EOS (client abort,
    // early upstream response) still carry log entries — sweep them into the
    // stage logs before the access-log entry is assembled.
    super::pg_request_body_filter_action::drain_leftover_stream_logs(ctx);
    super::pg_request_body_filter_action::drain_request_body_observer_logs(ctx);
    let (outcome, reason) = match _e.map(PingoraError::esource) {
        Some(ErrorSource::Upstream) => ("upstream-abort", "stream-error"),
        Some(ErrorSource::Downstream) | None => ("cancelled", "downstream-cancel"),
        Some(ErrorSource::Internal | ErrorSource::Unset) => ("fail-close", "internal-stream-error"),
    };
    if let Some(driver) = ctx.response_processor_driver() {
        driver.finalize_semantic_incomplete(outcome, reason).await;
    } else if let Some(processor) = ctx.ai_state_mut().and_then(|state| state.semantic_processor.as_mut()) {
        // Local replies and response-stage denials can finish before the final
        // response header freezes the builder slots into a driver.
        processor.finalize_incomplete(outcome, reason);
    }
    finalize_attached_ext_proc_logs(ctx).await;
    finalize_attached_guardrail_log(ctx);

    ctx.finalize_waf_for_logging();
    let mut entry = AccessLogEntry::from_context(ctx);
    entry.waf = ctx.waf_access_log();
    // Resolve operator-declared `ext` fields here rather than in a request-stage
    // filter: this is the only point every request reaches, including the ones
    // that short-circuited before any plugin ran and the ones a Deny terminated.
    entry.ext = crate::observe::access_log::collect(session, ctx);
    // Extract LogMeta BEFORE serialization — typed fields needed by OTLP sender.
    // entry.timestamp is i64 milliseconds; guard negative values before casting.
    let status_code = entry.request_info.status_option();
    let meta = LogMeta {
        trace_id: ctx.request_info.trace_id.typed_trace_id(),
        timestamp_ns: entry.timestamp.max(0) as u64 * 1_000_000,
        severity_number: LogMeta::severity_from_status(status_code),
        status_code,
        method: Some(session.req_header().method.as_str()),
        path: Some(ctx.path()),
    };
    let entry_json = entry.to_json();

    // Clone only when integration testing needs the log stored; production path is zero-copy.
    let entry_json_for_store = if edgion_common::test_mode::is_integration_testing_mode() {
        let should_store = session
            .req_header()
            .headers
            .get("access_log")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "test_store")
            .unwrap_or(false);
        if should_store {
            Some(entry_json.clone())
        } else {
            None
        }
    } else {
        None
    };

    edgion_http.access_logger.send_with_meta(entry_json, &meta).await;

    // Defensive cleanup for mirror state. Dropping JoinHandle detaches task.
    if let Some(mirror_state) = ctx.take_mirror_state() {
        drop(mirror_state);
    }

    if let Some(json_for_store) = entry_json_for_store {
        let store = crate::observe::access_log_store::get_access_log_store();
        let trace_key = ctx
            .request_info
            .trace_id
            .standard_trace_id()
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Err(e) = store.store(trace_key, json_for_store) {
            tracing::warn!( // tracing-allow: access-log store send failure (registered exception, see 01-log-safety.md)
                component = "access_log_store",
                error = %e,
                "Failed to store access log"
            );
            global_metrics().access_log_dropped();
        }
    }
}

async fn finalize_attached_ext_proc_logs(ctx: &mut EdgionHttpContext) {
    super::pg_upstream_response_trailer_filter::finalize_incomplete_trailer_companions(ctx).await;
    let Some((stages, attached)) = ctx.stage_logs_and_attached_ext_proc() else {
        return;
    };
    for (identity, owner) in attached {
        owner.terminalize_for_logging();
        let Some(line) = owner.take_summary_line() else {
            continue;
        };
        'stages: for stage in stages.iter_mut() {
            for log in &mut stage.filters {
                if log.internal_token.as_deref() == Some(identity.as_str()) {
                    log.push(&line);
                    break 'stages;
                }
            }
            for resource in &mut stage.edgion_plugins {
                for log in &mut resource.logs {
                    if log.internal_token.as_deref() == Some(identity.as_str()) {
                        log.push(&line);
                        break 'stages;
                    }
                }
            }
        }
    }
}

fn finalize_attached_guardrail_log(ctx: &mut EdgionHttpContext) {
    let Some((identity, owner)) = ctx.ai_state_mut().and_then(|state| state.attached_guardrail.take()) else {
        return;
    };
    let Some(line) = owner.take_summary_line() else {
        return;
    };
    let stages = ctx.stage_logs_mut();
    if stages.is_empty() {
        return;
    }
    'stages: for stage in stages.iter_mut() {
        for log in &mut stage.filters {
            if log.internal_token.as_deref() == Some(identity.as_str()) {
                log.push(&line);
                break 'stages;
            }
        }
        for resource in &mut stage.edgion_plugins {
            for log in &mut resource.logs {
                if log.internal_token.as_deref() == Some(identity.as_str()) {
                    log.push(&line);
                    break 'stages;
                }
            }
        }
    }
}

/// Finalize AI usage once at the request's universal logging boundary.
#[inline]
fn finalize_ai_usage(ctx: &mut EdgionHttpContext) {
    finalize_ai_usage_with_metrics_enabled(ctx, get_genai_tokens_config().enabled);
}

/// Testable finalizer with the restart-bound metric switch passed explicitly,
/// avoiding process-global `OnceLock` mutation in parallel unit tests.
#[inline]
fn finalize_ai_usage_with_metrics_enabled(ctx: &mut EdgionHttpContext, metrics_enabled: bool) {
    let streaming = ctx.response_stream_kind == ResponseStreamKind::Sse;
    let ttft_ms = ctx.get_current_upstream().and_then(|upstream| upstream.bt);
    let Some(state) = ctx.ai_state_mut() else {
        return;
    };
    if state.summary.is_some() {
        return;
    }
    if state.attempt_plan.is_some() && !state.attempt_runtime.ever_dispatched() {
        return;
    }
    let Some(ai) = state.request.as_ref() else {
        return;
    };

    let provider = ai.provider;
    let model = ai.model.clone();
    let (usage, usage_source) = match ai.usage.reported_usage() {
        Some(usage) => (usage, GenAiUsageSource::Reported),
        None => (
            ReportedAiUsage {
                input_tokens: ceil_chars_div_four(ai.usage.input_chars()),
                output_tokens: ceil_chars_div_four(ai.usage.output_chars()),
                cache_read_input_tokens: 0,
                cache_write_input_tokens: 0,
            },
            GenAiUsageSource::Estimated,
        ),
    };
    // Store the immutable summary before metric emission. Its presence is the
    // commit sentinel, so re-entry can never increment the counters twice.
    let summary = GenAiSummary {
        provider: provider.as_str(),
        model,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        streaming,
        usage_estimated: usage_source == GenAiUsageSource::Estimated,
        estimated_cost_usd: ai.pricing.map(|pricing| estimated_cost_usd(pricing, usage)),
        quota_status: ai.quota_status,
        ttft_ms,
        attempts: state.attempt_runtime.attempts().to_vec(),
        attempts_truncated: state.attempt_runtime.attempts_truncated(),
    };
    state.summary = Some(summary);
    if state.attempt_runtime.ever_dispatched() {
        if let Ok(totals) = state.attempt_runtime.metric_totals() {
            for total in totals {
                let source = match total.usage_source {
                    GenAiAttemptUsageSource::Reported => GenAiUsageSource::Reported,
                    GenAiAttemptUsageSource::Estimated => GenAiUsageSource::Estimated,
                };
                record_genai_tokens(
                    metrics_enabled,
                    total.provider,
                    source,
                    total.input_tokens,
                    total.output_tokens,
                );
            }
        }
    } else {
        record_genai_tokens(
            metrics_enabled,
            provider,
            usage_source,
            usage.input_tokens,
            usage.output_tokens,
        );
    }
}

async fn finalize_ai_quota(ctx: &mut EdgionHttpContext) {
    let Some(state) = ctx.ai_state_mut() else {
        return;
    };
    let Some(summary) = state.summary.as_ref() else {
        return;
    };
    let authoritative_total = if state.attempt_runtime.ever_dispatched() {
        state.attempt_runtime.charged_request_tokens().ok()
    } else {
        summary.input_tokens.checked_add(summary.output_tokens)
    };
    let Some(authoritative_total) = authoritative_total else {
        if let Some(summary) = state.summary.as_mut() {
            summary.quota_status = Some("accounting_overflow");
        }
        ctx.record_error(edgion_resources::EdgionErrorCode::AiQuotaReconciliationFailed);
        return;
    };
    let Some(quota) = state.request.as_mut().and_then(|ai| ai.quota.as_mut()) else {
        return;
    };
    let status = quota.finalize(authoritative_total).await;
    if let Some(summary) = state.summary.as_mut() {
        summary.quota_status = Some(status.as_str());
    }
    if matches!(
        status,
        crate::ai::quota::QuotaFinalStatus::Failed | crate::ai::quota::QuotaFinalStatus::TimedOut
    ) {
        ctx.record_error(edgion_resources::EdgionErrorCode::AiQuotaReconciliationFailed);
    }
}

#[inline]
const fn ceil_chars_div_four(chars: u64) -> u64 {
    chars / 4 + if chars.is_multiple_of(4) { 0 } else { 1 }
}

#[inline]
fn record_request_metrics(ctx: &EdgionHttpContext, error: Option<&PingoraError>) {
    let duration_ms = Instant::now().saturating_duration_since(ctx.start_time).as_secs_f64() * 1000.0;

    if try_record_via_handles(ctx, duration_ms) {
        return;
    }

    // Fallback: integration-testing mode, route miss, bypass path,
    // gateway not in the route's attached set, or unknown protocol.
    record_request_metrics_fallback(ctx, error, duration_ms);
}

#[inline]
fn try_record_via_handles(ctx: &EdgionHttpContext, duration_ms: f64) -> bool {
    use crate::observe::metrics::route_handles::{protocol_to_idx, ps_index, status_to_idx, RouteMetricHandles};

    if edgion_common::test_mode::is_integration_testing_mode() {
        return false;
    }

    let handles: &RouteMetricHandles = match (&ctx.route_unit, &ctx.grpc_route_unit) {
        (Some(r), _) => &r.metric_handles,
        (None, Some(g)) => &g.metric_handles,
        (None, None) => return false,
    };

    let Some(gateway_info) = ctx.gateway_info.as_deref() else {
        return false;
    };

    let Some(entry) = handles.lookup(gateway_info.gateway_namespace(), gateway_info.gateway_name()) else {
        return false;
    };

    let Some(bc) = ctx.backend_context.as_ref() else {
        return false;
    };
    let Some(b_idx) = bc.backend_ref_idx else {
        return false;
    };

    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");
    let Some(p_idx) = protocol_to_idx(protocol) else {
        return false;
    };
    let s_idx = status_to_idx(ctx.request_info.status_option());
    let idx = ps_index(p_idx, s_idx);

    // Graceful degradation on a stale-handle / backend-index mismatch: `record`
    // returns false so the caller falls back to the dynamic metrics path (the
    // drift stays visible in the access log). No panic on the data-plane hot path.
    entry.record(b_idx, idx, duration_ms)
}

/// Dynamic fallback path. Same code as the previous `record_request_metrics`
/// body — kept verbatim so route-miss / bypass / integration-testing-mode
/// requests continue to emit the same time series.
#[inline]
fn record_request_metrics_fallback(ctx: &EdgionHttpContext, error: Option<&PingoraError>, duration_ms: f64) {
    let gateway_ns = match ctx.gateway_info.as_deref().map(GatewayInfo::gateway_namespace) {
        None | Some("") => "unknown",
        Some(ns) => ns,
    };
    let gateway_name = match ctx.gateway_info.as_deref().map(GatewayInfo::gateway_name) {
        None | Some("") => "unknown",
        Some(name) => name,
    };

    let (route_ns, route_name) = if let Some(ref route_unit) = ctx.route_unit {
        (
            route_unit.matched_info.rsc.ns.as_str(),
            route_unit.matched_info.rsc.name.as_str(),
        )
    } else if let Some(ref grpc_unit) = ctx.grpc_route_unit {
        (
            grpc_unit.matched_info.rsc.ns.as_str(),
            grpc_unit.matched_info.rsc.name.as_str(),
        )
    } else {
        ("unknown", "unknown")
    };

    let (backend_ns, backend_name) = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.rsc.as_ref())
        .map(|rsc| (rsc.ns.as_str(), rsc.name.as_str()))
        .unwrap_or(("unknown", "unknown"));

    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");

    let (test_key, test_data) = if edgion_common::test_mode::is_integration_testing_mode() {
        let key = ctx
            .gateway_info
            .as_deref()
            .and_then(|info| info.metrics_test_key.as_deref())
            .unwrap_or("");
        let data = build_test_data(ctx, error);
        (key, data)
    } else {
        ("", String::new())
    };

    let status = status_group(ctx.request_info.status_option());
    record_backend_request(
        &get_backend_requests_config().labels,
        gateway_ns,
        gateway_name,
        route_ns,
        route_name,
        backend_ns,
        backend_name,
        protocol,
        status,
        test_key,
        &test_data,
    );

    record_request_duration(
        get_request_duration_config(),
        gateway_ns,
        gateway_name,
        route_ns,
        route_name,
        backend_ns,
        backend_name,
        protocol,
        status,
        duration_ms,
    );
}

/// Balance the final attempt's outstanding-connection count at request end.
///
/// Earlier attempts were finalized by `begin_upstream_attempt` /
/// `begin_ai_peer_attempt` on each retry, so only the last upstream entry can
/// still hold a live guard here. Idempotent: a second call sees `None` from
/// `take()` inside `finalize_attempt`, and an unresolved guard balances itself
/// on drop.
///
/// This no longer records the latency sample — that moved to
/// `pg_upstream_response_filter`, at response headers.
#[inline]
fn lb_runtime_cleanup(ctx: &mut EdgionHttpContext) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::lb::finalize_attempt(upstream);
    }
}

/// Build test data based on test type from Gateway annotations
///
/// Returns empty string if test mode is not enabled.
/// All data is collected from ctx at logging stage.
#[inline]
fn build_test_data(ctx: &EdgionHttpContext, error: Option<&PingoraError>) -> String {
    let Some(test_type) = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.metrics_test_type.as_ref())
    else {
        return String::new();
    };

    let mut test_data = TestData::new();

    match test_type {
        TestType::Lb => {
            // LB test: collect backend IP, port from UpstreamInfo (saved by push_upstream)
            if let Some(upstream) = ctx.get_current_upstream() {
                test_data.ip = upstream.ip.clone();
                test_data.port = upstream.port;
            }
            test_data.hash_key = ctx.hash_key.clone();
        }
        TestType::Retry => {
            // Retry test: collect try count and error
            test_data.try_count = Some(ctx.try_cnt);
            test_data.error = error.map(|e| e.to_string());
        }
        TestType::Latency => {
            // Latency test: collect upstream latency
            if let Some(start) = ctx.upstream_start_time {
                test_data.latency_ms = Some(Instant::now().saturating_duration_since(start).as_millis() as u64);
            }
        }
        TestType::None => {
            return String::new();
        }
    }

    test_data.to_json()
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_logging_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_logging_test/genai_tests.rs"]
mod genai_tests;
pub(super) fn finalize_response_terminal_for_logging(ctx: &mut EdgionHttpContext, error_source: Option<ErrorSource>) {
    if matches!(ctx.response_terminal_state, ResponseTerminalState::Claimed { .. }) {
        ctx.response_terminal_state.settle_pending_without_event();
        ctx.response_terminal_state.close_gateway();
    } else {
        let close = match error_source {
            Some(ErrorSource::Upstream) => ResponseClose::UpstreamAbort,
            Some(ErrorSource::Downstream) | None => ResponseClose::DownstreamCancel,
            Some(ErrorSource::Internal | ErrorSource::Unset) => ResponseClose::UpstreamAbort,
        };
        ctx.response_terminal_state.close_open(close);
    }
}
