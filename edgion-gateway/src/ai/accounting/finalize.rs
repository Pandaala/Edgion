//! Attempt, usage and quota settlement at universal terminal logging.

use crate::ai::{estimated_cost_usd, GenAiAttemptUsageSource, GenAiSummary, ReportedAiUsage};
use crate::cli::config::get_genai_tokens_config;
use crate::ctx::ResponseStreamKind;
use crate::observe::metrics::{record_genai_tokens, GenAiUsageSource};
use crate::EdgionHttpContext;

pub(crate) async fn finalize(ctx: &mut EdgionHttpContext) {
    let ai_ttft_ms = ctx.get_current_upstream().and_then(|upstream| upstream.bt);
    if let Some(state) = ctx.ai_state_mut() {
        state.attempt_runtime.finalize(ai_ttft_ms).await;
    }
    finalize_ai_usage(ctx);
    finalize_ai_quota(ctx).await;
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

#[cfg(test)]
#[path = "../../../tests/unit/ai/accounting/finalize_test.rs"]
mod tests;
