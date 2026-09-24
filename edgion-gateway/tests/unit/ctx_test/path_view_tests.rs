use super::*;
use pingora_http::RequestHeader;

fn make_ctx(entry_path: &str, entry_query: Option<&str>) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.entry_raw_path = entry_path.to_string();
    ctx.request_info.entry_raw_query = entry_query.map(str::to_string);
    let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
    ctx.request_info.normalized_current_path =
        match edgion_resources::path_normalization::normalize_path(entry_path, &path_cfg) {
            Some(s) => s,
            None => entry_path.to_string(),
        };
    ctx
}

#[test]
fn path_returns_normalized_current() {
    let ctx = make_ctx("/%61dmin", None);
    assert_eq!(ctx.path(), "/admin");
}

#[test]
fn request_path_returns_entry_raw() {
    let ctx = make_ctx("/%61dmin", None);
    assert_eq!(ctx.request_path(), "/%61dmin");
}

#[test]
fn request_uri_composes_entry() {
    let ctx = make_ctx("/%61dmin", Some("x=1"));
    assert_eq!(ctx.request_uri(), "/%61dmin?x=1");
}

#[test]
fn refresh_after_rewrite() {
    let mut ctx = make_ctx("/api/old", None);
    let req = RequestHeader::build("GET", b"/api/new", None).unwrap();
    let path_cfg = edgion_resources::path_normalization::PathNormalizationConfig::default();
    ctx.refresh_normalized_path(&req, &path_cfg);
    assert_eq!(ctx.path(), "/api/new");
    assert_eq!(ctx.request_path(), "/api/old");
}
