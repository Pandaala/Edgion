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
    entry.ext = crate::observe::access_log::collect(edgion_http, session, ctx);
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
            .correlation_id("x-trace-id")
            .or_else(|| ctx.request_info.trace_id.standard_trace_id())
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
    chars / 4 + if chars % 4 == 0 { 0 } else { 1 }
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

    let entry = match handles.lookup(gateway_info.gateway_namespace(), gateway_info.gateway_name()) {
        Some(e) => e,
        None => return false,
    };

    let bc = match ctx.backend_context.as_ref() {
        Some(bc) => bc,
        None => return false,
    };
    let b_idx = match bc.backend_ref_idx {
        Some(i) => i,
        None => return false,
    };

    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");
    let p_idx = match protocol_to_idx(protocol) {
        Some(p) => p,
        None => return false,
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
mod tests {
    //! Unit tests for the g1-007 LB runtime ledger cleanup contract.
    //!
    //! These tests verify the invariants of [`lb_runtime_cleanup`] without
    //! standing up a Pingora session: by directly mutating `EdgionHttpContext`
    //! fields and calling per-backend atomic methods we reproduce the exact
    //! stage sequence that the proxy would emit.
    use super::*;
    use crate::lb::backend::LbBackend;
    use crate::lb::commit_attempt;
    use crate::BackendContext;
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn attached_owner() -> Arc<crate::plugins::http::ext_proc::AttachedExtProc> {
        Arc::new(crate::plugins::http::ext_proc::AttachedExtProc::test_lazy())
    }

    #[tokio::test]
    async fn attached_summary_finalizes_top_level_token_once() {
        use crate::plugins::runtime::log::{PluginLog, StageLogs};

        let identity = "route/requestPlugins/#0".to_string();
        let owner = attached_owner();
        let mut log = PluginLog::new("ExtProc");
        log.set_internal_token(identity.clone());
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.install_attached_ext_proc(identity, owner, false));
        ctx.push_stage_log(StageLogs {
            stage: "request_filters",
            filters: vec![log],
            edgion_plugins: Vec::new(),
        });

        finalize_attached_ext_proc_logs(&mut ctx).await;
        finalize_attached_ext_proc_logs(&mut ctx).await;
        let rendered = serde_json::to_string(&ctx.stage_logs()[0].filters[0]).unwrap();
        assert_eq!(rendered.matches("ext_proc attached").count(), 1);
        assert!(rendered.contains("reason=logging-fallback"));
    }

    #[tokio::test]
    async fn attached_owner_without_stage_log_still_terminalizes() {
        let identity = "route/requestPlugins/#0".to_string();
        let owner = attached_owner();
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.install_attached_ext_proc(identity, Arc::clone(&owner), false));

        finalize_attached_ext_proc_logs(&mut ctx).await;

        assert_eq!(owner.test_state(), ("cancelled", "logging-fallback", true));
        assert!(ctx.stage_logs().is_empty());
    }

    #[tokio::test]
    async fn attached_summary_finds_nested_extension_ref_token_once() {
        use crate::plugins::runtime::log::{EdgionPluginsLog, PluginLog, StageLogs};

        let identity = "extension/ref/requestPlugins/#0".to_string();
        let owner = attached_owner();
        let mut log = PluginLog::new("ExtProc");
        log.set_internal_token(identity.clone());
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.install_attached_ext_proc(identity, owner, false));
        ctx.push_stage_log(StageLogs {
            stage: "request_filters",
            filters: Vec::new(),
            edgion_plugins: vec![EdgionPluginsLog {
                name: "extension-ref".to_string(),
                logs: vec![log],
            }],
        });

        finalize_attached_ext_proc_logs(&mut ctx).await;
        finalize_attached_ext_proc_logs(&mut ctx).await;
        let rendered = serde_json::to_string(&ctx.stage_logs()[0].edgion_plugins[0].logs[0]).unwrap();
        assert_eq!(rendered.matches("ext_proc attached").count(), 1);
        assert!(rendered.contains("reason=logging-fallback"));
    }

    fn make_backend(ip: [u8; 4], port: u16) -> Arc<LbBackend> {
        Arc::new(LbBackend::new(Backend {
            addr: SocketAddr::Inet(std::net::SocketAddr::from((ip, port))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    /// Push and commit a new upstream entry through the production ownership path.
    fn setup_committed_attempt(
        ctx: &mut EdgionHttpContext,
        backend: Arc<LbBackend>,
        policy: edgion_resources::resources::lb_policy::ParsedLBPolicy,
        start_time: Instant,
    ) {
        if ctx.backend_context.is_none() {
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
        }
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
            lb_backend: Some(backend),
            lb_outlier_completion: None,
            service_key: None,
            lb_policy: Some(policy),
            tls: None,
            lb_committed: None,
        });
        bc.current_upstream_id = Some(bc.upstreams.len() - 1);
        commit_attempt(bc.upstreams.last_mut().unwrap());
    }

    #[test]
    fn cleanup_no_increment_does_not_touch_other_inflight() {
        let a = make_backend([127, 0, 0, 1], 30001);
        // Another request has an in-flight on A.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        // This request: an upstream entry exists but connect failed — lb_committed never set.
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
        ctx.push_upstream(None, None);
        // lb_committed stays None (default from push_upstream)

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.conn(),
            1,
            "cleanup with lb_committed=None must not touch another in-flight count"
        );
    }

    #[test]
    fn cleanup_after_connected_balances_to_zero() {
        let a = make_backend([127, 0, 0, 2], 30001);

        let mut ctx = EdgionHttpContext::new();
        setup_committed_attempt(
            &mut ctx,
            a.clone(),
            edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
            Instant::now(),
        );
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert!(
            ctx.get_current_upstream().unwrap().lb_committed.is_none(),
            "lb_committed must be taken by cleanup"
        );
    }

    #[test]
    fn retry_then_connected_then_logging_balances_both() {
        use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        let a = make_backend([127, 0, 0, 3], 30001);
        let b = make_backend([127, 0, 0, 4], 30001);

        let mut ctx = EdgionHttpContext::new();

        // Attempt 1: connected to A.
        setup_committed_attempt(
            &mut ctx,
            a.clone(),
            edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
            Instant::now(),
        );
        assert_eq!(a.conn(), 1);

        // Retry: begin_upstream_attempt finalizes attempt 1 with Discard (dec A),
        // then pushes an empty slot for attempt 2.
        begin_upstream_attempt(&mut ctx);
        assert_eq!(a.conn(), 0);
        assert!(
            ctx.backend_context.as_ref().unwrap().upstreams[0]
                .lb_committed
                .is_none(),
            "begin_upstream_attempt must consume upstream 0's lb_committed"
        );

        // Attempt 2: connected to B.
        if let Some(up) = ctx.get_current_upstream_mut() {
            up.lb_backend = Some(b.clone());
            up.lb_policy = Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn);
            commit_attempt(up);
        }
        assert_eq!(b.conn(), 1);

        // Logging cleanup: settles attempt 2 (dec B).
        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert_eq!(b.conn(), 0);
    }

    #[test]
    fn cleanup_records_no_latency_sample() {
        use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        // The sample moved to `pg_upstream_response_filter` (response headers).
        // Request-end cleanup must only balance the outstanding-connection
        // count — it must never touch the score, in either mode.
        let a = make_backend([127, 0, 0, 5], 30001);
        let now = crate::lb::ewma::score::now_units();
        let initial = a.latency_estimate(now);

        let mut ctx = EdgionHttpContext::new();
        setup_committed_attempt(
            &mut ctx,
            a.clone(),
            edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma,
            Instant::now(),
        );

        // Retry finalizes attempt 1; its guard is consumed.
        begin_upstream_attempt(&mut ctx);
        assert!(
            ctx.backend_context.as_ref().unwrap().upstreams[0]
                .lb_committed
                .is_none(),
            "retry must consume upstream 0's guard"
        );

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.latency_estimate(now),
            initial,
            "neither retry-finalize nor request-end cleanup may score"
        );
        assert_eq!(a.conn(), 0, "but both must balance the connection count");
    }

    #[test]
    fn ewma_policy_balances_conn_like_leastconn() {
        // Ewma now maintains conn_count too, because its score is
        // `latency_estimate * (conn + 1)`.
        let a = make_backend([127, 0, 0, 6], 30001);
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
        ctx.push_upstream(None, None);
        if let Some(up) = ctx.get_current_upstream_mut() {
            up.lb_backend = Some(a.clone());
            up.lb_policy = Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma);
            up.start_time = Instant::now() - Duration::from_millis(50);
            commit_attempt(up);
        }
        assert_eq!(a.conn(), 1, "Ewma commit must increment");

        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 0, "cleanup must decrement");
    }

    #[test]
    fn cleanup_idempotent_on_double_call() {
        let a = make_backend([127, 0, 0, 7], 30001);

        let mut ctx = EdgionHttpContext::new();
        setup_committed_attempt(
            &mut ctx,
            a.clone(),
            edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
            Instant::now(),
        );
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 0);

        // Simulate a separate concurrent in-flight on A.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        // Second cleanup: lb_committed was taken; must be a no-op.
        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 1, "second cleanup must not decrement again");
    }
}

