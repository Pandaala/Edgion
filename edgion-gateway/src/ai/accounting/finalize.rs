//! Attempt, usage and quota settlement at universal terminal logging.

use crate::ai::{estimated_cost_usd, GenAiAttemptUsageSource, GenAiSummary};
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
    let mut snapshot = ai
        .usage
        .accounting_snapshot(ceil_chars_div_four(ai.usage.input_chars()), true);
    if let Some(source) = state.attempt_runtime.unresolved_usage_source() {
        snapshot.source = source;
        snapshot.usage = None;
    }
    if state.attempt_runtime.accounting_overflow() {
        snapshot.source = GenAiAttemptUsageSource::Unknown;
        snapshot.usage = None;
    }
    let unavailable_diagnostic = snapshot.unavailable.is_some() && ai.usage.claim_unavailable_diagnostic();
    let usage = snapshot.usage;
    let usage_source = snapshot.source;
    // Store the immutable summary before metric emission. Its presence is the
    // commit sentinel, so re-entry can never increment the counters twice.
    let summary = GenAiSummary {
        provider: provider.as_str(),
        model,
        input_tokens: usage.map(|usage| usage.input_tokens),
        output_tokens: usage.map(|usage| usage.output_tokens),
        cache_read_input_tokens: usage
            .and_then(|usage| (usage.cache_read_input_tokens != 0).then_some(usage.cache_read_input_tokens)),
        cache_write_input_tokens: usage
            .and_then(|usage| (usage.cache_write_input_tokens != 0).then_some(usage.cache_write_input_tokens)),
        streaming,
        usage_estimated: usage_source == GenAiAttemptUsageSource::Estimated,
        usage_source,
        estimated_cost_usd: usage.and_then(|usage| ai.pricing.map(|pricing| estimated_cost_usd(pricing, usage))),
        quota_status: ai.quota_status,
        ttft_ms,
        attempts: state.attempt_runtime.take_finalized_attempts(),
        attempts_truncated: state.attempt_runtime.attempts_truncated(),
    };
    state.summary = Some(summary);
    if state.attempt_runtime.ever_dispatched() {
        for total in state.attempt_runtime.metric_totals() {
            let source = match total.usage_source {
                GenAiAttemptUsageSource::Reported => GenAiUsageSource::Reported,
                GenAiAttemptUsageSource::Estimated => GenAiUsageSource::Estimated,
                GenAiAttemptUsageSource::Unknown | GenAiAttemptUsageSource::NotObserved => continue,
            };
            record_genai_tokens(
                metrics_enabled,
                total.provider,
                source,
                total.input_tokens,
                total.output_tokens,
            );
        }
    } else if let Some(usage) = usage {
        record_genai_tokens(
            metrics_enabled,
            provider,
            if usage_source == GenAiAttemptUsageSource::Reported {
                GenAiUsageSource::Reported
            } else {
                GenAiUsageSource::Estimated
            },
            usage.input_tokens,
            usage.output_tokens,
        );
    }
    if unavailable_diagnostic {
        ctx.record_error(edgion_resources::EdgionErrorCode::AiUsageObservationUnavailable);
    }
}

async fn finalize_ai_quota(ctx: &mut EdgionHttpContext) {
    let Some(state) = ctx.ai_state_mut() else {
        return;
    };
    let Some(summary) = state.summary.as_ref() else {
        return;
    };
    let Some(ai) = state.request.as_mut() else {
        return;
    };
    let Some(quota) = ai.quota.as_mut() else {
        return;
    };
    let authoritative_total = if state.attempt_runtime.ever_dispatched() {
        state.attempt_runtime.charged_request_tokens().ok()
    } else {
        summary
            .input_tokens
            .and_then(|input| summary.output_tokens.and_then(|output| input.checked_add(output)))
    };
    let overflow = state.attempt_runtime.accounting_overflow()
        || ai
            .usage
            .accounting_snapshot(ceil_chars_div_four(ai.usage.input_chars()), true)
            .unavailable
            == Some(super::usage::AiUsageUnavailableReason::ArithmeticOverflow);
    let status = match authoritative_total {
        Some(total) => quota.finalize(total).await,
        None => quota.finalize_unknown().await,
    };
    if let Some(summary) = state.summary.as_mut() {
        summary.quota_status = Some(
            if overflow && status == crate::ai::quota::QuotaFinalStatus::Unresolved {
                "accounting_overflow"
            } else {
                status.as_str()
            },
        );
    }
    if overflow
        || matches!(
            status,
            crate::ai::quota::QuotaFinalStatus::Failed | crate::ai::quota::QuotaFinalStatus::TimedOut
        )
    {
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
