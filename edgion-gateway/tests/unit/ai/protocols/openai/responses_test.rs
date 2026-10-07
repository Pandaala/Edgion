use super::*;
use crate::sse::{SseDecoder, SseRecordRef};

#[test]
fn typed_terminals_validate_status_and_select_usage_without_output_repeats() {
    for (kind, status, expected) in [
        ("response.completed", "completed", ResponseOutcome::Completed),
        ("response.incomplete", "incomplete", ResponseOutcome::Incomplete),
        ("response.failed", "failed", ResponseOutcome::Failed),
    ] {
        let data = format!("data: {{\"type\":\"{kind}\",\"sequence_number\":9,\"response\":{{\"status\":\"{status}\",\"output\":[{{\"text\":\"repeated full output\"}}],\"usage\":{{\"input_tokens\":11,\"output_tokens\":7,\"input_tokens_details\":{{\"cached_tokens\":3}},\"output_tokens_details\":{{\"reasoning_tokens\":4}}}}}}}}\n\n");
        for width in 1..=data.len() {
            let mut decoder = SseDecoder::with_default_limit();
            let mut seen = 0;
            for part in data.as_bytes().chunks(width) {
                decoder.feed_records(Bytes::copy_from_slice(part), |record| {
                    if let SseRecordRef::Block { event, .. } = record {
                        let observation = observe_event(&event);
                        assert_eq!(observation.outcome, Some(expected));
                        assert_eq!(observation.sequence, Some(9));
                        assert_eq!(
                            observation.usage,
                            Some(ReportedAiUsage {
                                input_tokens: 11,
                                output_tokens: 7,
                                cache_read_input_tokens: 3,
                                cache_write_input_tokens: 0,
                            })
                        );
                        assert!(!observation.unavailable);
                        seen += 1;
                    }
                });
            }
            assert_eq!(seen, 1);
        }
    }
}

#[test]
fn local_error_uses_typed_sdk_envelope_and_checked_sequence() {
    let event = local_error(AiProxyError::QuotaUnavailable, Some(7)).unwrap();
    assert!(event.starts_with(b"\n\nevent: error\ndata: "));
    let body = &event[b"\n\nevent: error\ndata: ".len()..event.len() - 2];
    let value: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(value["type"], "error");
    assert_eq!(value["sequence_number"], 8);
    assert_eq!(value["code"], "token_quota_unavailable");
    assert!(value["param"].is_null());
    assert!(local_error(AiProxyError::QuotaExceeded, Some(u64::MAX)).is_none());
}
