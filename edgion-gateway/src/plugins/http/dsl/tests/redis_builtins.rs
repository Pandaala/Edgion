//! Unit-test coverage for `resolve_redis_client` failure paths.
//!
//! These two paths are DSL-specific (alias → ns/name → live-client lookup) and
//! cannot be exercised by `edgion-tests/integration/`, which always runs against
//! a real Redis with a properly registered link.
//!
//! All `redis.*` builtins share the same `resolve_redis_client` prelude and
//! return its `Err` value verbatim via `return Ok(e)`; covering the prelude
//! through `redis.get` proves the contract for all thirteen builtins.

use super::*;
use std::collections::HashMap;

/// Path A: alias is not present in `runtime.redis_links`. The `redis.get`
/// builtin must return `Map { ok: false, value: Nil, error: Str("link 'X' not found ...") }`,
/// which the DSL script surfaces as a response header so the mock can capture it.
#[test]
fn test_redis_get_returns_redis_err_when_link_alias_not_declared() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "x-test-ok" && value == "false")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|name, value| name == "x-test-err" && value.contains("link 'p0_missing' not found"))
        .times(1)
        .returning(|_, _| Ok(()));

    let runtime = DslRuntime::default();

    let source = r#"let r = redis.get("p0_missing", "anykey")
resp.set_header("x-test-ok", r["ok"])
resp.set_header("x-test-err", r["error"])"#;

    let result = run_with_runtime(source, session, runtime);
    assert_eq!(result, good_next());
}

/// Path B: alias maps to a namespace/name that is intentionally absent from the
/// global `REDIS_RUNTIME` ArcSwap (see `link_sys/runtime/store.rs:227`). The
/// `__edgion_dsl_p0_test__` namespace prefix is reserved for this file —
/// no other code path constructs `RedisLinkClient` instances under that
/// namespace, so `get_redis_client(...)` returns `None` deterministically.
#[test]
fn test_redis_get_returns_redis_err_when_link_not_ready() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "x-test-ok" && value == "false")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|name, value| name == "x-test-err" && value.contains("link 'p0_notready' not ready"))
        .times(1)
        .returning(|_, _| Ok(()));

    let mut redis_links: HashMap<String, String> = HashMap::new();
    redis_links.insert("p0_notready".to_string(), "__edgion_dsl_p0_test__/notready".to_string());
    let runtime = DslRuntime {
        redis_links,
        ..DslRuntime::default()
    };

    let source = r#"let r = redis.get("p0_notready", "anykey")
resp.set_header("x-test-ok", r["ok"])
resp.set_header("x-test-err", r["error"])"#;

    let result = run_with_runtime(source, session, runtime);
    assert_eq!(result, good_next());
}
