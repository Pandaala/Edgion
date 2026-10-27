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
    assert_eq!(std::mem::size_of::<ParsedRequestCaches>(), 16);
}

#[test]
fn empty_parsed_views_initialize_without_allocating() {
    let ctx = EdgionHttpContext::new();
    let mut empty_query_ctx = EdgionHttpContext::new();
    empty_query_ctx.request_query.capture(Some(String::new())).unwrap();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx.entry_query_param("missing").is_none());
        assert!(empty_query_ctx.entry_query_param("missing").is_none());
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
    ctx.request_query
        .capture(Some("name=hello%20world&plus=hello+world&n=1".into()))
        .unwrap();
    let first = ctx.entry_query_param("name").unwrap().as_ptr();
    assert_eq!(ctx.query_param("name").unwrap().as_ptr(), first);
    assert!(ctx.request_query.capture(Some("name=changed".into())).is_err());
    assert_eq!(ctx.entry_query_param("name"), Some("hello world"));
    assert_eq!(ctx.entry_query_param("name").unwrap().as_ptr(), first);
    assert_eq!(ctx.entry_query_param("plus"), Some("hello world"));
    assert_eq!(ctx.entry_query_param("n"), Some("1"));
}

#[test]
fn cached_query_handles_none_raw() {
    let mut ctx = EdgionHttpContext::new();
    assert_eq!(ctx.entry_query_param("a"), None);
    ctx.request_query.capture(Some("a=1".into())).unwrap();
    assert_eq!(ctx.entry_query_param("a"), Some("1"));
}

#[test]
fn cached_query_uses_entry_snapshot_across_live_rewrites() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_query.capture(Some("name=client%20value".into())).unwrap();
    ctx.request_query
        .replace_raw(Some("name=rewritten+value".into()))
        .unwrap();
    assert_eq!(ctx.query(), Some("name=rewritten+value"));
    assert_eq!(ctx.query_param("name"), Some("rewritten value"));
    assert_eq!(ctx.entry_query_param("name"), Some("client value"));
    let first = ctx.entry_query_param("name").unwrap().as_ptr();
    ctx.request_query
        .replace_raw(Some("name=later%20value".into()))
        .unwrap();
    assert_eq!(ctx.query(), Some("name=later%20value"));
    assert_eq!(ctx.query_param("name"), Some("later value"));
    assert_eq!(ctx.entry_query_param("name").unwrap().as_ptr(), first);
}
