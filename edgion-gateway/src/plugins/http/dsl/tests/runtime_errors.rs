//! Variant-level assertions for RuntimeError — 7 previously uncovered variants.
//!
//! Uses run_expect_runtime_err / run_expect_runtime_err_with_limits to bypass
//! DslErrorPolicy so callers can assert the exact RuntimeError variant via matches!.

use super::*;

// ── DivisionByZero ──────────────────────────────────────────────────────────

#[test]
fn test_division_by_zero() {
    let err = run_expect_runtime_err("let x = 10 / 0", MockPluginSession::new());
    assert!(matches!(err, RuntimeError::DivisionByZero));
}

// ── IntegerOverflow ──────────────────────────────────────────────────────────

#[test]
fn test_integer_overflow_add() {
    let err = run_expect_runtime_err("let x = 9223372036854775807 + 1", MockPluginSession::new());
    assert!(matches!(err, RuntimeError::IntegerOverflow { operation } if operation == "+"));
}

#[test]
fn test_integer_overflow_sub() {
    // -9223372036854775807 - 2  =  -9223372036854775809  < i64::MIN  → overflow
    let err = run_expect_runtime_err("let x = -9223372036854775807 - 2", MockPluginSession::new());
    assert!(matches!(err, RuntimeError::IntegerOverflow { operation } if operation == "-"));
}

#[test]
fn test_integer_overflow_mul() {
    let err = run_expect_runtime_err("let x = 9223372036854775807 * 2", MockPluginSession::new());
    assert!(matches!(err, RuntimeError::IntegerOverflow { operation } if operation == "*"));
}

#[test]
fn test_integer_overflow_div() {
    // i64::MIN / -1  overflows — build i64::MIN via subtraction to stay within
    // the parser's positive-literal range.
    let err = run_expect_runtime_err(
        "let a = 0 - 9223372036854775807 - 1\nlet b = 0 - 1\nlet x = a / b",
        MockPluginSession::new(),
    );
    assert!(matches!(err, RuntimeError::IntegerOverflow { operation } if operation == "/"));
}

#[test]
fn test_integer_overflow_neg() {
    // -(i64::MIN)  overflows — build i64::MIN the same way as div test.
    let err = run_expect_runtime_err(
        "let a = 0 - 9223372036854775807 - 1\nlet x = -a",
        MockPluginSession::new(),
    );
    assert!(matches!(
        err,
        RuntimeError::IntegerOverflow { operation } if operation == "unary -"
    ));
}

// ── CallLimitExceeded ────────────────────────────────────────────────────────

#[test]
fn test_call_limit_exceeded() {
    // tick_call() fires before the session call, so no mock expectations needed.
    let limits = VmLimits {
        max_call_count: 0,
        ..Default::default()
    };
    let err = run_expect_runtime_err_with_limits(r#"req.set_header("x", "y")"#, MockPluginSession::new(), limits);
    assert!(matches!(err, RuntimeError::CallLimitExceeded { limit: 0 }));
}

// ── StackOverflow ────────────────────────────────────────────────────────────

#[test]
fn test_stack_overflow() {
    // max_stack_depth: 1 — the second LoadConst in "1 + 2" overflows.
    let limits = VmLimits {
        max_stack_depth: 1,
        ..Default::default()
    };
    let err = run_expect_runtime_err_with_limits("let x = 1 + 2", MockPluginSession::new(), limits);
    assert!(matches!(err, RuntimeError::StackOverflow { limit: 1 }));
}

// ── ApiError ─────────────────────────────────────────────────────────────────

#[test]
fn test_api_error() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "x" && v == "y")
        .times(1)
        .returning(|_, _| Err(Box::new(std::io::Error::other("injected session failure"))));
    let err = run_expect_runtime_err(r#"req.set_header("x", "y")"#, session);
    assert!(matches!(
        err,
        RuntimeError::ApiError { ref function, .. } if function == "req.set_header"
    ));
}

// ── Internal ─────────────────────────────────────────────────────────────────

#[test]
fn test_internal_stack_underflow() {
    // Pop on an empty stack triggers RuntimeError::Internal { "stack underflow" }.
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::Pop],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let err = run_expect_runtime_err_on_script(script, MockPluginSession::new(), VmLimits::default());
    assert!(matches!(
        err,
        RuntimeError::Internal { ref message } if message.contains("stack underflow")
    ));
}
