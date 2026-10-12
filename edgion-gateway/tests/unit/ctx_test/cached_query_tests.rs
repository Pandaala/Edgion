use super::*;
use edgion_resources::resources::http_route::HTTPPathMatch;
use pingora_http::RequestHeader;

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
    let mut empty_query_ctx = EdgionHttpContext::new();
    empty_query_ctx.request_info.entry_raw_query = Some(String::new());
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx.cached_query().is_empty());
        assert!(empty_query_ctx.cached_query().is_empty());
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
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.entry_raw_query = Some("name=hello%20world&plus=hello+world&n=1".to_string());
    assert!(ctx.parsed_request_caches.query.get().is_none());

    let first = ctx.cached_query() as *const _;
    assert!(ctx.parsed_request_caches.query.get().is_some());
    // Synthetic contexts can still mutate the snapshot; an initialized cache
    // retains its values and borrowed-map identity.
    ctx.request_info.entry_raw_query = Some("name=changed&n=2".to_string());
    let second = ctx.cached_query() as *const _;
    assert_eq!(first, second, "cached_query must be init-once per request");

    let map = ctx.cached_query();
    assert_eq!(map.get("name").map(String::as_str), Some("hello world"));
    assert_eq!(map.get("plus").map(String::as_str), Some("hello world"));
    assert_eq!(map.get("n").map(String::as_str), Some("1"));
}

#[test]
fn cached_query_handles_none_raw() {
    let mut ctx = EdgionHttpContext::new();
    let first = ctx.cached_query() as *const _;
    assert!(ctx.cached_query().is_empty(), "Absent entry query yields empty cache");
    ctx.request_info.entry_raw_query = Some("a=1".to_string());
    let map = ctx.cached_query();
    assert!(map.is_empty(), "OnceLock already initialized to empty");
    assert_eq!(first, map as *const _);
}

#[test]
fn cached_query_uses_entry_snapshot_across_live_rewrites() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.entry_raw_query = Some("name=client%20value".to_string());
    let mut req = RequestHeader::build("GET", b"/entry?name=client%20value", None).unwrap();

    // A rewrite before the first decoded read must not choose the cache source.
    req.set_uri("/backend?name=rewritten+value".parse().unwrap());
    assert_eq!(ctx.query(&req), Some("name=rewritten+value"));
    assert_eq!(ctx.cached_query().get("name").map(String::as_str), Some("client value"));
    let first = ctx.cached_query() as *const _;

    req.set_uri("/other?name=later%20value".parse().unwrap());
    assert_eq!(ctx.query(&req), Some("name=later%20value"));
    assert_eq!(ctx.cached_query().get("name").map(String::as_str), Some("client value"));
    assert_eq!(first, ctx.cached_query() as *const _);
}
