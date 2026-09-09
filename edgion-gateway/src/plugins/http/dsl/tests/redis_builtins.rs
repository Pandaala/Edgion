//! Unit-test coverage for `resolve_redis_client` failure paths.
//!
//! These two paths are DSL-specific (alias → ns/name → live-client lookup) and
//! cannot be exercised by `edgion-tests/integration/`, which always runs against
//! a real Redis with a properly registered link.
//!
//! The thirteen basic Redis operations share the `resolve_redis_client` prelude
//! and return its `Err` value verbatim via `return Ok(e)`. The `redis.get` tests
//! cover that common lookup contract. The separate `redis.rate_limit` tests cover
//! its fixed error classifications and pre-I/O validation.

use super::super::runtime::DslRateLimits;
use super::*;
use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::DslConfig;
use std::collections::HashMap;

fn rate_limit_runtime() -> DslRuntime {
    let config: DslConfig = serde_json::from_value(serde_json::json!({
        "name": "limited",
        "linkSysRefs": [{"name": "cache", "namespace": "__dsl_rate_limit_unit_test__"}],
        "rateLimitPolicies": [{"name": "requests", "link": "cache", "namespace": "unit-test",
            "rate": 10, "interval": "1s"}]
    }))
    .unwrap();
    DslRuntime {
        redis_links: HashMap::from([("cache".into(), "__dsl_rate_limit_unit_test__/cache".into())]),
        rate_limits: DslRateLimits::from_config(&config).unwrap(),
        ..Default::default()
    }
}

#[test]
fn redis_rate_limit_errors_are_script_visible_and_sanitized() {
    for (args, expected) in [
        ("\"cache\", \"requests\", \"key\", 1", "provider_not_ready"),
        ("\"cache\", \"missing\", \"key\", 1", "unknown_policy"),
        ("\"other\", \"requests\", \"key\", 1", "unknown_policy"),
        ("\"cache\", \"requests\", \"key\", 0", "invalid_argument"),
        ("\"cache\", \"requests\", \"key\", -1", "invalid_argument"),
        ("\"cache\", \"requests\", \"key\", 1000001", "invalid_argument"),
        ("\"cache\", \"requests\", \"key\", \"1\"", "invalid_argument"),
        ("\"cache\", \"requests\", 1, 1", "invalid_argument"),
        ("nil, \"requests\", \"key\", 1", "invalid_argument"),
    ] {
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header()
            .withf(move |name, value| name == "x-error" && value == expected)
            .times(1)
            .returning(|_, _| Ok(()));
        let source = format!("let r = redis.rate_limit({args})\nif r[\"ok\"] || r[\"value\"] != nil {{ return deny(500, \"bad envelope\") }}\nresp.set_header(\"x-error\", r[\"error\"])");
        assert_eq!(run_with_runtime(&source, session, rate_limit_runtime()), good_next());
    }
    let key = "x".repeat(4096);
    let source = format!("let r = redis.rate_limit(\"cache\", \"requests\", \"{key}\" + \"x\", 1)\nif r[\"error\"] != \"invalid_argument\" {{ return deny(500, \"bad key limit\") }}");
    assert_eq!(
        run_with_runtime(&source, MockPluginSession::new(), rate_limit_runtime()),
        good_next()
    );
}

#[test]
fn redis_rate_limit_result_budget_is_checked_before_io() {
    for limits in [
        VmLimits {
            max_map_size: 4,
            ..Default::default()
        },
        VmLimits {
            max_string_len: 17,
            ..Default::default()
        },
    ] {
        let vm = make_vm_with_limits("redis.rate_limit(\"cache\", \"requests\", \"key\", 1)", limits);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let runtime = rate_limit_runtime();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(vm.execute(&mut session, &mut log, &runtime));
        assert!(matches!(
            result,
            Err(RuntimeError::MapTooLarge { .. } | RuntimeError::StringTooLong { .. })
        ));
    }
}

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
