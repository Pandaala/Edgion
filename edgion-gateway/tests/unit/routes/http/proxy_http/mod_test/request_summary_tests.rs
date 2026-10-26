use super::append_request_trace_summary;
use crate::test_allocation_counter::count_allocation_stats;
use std::hint::black_box;

fn original_summary(summary: String, trace_id: Option<&str>) -> String {
    match trace_id {
        Some(trace_id) => format!("{summary}, trace-id: {trace_id}"),
        None => summary,
    }
}

#[test]
fn trace_summary_preserves_bytes_and_reuses_available_capacity() {
    let long = format!("GET /{}, Host: example.com", "a".repeat(4096));
    for summary in ["", "GET /, Host: example.com", "GET /café, Host: example.com", &long] {
        for trace in [None, Some(""), Some("0123456789abcdef0123456789abcdef")] {
            for spare in [0, 128] {
                let input = || {
                    let mut value = String::with_capacity(summary.len() + spare);
                    value.push_str(summary);
                    value
                };
                let old_input = input();
                let new_input = input();
                let original_ptr = new_input.as_ptr();
                let original_capacity = new_input.capacity();
                let (expected, before) =
                    count_allocation_stats(|| original_summary(black_box(old_input), black_box(trace)));
                let (actual, after) =
                    count_allocation_stats(|| append_request_trace_summary(black_box(new_input), black_box(trace)));
                assert_eq!(actual, expected);
                // These comparisons cover this fixture, not all allocator implementations.
                assert!(after.calls <= before.calls);
                assert!(after.requested_bytes <= before.requested_bytes);
                if trace.is_none() || original_capacity >= expected.len() {
                    assert_eq!(actual.as_ptr(), original_ptr);
                    assert_eq!(actual.capacity(), original_capacity);
                    assert_eq!(after.calls, 0);
                    assert_eq!(after.requested_bytes, 0);
                }
            }
        }
    }
}

#[test]
fn diagnostic_summary_is_safe_before_headers_and_after_deferred_secret_removal() {
    let mut ctx = crate::EdgionHttpContext::new();
    assert_eq!(super::request_diagnostic_summary(&ctx), "request -");
    ctx.request_info.method = Some(http::Method::GET);
    ctx.request_info.entry_raw_path = "/private/path".into();
    ctx.request_query.capture(Some("token=secret&keep=1".into())).unwrap();
    ctx.request_query.remove_current_names(&["token"]).unwrap();
    assert_eq!(super::request_diagnostic_summary(&ctx), "request GET");
    assert!(ctx.request_query.projection_is_dirty());
}
