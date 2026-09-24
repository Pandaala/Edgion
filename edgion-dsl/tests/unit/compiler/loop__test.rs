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