#[cfg(test)]
mod genai_tests {
    use super::*;
    use crate::ai::{
        AiProviderLabel, AiRequestContext, AiUsageShared, GenAiAttempt, GenAiAttemptFailure, GenAiAttemptUsageSource,
        UsageExtractionProcessor,
    };
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::StreamingResponseProcessor;
    use crate::BackendContext;
    use bytes::Bytes;
    use edgion_resources::resources::edgion_backend::AiModelPricing;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::sync::Arc;

    fn ai_context(provider: AiProviderLabel, model: &str, requested_streaming: bool) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.ensure_ai_state().request = Some(AiRequestContext {
            provider,
            model: model.to_string(),
            pricing: None,
            requested_streaming,
            usage: Arc::new(AiUsageShared::default()),
            quota: None,
            quota_status: None,
        });
        ctx
    }

    async fn observe_response(ctx: &EdgionHttpContext, body: &[u8], eos: bool, kind: ResponseStreamKind) {
        let shared = ctx.ai_state().unwrap().request.as_ref().unwrap().usage.clone();
        let mut processor = UsageExtractionProcessor::new(shared);
        let mut chunk = Some(Bytes::copy_from_slice(body));
        let mut session = MockPluginSession::new();
        session.expect_semantic_guard_active().return_const(false);
        let mut sink = pingora_proxy::ResponseBodySink::new();
        processor.on_chunk(&mut chunk, eos, kind, &mut sink, &mut session).await;
        assert_eq!(chunk.as_deref(), Some(body), "usage observation must not mutate body");
    }

    fn init_backend_context(ctx: &mut EdgionHttpContext) {
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
    }

    #[test]
    fn ceil_character_estimate_is_overflow_safe() {
        assert_eq!(ceil_chars_div_four(0), 0);
        assert_eq!(ceil_chars_div_four(1), 1);
        assert_eq!(ceil_chars_div_four(4), 1);
        assert_eq!(ceil_chars_div_four(5), 2);
        assert_eq!(ceil_chars_div_four(u64::MAX), u64::MAX / 4 + 1);
    }

    #[tokio::test]
    async fn finalize_uses_reported_usage_and_actual_sse_classification() {
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        observe_response(
            &ctx,
            b"data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7}}\n\n",
            true,
            ResponseStreamKind::Sse,
        )
        .await;
        ctx.response_stream_kind = ResponseStreamKind::Sse;
        finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

        let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
        assert_eq!((summary.input_tokens, summary.output_tokens), (12, 7));
        assert!(!summary.usage_estimated);
        assert!(
            summary.streaming,
            "actual response classification must win over requested=false"
        );
    }

    #[tokio::test]
    async fn finalize_prices_ordinary_cached_and_output_tokens_without_float() {
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        ctx.ensure_ai_state().request.as_mut().unwrap().pricing = Some(
            AiModelPricing {
                input_per_million_usd: "10".to_string(),
                output_per_million_usd: "20".to_string(),
                cache_read_input_per_million_usd: Some("1".to_string()),
                cache_write_input_per_million_usd: Some("2".to_string()),
            }
            .parsed()
            .unwrap(),
        );
        observe_response(
            &ctx,
            br#"{"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":20},"cache_creation_input_tokens":10}}"#,
            true,
            ResponseStreamKind::None,
        )
        .await;
        finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

        let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
        assert_eq!(summary.cache_read_input_tokens, 20);
        assert_eq!(summary.cache_write_input_tokens, 10);
        assert_eq!(summary.estimated_cost_usd.as_deref(), Some("0.00084"));
        let json = crate::observe::AccessLogEntry::from_context(&ctx).to_json();
        assert!(json.contains(r#""estimated_cost_usd":"0.00084""#));
    }

    #[test]
    fn finalize_preserves_quota_admission_bypass_status() {
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "model-a", false);
        let ai = ctx.ensure_ai_state().request.as_mut().unwrap();
        ai.quota_status = Some("fail_open");
        ai.usage.observe_reported_usage(2, 3, 0, 0);

        finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

        assert_eq!(
            ctx.ai_state().unwrap().summary.as_ref().unwrap().quota_status,
            Some("fail_open")
        );
    }

    #[tokio::test]
    async fn finalize_estimates_partial_aborted_stream_and_emits_metrics_once() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let mut ctx = ai_context(AiProviderLabel::OpenAiCompatible, "other", true);
        ctx.ai_state()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
            .usage
            .set_input_chars(5);
        observe_response(&ctx, b"data: hello", false, ResponseStreamKind::Sse).await;
        ctx.response_stream_kind = ResponseStreamKind::Sse;
        metrics::with_local_recorder(&recorder, || {
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        });

        let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
        assert_eq!((summary.input_tokens, summary.output_tokens), (2, 2));
        assert!(summary.usage_estimated);
        assert!(summary.streaming);

        let rendered = handle.render();
        let samples: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
            .collect();
        assert_eq!(
            samples.len(),
            2,
            "aborted request must emit one input/output pair:\n{rendered}"
        );
        assert!(samples.iter().all(|line| line.ends_with(" 2")));
    }

    #[test]
    fn ttft_uses_only_current_attempt_and_is_absent_without_its_body() {
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        init_backend_context(&mut ctx);
        ctx.push_upstream(None, None);
        ctx.get_current_upstream_mut().unwrap().bt = Some(10);
        ctx.push_upstream(None, None);
        finalize_ai_usage_with_metrics_enabled(&mut ctx, false);
        assert_eq!(ctx.ai_state().unwrap().summary.as_ref().unwrap().ttft_ms, None);

        let mut with_body = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        init_backend_context(&mut with_body);
        with_body.push_upstream(None, None);
        with_body.get_current_upstream_mut().unwrap().bt = Some(10);
        with_body.push_upstream(None, None);
        with_body.get_current_upstream_mut().unwrap().bt = Some(23);
        finalize_ai_usage_with_metrics_enabled(&mut with_body, false);
        assert_eq!(
            with_body.ai_state().unwrap().summary.as_ref().unwrap().ttft_ms,
            Some(23)
        );
    }

    #[tokio::test]
    async fn finalize_is_idempotent_and_emits_metrics_once() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        observe_response(
            &ctx,
            br#"{"usage":{"prompt_tokens":12,"completion_tokens":7}}"#,
            true,
            ResponseStreamKind::None,
        )
        .await;

        metrics::with_local_recorder(&recorder, || {
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
            let first = ctx.ai_state().unwrap().summary.clone();
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
            assert_eq!(ctx.ai_state().unwrap().summary, first);
        });
        let rendered = handle.render();
        let samples: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
            .collect();
        assert_eq!(
            samples.len(),
            2,
            "one request must create only input/output series:\n{rendered}"
        );
        assert!(samples.iter().any(|line| line.ends_with(" 12")));
        assert!(samples.iter().any(|line| line.ends_with(" 7")));
    }

    #[test]
    fn retry_metrics_sum_all_attempts_and_emit_once() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        for (number, input_tokens, output_tokens, failure) in
            [(1, 3, 4, Some(GenAiAttemptFailure::RetryableStatus)), (2, 5, 7, None)]
        {
            ctx.ensure_ai_state().attempt_runtime.record_test_attempt(
                GenAiAttempt {
                    number,
                    backend: "service-a".to_string(),
                    provider: "openai",
                    credential: "primary".to_string(),
                    status: Some(200),
                    failure,
                    retry_after_seconds: None,
                    ttft_ms: None,
                    usage_source: GenAiAttemptUsageSource::Reported,
                    input_tokens,
                    output_tokens,
                    estimated_cost_usd: None,
                },
                AiProviderLabel::OpenAi,
            );
        }

        metrics::with_local_recorder(&recorder, || {
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
            finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        });

        let rendered = handle.render();
        let samples: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
            .collect();
        assert_eq!(
            samples.len(),
            2,
            "attempts must share fixed-label input/output series:\n{rendered}"
        );
        assert!(samples.iter().any(|line| line.ends_with(" 8")));
        assert!(samples.iter().any(|line| line.ends_with(" 11")));
        assert!(samples.iter().all(|line| line.contains("provider=\"openai\"")));
        assert!(samples.iter().all(|line| line.contains("usage_source=\"reported\"")));
    }

    #[test]
    fn disabled_or_non_ai_finalization_never_registers_metric_family() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut ai = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
            finalize_ai_usage_with_metrics_enabled(&mut ai, false);
            let mut ordinary = EdgionHttpContext::new();
            finalize_ai_usage_with_metrics_enabled(&mut ordinary, true);
            assert!(ordinary.ai_state().is_none());
        });
        assert!(!handle
            .render()
            .contains(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL));
    }

    #[tokio::test]
    async fn access_log_genai_shape_contains_no_request_response_or_key_text() {
        let prompt = "PROMPT-FIXTURE-DO-NOT-LOG";
        let completion = "COMPLETION-FIXTURE-DO-NOT-LOG";
        let api_key = "API-KEY-FIXTURE-DO-NOT-LOG";
        let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        ctx.ai_state()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
            .usage
            .set_input_chars(prompt.chars().count() as u64);
        let event = format!("data: {completion}");
        observe_response(&ctx, event.as_bytes(), false, ResponseStreamKind::Sse).await;
        ctx.ensure_ai_state().attempt_runtime.record_test_attempt(
            GenAiAttempt {
                number: 1,
                backend: "default/service-a".to_string(),
                provider: "openai",
                credential: "primary".to_string(),
                status: Some(200),
                failure: None,
                retry_after_seconds: Some(3),
                ttft_ms: Some(11),
                usage_source: GenAiAttemptUsageSource::Estimated,
                input_tokens: 4,
                output_tokens: 5,
                estimated_cost_usd: Some("0.00001".to_string()),
            },
            AiProviderLabel::OpenAi,
        );
        finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

        let json = crate::observe::AccessLogEntry::from_context(&ctx).to_json();
        assert!(!json.contains(prompt));
        assert!(!json.contains(completion));
        assert!(!json.contains(api_key));
        assert!(!json.contains("authorization"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let genai = value["genai"].as_object().unwrap();
        assert_eq!(genai["provider"], "openai");
        assert_eq!(genai["model"], "gpt-declared");
        assert_eq!(genai["streaming"], false);
        assert!(genai.get("ttft_ms").is_none());
        let keys: std::collections::BTreeSet<&str> = genai.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "input_tokens",
                "model",
                "output_tokens",
                "provider",
                "streaming",
                "usage_estimated",
                "attempts",
            ]
            .into_iter()
            .collect()
        );
        let attempts = genai["attempts"].as_array().expect("attempt array");
        assert_eq!(attempts.len(), 1);
        let attempt = attempts[0].as_object().expect("attempt object");
        assert_eq!(attempt["backend"], "default/service-a");
        assert_eq!(attempt["credential"], "primary");
        assert_eq!(
            attempt
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "credential",
                "estimated_cost_usd",
                "input_tokens",
                "number",
                "output_tokens",
                "provider",
                "retry_after_seconds",
                "backend",
                "status",
                "ttft_ms",
                "usage_source",
            ]
            .into_iter()
            .collect()
        );

        let ordinary = EdgionHttpContext::new();
        let ordinary_json = crate::observe::AccessLogEntry::from_context(&ordinary).to_json();
        assert!(!ordinary_json.contains("\"genai\""));
    }
}
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
