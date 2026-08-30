#!/usr/bin/env python3
"""Plain-Python self-test for check_gateway_no_tracing.py (no pytest).

Run with: cd cicd/checks && python3 test_check_gateway_no_tracing.py
"""

from check_gateway_no_tracing import scan_repository, scan_source, validate_registrations


def violations(source: str, path: str = "plugins/http/example/plugin.rs", cold=None):
    return scan_source(path, source, cold or set()).violations


def test_exact_cold_registration_allows_try_new():
    source = """
    impl Plugin {
        pub fn try_new() -> Self {
            tracing::info!("constructed");
            Self
        }
    }
    """
    path = "plugins/http/example/plugin.rs"
    assert not violations(source, path, {(path, "Plugin", "try_new")})


def test_same_function_name_in_another_file_is_not_allowed():
    source = "fn try_new() { tracing::info!(\"hot\"); }"
    cold = {("plugins/http/registered/plugin.rs", "<free>", "try_new")}
    hits = violations(source, "plugins/http/other/plugin.rs", cold)
    assert len(hits) == 1 and hits[0][2] == "<free>::try_new", hits


def test_same_method_name_in_another_impl_is_not_allowed():
    source = """
    impl Cold { fn new() { tracing::info!("cold"); } }
    impl Hot { fn new() { tracing::info!("hot"); } }
    """
    path = "plugins/http/example/plugin.rs"
    hits = violations(source, path, {(path, "Cold", "new")})
    assert len(hits) == 1 and hits[0][2] == "Hot::new", hits


def test_inline_module_is_part_of_method_owner():
    source = """
    mod cold { impl Plugin { fn new() { tracing::info!("cold"); } } }
    mod hot { impl Plugin { fn new() { tracing::info!("hot"); } } }
    """
    path = "plugins/http/example/plugin.rs"
    hits = violations(source, path, {(path, "cold::Plugin", "new")})
    assert len(hits) == 1 and hits[0][2] == "hot::Plugin::new", hits


def test_constructor_like_hot_helper_names_are_not_implicitly_cold():
    source = """
    fn build_headers() { tracing::debug!("hot"); }
    fn parse_authorization_value() { tracing::warn!("hot"); }
    """
    hits = violations(source)
    assert [hit[2] for hit in hits] == ["<free>::build_headers", "<free>::parse_authorization_value"], hits


def test_multiline_async_generic_function_is_attributed():
    source = """
    async fn run_request<T>(
        value: T,
    ) -> Result<(), T>
    where
        T: Send,
    {
        tracing::info!("hot");
        Ok(())
    }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "<free>::run_request", hits


def test_comments_and_literals_do_not_count_as_tracing():
    source = r'''
    fn request_filter() {
        // tracing::warn!("comment");
        /* event!("block comment"); */
        let _normal = "tracing::info!";
        let _raw = r#"tracing::debug!"#;
    }
    '''
    assert not violations(source)


def test_test_module_does_not_hide_later_production_function():
    source = """
    #[cfg(test)]
    mod tests {
        fn helper() { tracing::info!("test"); }
    }

    fn request_filter() { tracing::info!("production"); }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "<free>::request_filter", hits


def test_ungated_tests_module_is_scanned():
    source = """
    mod tests {
        fn request_filter() { tracing::info!("production module"); }
    }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "tests::<free>::request_filter", hits


def test_cfg_test_item_is_ignored_without_latching_to_eof():
    source = """
    #[cfg(test)]
    fn helper() { tracing::info!("test"); }

    fn request_filter() { tracing::info!("production"); }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "<free>::request_filter", hits


def test_cfg_not_test_item_is_scanned():
    source = """
    #[cfg(not(test))]
    fn request_filter() { tracing::info!("production"); }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "<free>::request_filter", hits


def test_cfg_any_test_or_feature_item_is_scanned():
    source = """
    #[cfg(any(test, feature = "diagnostics"))]
    fn request_filter() { tracing::info!("can compile in production"); }
    """
    hits = violations(source)
    assert len(hits) == 1 and hits[0][2] == "<free>::request_filter", hits


def test_cfg_attr_with_nested_cfg_test_does_not_hide_production_code():
    source = """
    #[cfg_attr(feature = "diagnostics", cfg(test))]
    fn request_filter() { tracing::info!("production without feature"); }

    #[cfg_attr(feature = "diagnostics", cfg(test))]
    mod tests {
        fn response_filter() { tracing::info!("production without feature"); }
    }
    """
    hits = violations(source)
    assert [hit[2] for hit in hits] == [
        "<free>::request_filter",
        "tests::<free>::response_filter",
    ], hits


def test_inline_exception_requires_a_reason():
    allowed = "fn logging() { tracing::warn!(\"failed\"); // tracing-allow: registered send failure\n}"
    missing_reason = "fn logging() { tracing::warn!(\"failed\"); // tracing-allow:\n}"
    assert not violations(allowed)
    assert len(violations(missing_reason)) == 1


def test_inline_exception_marker_inside_string_is_not_allowed():
    source = 'fn logging() { tracing::warn!("tracing-allow: not a comment"); }'
    assert len(violations(source)) == 1


def test_registration_validation_rejects_hot_stale_and_duplicate_entries():
    path = "plugins/http/example/plugin.rs"
    registrations = [
        (path, "Plugin", "request_filter", "incorrect"),
        (path, "Plugin", "missing", "stale"),
        (path, "Plugin", "cold_without_tracing", "stale"),
        (path, "Plugin", "request_filter", "duplicate"),
    ]
    errors = validate_registrations(
        registrations,
        {path},
        {path: {("Plugin", "request_filter"), ("Plugin", "cold_without_tracing")}},
        {path: {("Plugin", "request_filter")}},
    )
    assert any("duplicate" in error for error in errors), errors
    assert any("known hot" in error for error in errors), errors
    assert any("function missing" in error for error in errors), errors
    assert any("no tracing remains" in error for error in errors), errors


def test_registration_validation_rejects_ambiguous_function_identity():
    path = "plugins/http/example/plugin.rs"
    registrations = [(path, "Plugin", "new", "construction")]
    errors = validate_registrations(
        registrations,
        {path},
        {path: [("Plugin", "new"), ("Plugin", "new")]},
        {path: {("Plugin", "new")}},
    )
    assert any("ambiguous" in error for error in errors), errors


def test_current_repository_has_no_findings():
    violations_found, registry_errors = scan_repository()
    assert not violations_found, violations_found
    assert not registry_errors, registry_errors


if __name__ == "__main__":
    import sys

    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_") and callable(value)]
    failed = 0
    for test in tests:
        try:
            test()
            print(f"PASS: {test.__name__}")
        except Exception as error:
            failed += 1
            print(f"FAIL: {test.__name__}: {error!r}")
    print(f"{len(tests) - failed} passed, {failed} failed")
    sys.exit(1 if failed else 0)
