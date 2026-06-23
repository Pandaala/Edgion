//! Tests for compile-time error detection: undefined variables, immutable assignment, invalid regex, etc.

use super::*;

fn compile_err(source: &str) {
    let result = compile_dsl_source(source, &ValidationLimits::default());
    assert!(result.is_err(), "Expected compile error for: {}", source);
}

#[test]
fn test_break_outside_loop() {
    compile_err("break");
}

#[test]
fn test_continue_outside_loop() {
    compile_err("continue");
}

#[test]
fn test_undefined_variable() {
    compile_err("let x = y");
}

#[test]
fn test_assign_immutable() {
    compile_err("let x = 1\nx = 2");
}

#[test]
fn test_invalid_regex() {
    // The `matches` operator validates regex at compile time
    compile_err(r#"if "hello" matches "[unclosed" { }"#);
}

#[test]
fn test_too_many_instructions() {
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("let x{} = {}\n", i, i));
    }
    compile_err(&src);
}

#[test]
fn test_shadowing_builtin_namespace() {
    compile_err("let req = 1");
}
