use super::*;

#[test]
fn test_parse_path_static() {
    let segments = PathRadixBuilder::parse_path(b"/api/users");
    assert_eq!(segments, vec![PathSegment::Static(b"/api/users".to_vec())]);
}

#[test]
fn test_parse_path_single_param() {
    let segments = PathRadixBuilder::parse_path(b"/api/:version/users");
    assert_eq!(
        segments,
        vec![
            PathSegment::Static(b"/api/".to_vec()),
            PathSegment::Param,
            PathSegment::Static(b"/users".to_vec()),
        ]
    );
}

#[test]
fn test_parse_path_multi_params() {
    let segments = PathRadixBuilder::parse_path(b"/api/:version/:resource");
    assert_eq!(
        segments,
        vec![
            PathSegment::Static(b"/api/".to_vec()),
            PathSegment::Param,
            PathSegment::Static(b"/".to_vec()),
            PathSegment::Param,
        ]
    );
}

#[test]
fn test_parse_path_escaped_colon() {
    let segments = PathRadixBuilder::parse_path(b"/api/::literal/test");
    assert_eq!(
        segments,
        vec![
            PathSegment::Static(b"/api/:literal".to_vec()),
            PathSegment::Static(b"/test".to_vec()),
        ]
    );
}

#[test]
fn test_parse_path_param_at_end() {
    let segments = PathRadixBuilder::parse_path(b"/users/:id");
    assert_eq!(
        segments,
        vec![PathSegment::Static(b"/users/".to_vec()), PathSegment::Param,]
    );
}

#[test]
fn test_parse_path_root_only() {
    let segments = PathRadixBuilder::parse_path(b"/");
    assert_eq!(segments, vec![PathSegment::Static(b"/".to_vec())]);
}

#[test]
fn test_parse_path_param_empty_name() {
    // /api/:/test - param has empty name (just ":")
    let segments = PathRadixBuilder::parse_path(b"/api/:/test");
    assert_eq!(
        segments,
        vec![
            PathSegment::Static(b"/api/".to_vec()),
            PathSegment::Param,
            PathSegment::Static(b"/test".to_vec()),
        ]
    );
}

#[test]
fn test_parse_path_trailing_param() {
    // /api/: - param at end with empty name
    let segments = PathRadixBuilder::parse_path(b"/api/:");
    assert_eq!(
        segments,
        vec![PathSegment::Static(b"/api/".to_vec()), PathSegment::Param,]
    );
}

#[test]
fn test_parse_path_consecutive_escapes() {
    // /api/:::test -> /api/::test (first :: escapes, third : is literal)
    let segments = PathRadixBuilder::parse_path(b"/api/:::test");
    assert_eq!(segments, vec![PathSegment::Static(b"/api/::test".to_vec())]);
}

#[test]
fn test_parse_path_escape_then_param() {
    // /api/::x/:id -> Static("/api/:x"), Static("/"), Param
    let segments = PathRadixBuilder::parse_path(b"/api/::x/:id");
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0], PathSegment::Static(b"/api/:x".to_vec()));
    assert_eq!(segments[2], PathSegment::Param);
}

#[test]
fn test_parse_path_double_slash() {
    // Double slash - treated as static
    let segments = PathRadixBuilder::parse_path(b"/api//test");
    assert_eq!(segments, vec![PathSegment::Static(b"/api//test".to_vec())]);
}
