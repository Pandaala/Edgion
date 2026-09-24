use super::super::PathRadixBuilder;
use super::PathMatchKind;

fn ext_has(results: &[(u32, PathMatchKind)], val: u32, kind: PathMatchKind) -> bool {
    results.iter().any(|&(v, k)| v == val && k == kind)
}

#[test]
fn test_ext_fully_consumed() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let results = tree.match_all_ext("/api");
    assert!(ext_has(&results, 1, PathMatchKind::FullyConsumed));
}

#[test]
fn test_ext_segment_boundary() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let results = tree.match_all_ext("/api/users");
    assert!(ext_has(&results, 1, PathMatchKind::SegmentBoundary));
}

#[test]
fn test_ext_partial_segment() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/v2", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let results = tree.match_all_ext("/v2example");
    assert!(ext_has(&results, 1, PathMatchKind::PartialSegment));
}

#[test]
fn test_ext_root_prefix_matches_everything() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let r = tree.match_all_ext("/");
    assert!(ext_has(&r, 1, PathMatchKind::FullyConsumed));

    let r = tree.match_all_ext("/anything");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));

    let r = tree.match_all_ext("/a/b/c");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));
}

#[test]
fn test_ext_exact_and_prefix_same_path() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api", 1).unwrap();
    builder.insert("/api/users", 2).unwrap();
    let tree = builder.freeze().unwrap();

    let r = tree.match_all_ext("/api");
    assert!(ext_has(&r, 1, PathMatchKind::FullyConsumed));
    assert!(!r.iter().any(|&(v, _)| v == 2));

    let r = tree.match_all_ext("/api/users");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));
    assert!(ext_has(&r, 2, PathMatchKind::FullyConsumed));

    let r = tree.match_all_ext("/api/users/123");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));
    assert!(ext_has(&r, 2, PathMatchKind::SegmentBoundary));
}

#[test]
fn test_ext_param_routes() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/users/:id", 1).unwrap();
    builder.insert("/users/:id/profile", 2).unwrap();
    let tree = builder.freeze().unwrap();

    let r = tree.match_all_ext("/users/123");
    assert!(ext_has(&r, 1, PathMatchKind::FullyConsumed));

    let r = tree.match_all_ext("/users/123/profile");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));
    assert!(ext_has(&r, 2, PathMatchKind::FullyConsumed));

    let r = tree.match_all_ext("/users/123/settings");
    assert!(ext_has(&r, 1, PathMatchKind::SegmentBoundary));
    assert!(!r.iter().any(|&(v, _)| v == 2));
}

#[test]
fn test_ext_no_match() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let r = tree.match_all_ext("/home");
    assert!(r.is_empty());
}

#[test]
fn test_ext_multi_segment_boundary() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api/v1", 1).unwrap();
    let tree = builder.freeze().unwrap();

    assert!(ext_has(&tree.match_all_ext("/api/v1"), 1, PathMatchKind::FullyConsumed));
    assert!(ext_has(
        &tree.match_all_ext("/api/v1/users"),
        1,
        PathMatchKind::SegmentBoundary
    ));
    assert!(ext_has(
        &tree.match_all_ext("/api/v1beta"),
        1,
        PathMatchKind::PartialSegment
    ));
}

#[test]
fn test_match_all_static_routes() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/health", 1).unwrap();
    builder.insert("/echo", 2).unwrap();
    builder.insert("/api/v1/users", 3).unwrap();
    let tree = builder.freeze().unwrap();

    // Test static route matching
    let results = tree.match_all("/health");
    assert!(results.contains(&1), "Should match /health");

    let results = tree.match_all("/echo");
    assert!(results.contains(&2), "Should match /echo");

    let results = tree.match_all("/api/v1/users");
    assert!(results.contains(&3), "Should match /api/v1/users");

    // Test non-matching path
    let results = tree.match_all("/notfound");
    assert!(results.is_empty(), "Should not match /notfound");
}

#[test]
fn test_match_all_with_params() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api/:version/users", 1).unwrap();
    builder.insert("/api/v1/users", 2).unwrap();
    let tree = builder.freeze().unwrap();

    // Should match both parametric and static routes
    let results = tree.match_all("/api/v1/users");
    assert!(results.contains(&1), "Should match parametric /api/:version/users");
    assert!(results.contains(&2), "Should match static /api/v1/users");
}

#[test]
fn test_match_all_prefixes_still_works() {
    let mut builder = PathRadixBuilder::new();
    builder.insert("/api", 1).unwrap();
    builder.insert("/api/users", 2).unwrap();
    let tree = builder.freeze().unwrap();

    // match_all should find all matching prefixes
    let results = tree.match_all("/api/users/123");
    assert!(results.contains(&1), "Should match prefix /api");
    assert!(results.contains(&2), "Should match prefix /api/users");
}
