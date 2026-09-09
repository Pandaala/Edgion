//! Tests for compile-time error detection: undefined variables, immutable assignment, invalid regex, etc.

use super::*;

fn compile_err(source: &str, expected: &str) {
    let errors =
        compile_dsl_source(source, &ValidationLimits::default()).expect_err("source must fail DSL compilation");
    assert!(
        errors.iter().any(|error| error.contains(expected)),
        "expected compile error containing {expected:?}, got {errors:?}"
    );
}

#[test]
fn test_break_outside_loop() {
    compile_err("break", "'break' outside of loop");
}

#[test]
fn test_continue_outside_loop() {
    compile_err("continue", "'continue' outside of loop");
}

#[test]
fn test_undefined_variable() {
    compile_err("let x = y", "undefined variable 'y'");
}

#[test]
fn test_assign_immutable() {
    compile_err("let x = 1\nx = 2", "cannot assign to immutable variable 'x'");
}

#[test]
fn test_invalid_regex() {
    // The `matches` operator validates regex at compile time
    compile_err(
        r#"if "hello".matches("[unclosed") { }"#,
        "regex is not compatible with streaming matches",
    );
}

#[test]
fn test_too_many_instructions() {
    let source = "true\n".repeat(501);
    compile_err(&source, "too many instructions");
}

#[test]
fn test_shadowing_builtin_namespace() {
    compile_err(
        "let req = 1",
        "'req' is a built-in namespace and cannot be used as a variable name",
    );
}
