use super::{provider_reset_hint, MAX_PROVIDER_RESET_HINT_BYTES};
use crate::ai::AiProviderLabel;
use http::{HeaderMap, HeaderValue};
use std::time::{Duration, SystemTime};

#[test]
fn openai_reset_headers_accept_delta_and_choose_the_later_bound() {
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("250ms"));
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("2s"));
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAi, SystemTime::UNIX_EPOCH),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAiCompatible, SystemTime::UNIX_EPOCH),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::Anthropic, SystemTime::UNIX_EPOCH),
        None,
        "provider-specific headers must not bleed across protocols"
    );
}

#[test]
fn openai_reset_headers_accept_absolute_epoch_and_reject_unsafe_values() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("1700000030"));
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAi, now),
        Some(Duration::from_secs(30))
    );

    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("invalid"));
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("1699999999"));
    assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);

    let oversized = "1".repeat(MAX_PROVIDER_RESET_HINT_BYTES + 1);
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_str(&oversized).unwrap());
    assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);
}
