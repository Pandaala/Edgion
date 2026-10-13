use super::super::super::parser::parse_program;
use super::super::Compiler;

#[test]
fn test_compile_for_range() {
    let source = r#"
            for i in range(0, 5) {
                log(to_str(i))
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert!(compiled.max_loop_depth >= 1);
}

#[test]
fn test_compile_break_in_while() {
    let source = r#"
            let mut i = 0
            while i < 10 {
                if i == 5 {
                    break
                }
                i = i + 1
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert!(compiled.max_loop_depth >= 1);
}

#[test]
fn test_compile_continue_in_while() {
    let source = r#"
            let mut i = 0
            while i < 10 {
                i = i + 1
                if i == 5 {
                    continue
                }
                log(to_str(i))
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert!(compiled.max_loop_depth >= 1);
}

#[test]
fn test_compile_break_in_for_range() {
    let source = r#"
            for i in range(0, 10) {
                if i == 5 {
                    break
                }
                log(to_str(i))
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert!(compiled.max_loop_depth >= 1);
}

#[test]
fn test_compile_break_outside_loop_error() {
    let source = r#"break"#;
    let prog = parse_program(source).unwrap();
    let result = Compiler::new().compile(&prog);
    assert!(result.is_err());
}

#[test]
fn test_compile_continue_outside_loop_error() {
    let source = r#"continue"#;
    let prog = parse_program(source).unwrap();
    let result = Compiler::new().compile(&prog);
    assert!(result.is_err());
}

/// Nested loops mixing all three loop forms (for-range, for-in, while) must
/// keep the break/continue patch stacks balanced (push at entry, pop at exit).
/// A future refactor that imbalances the stacks would surface here as a
/// `CompileError` instead of a panic.
#[test]
fn test_compile_nested_loops_patch_stacks_balanced() {
    let source = r#"
            let mut total = 0
            for i in range(0, 3) {
                let mut j = 0
                while j < 3 {
                    for name in req.header_names {
                        if name == "skip" {
                            continue
                        }
                        total = total + 1
                    }
                    if j == 2 {
                        break
                    }
                    j = j + 1
                }
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert!(compiled.max_loop_depth >= 3);
}

#[test]
fn internal_loop_storage_is_not_source_visible() {
    for (loop_head, names) in [
        ("for i in range(0, 1)", vec!["__range_end__"]),
        ("for i in split(\"1\", \",\")", vec!["__list__", "__idx__"]),
    ] {
        for name in names {
            for body in [format!("let value = {name}"), format!("{name} = 100")] {
                let source = format!("{loop_head} {{ {body} }}");
                let program = parse_program(&source).unwrap();
                let error = Compiler::new().compile(&program).unwrap_err();
                assert!(error.message.contains("undefined variable"), "{source}: {error}");
            }
        }
    }
}

#[test]
fn authored_internal_looking_names_remain_legal_immutable_loop_variables() {
    for name in ["__range_end__", "__list__", "__idx__"] {
        for iterable in ["range(0, 1)", "split(\"1\", \",\")"] {
            let source = format!("for {name} in {iterable} {{ let copy = {name} }}");
            assert!(Compiler::new().compile(&parse_program(&source).unwrap()).is_ok());
            let source = format!("for {name} in {iterable} {{ {name} = 2 }}");
            assert!(Compiler::new().compile(&parse_program(&source).unwrap()).is_err());
        }
    }
}

#[test]
fn anonymous_loop_slots_remain_in_physical_local_count() {
    for (source, expected) in [
        ("for i in range(0, 1) {}", 2),
        ("for i in split(\"1\", \",\") {}", 3),
        ("for i in split(\"1\", \",\") { for j in range(0, 1) {} }", 5),
        ("for i in range(0, 1) {} for j in split(\"1\", \",\") {}", 5),
    ] {
        let compiled = Compiler::new().compile(&parse_program(source).unwrap()).unwrap();
        assert_eq!(compiled.local_count, expected);
    }
}
