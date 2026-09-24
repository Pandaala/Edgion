use super::*;
use edgion_resources::resources::http_route::HTTPPathMatch;

fn matched_path(match_type: &str, value: &str) -> HTTPRouteMatch {
    HTTPRouteMatch {
        path: Some(HTTPPathMatch {
            match_type: Some(match_type.to_string()),
            value: Some(value.to_string()),
        }),
        headers: None,
        query_params: None,
        method: None,
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn parsed_request_caches_have_compact_inline_layout() {
    assert_eq!(std::mem::size_of::<ParsedRequestCaches>(), 32);
}

#[test]
fn empty_parsed_views_initialize_without_allocating() {
    let ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx.cached_query(None).is_empty());
        assert!(ctx.cache_path_params(HashMap::new()).is_empty());
    });

    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn matched_path_captures_are_frozen_before_rewrite() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/users/123".to_string();
    ctx.capture_matched_path_params(&matched_path("Exact", "/users/:id"));
    ctx.request_info.normalized_current_path = "/backend/999".to_string();
    ctx.capture_matched_path_params(&matched_path("Exact", "/backend/:id"));

    assert_eq!(
        ctx.cached_path_params().unwrap().get("id").map(String::as_str),
        Some("123")
    );
}

#[test]
fn regular_expression_paths_do_not_create_named_captures() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/users/123".to_string();
    ctx.capture_matched_path_params(&matched_path("RegularExpression", "/users/:id"));

    assert!(ctx.cached_path_params().unwrap().is_empty());
}

#[test]
fn cached_query_initializes_lazily_and_caches() {
    let ctx = EdgionHttpContext::new();
    // First access triggers parse. The space in `hello world` is percent-
    // encoded as `%20`, so the decoded value exercises url-decode.
    let first = ctx.cached_query(Some("name=hello%20world&n=1")) as *const _;
    // Second access with a *different* raw_query must still return the
    // already-cached HashMap — init-once semantics, not "re-parse".
    let second = ctx.cached_query(Some("name=changed&n=2")) as *const _;
    assert_eq!(first, second, "cached_query must be init-once per request");

    // Verify decoded values (route-matcher-compatible).
    let map = ctx.cached_query(Some("ignored=value"));
    assert_eq!(map.get("name"), Some(&"hello world".to_string()));
    assert_eq!(map.get("n"), Some(&"1".to_string()));
}

#[test]
fn cached_query_handles_none_raw() {
    let ctx = EdgionHttpContext::new();
    let map = ctx.cached_query(None);
    assert!(map.is_empty(), "None raw_query yields empty cache");
    // Subsequent call with Some(...) must NOT replace the cached empty map.
    let map2 = ctx.cached_query(Some("a=1"));
    assert!(map2.is_empty(), "OnceLock already initialized to empty");
}
