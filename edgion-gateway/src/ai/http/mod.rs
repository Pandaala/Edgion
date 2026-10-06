//! AI HTTP lifecycle integration.

pub(crate) mod proxy;
pub(crate) mod response;

use crate::EdgionHttpContext;

/// Settle AI accounting once at the existing universal terminal boundary.
pub(crate) async fn finalize(ctx: &mut EdgionHttpContext) {
    crate::ai::accounting::finalize::finalize(ctx).await;
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
