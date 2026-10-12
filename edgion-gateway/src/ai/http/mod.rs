//! AI HTTP lifecycle integration.

pub(crate) mod attempt;
pub(crate) mod proxy;
pub(crate) mod response;

use crate::EdgionHttpContext;

/// Settle AI accounting once at the existing universal terminal boundary.
pub(crate) async fn finalize(ctx: &mut EdgionHttpContext) {
    if ctx.ai_state().is_none() {
        return;
    }
    if let Some(driver) = ctx.response_processor_driver() {
        driver.finalize_ai_incomplete().await;
    }
    if let Some(state) = ctx.ai_state_mut() {
        if let Some(processor) = state.stream_processor.as_mut() {
            processor.finalize_incomplete("terminal", "builder-fallback");
        }
        state.unary_owner = None;
    }
    crate::ai::accounting::finalize::finalize(ctx).await;
}

/// Finalize semantic processing at the late logging boundary, after stream logs.
pub(crate) async fn finalize_semantic_for_logging(
    ctx: &mut EdgionHttpContext,
    error_source: Option<&pingora_core::ErrorSource>,
) {
    use pingora_core::ErrorSource;

    let (outcome, reason) = match error_source {
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
}

pub(crate) fn finalize_guardrail_log(ctx: &mut EdgionHttpContext) {
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

#[cfg(test)]
#[path = "../../../tests/unit/ai/http/mod_test.rs"]
mod tests;
