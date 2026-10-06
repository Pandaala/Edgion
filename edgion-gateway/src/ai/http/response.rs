//! Provider-specific response cooldown handling.

use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use std::time::{Duration, SystemTime};

const MAX_PROVIDER_RESET_HINT_BYTES: usize = 64;
const OPENAI_RESET_HEADERS: [&str; 2] = ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"];

pub(crate) fn apply_provider_cooldown(ctx: &mut EdgionHttpContext, upstream_response: &mut ResponseHeader) {
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
        let seconds = crate::ai::runtime::attempt_runtime::cooldown_delta_seconds(cooldown);
        let _ = upstream_response.insert_header("retry-after", seconds.to_string());
    }
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
#[path = "../../../tests/unit/ai/http/response_test.rs"]
mod tests;
