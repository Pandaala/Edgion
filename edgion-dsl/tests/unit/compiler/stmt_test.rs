use super::super::super::bytecode::Constant;
use super::super::super::parser::parse_program;
use super::super::Compiler;

#[test]
fn test_compile_simple_let() {
    let prog = parse_program("let x = 42").unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert_eq!(compiled.local_count, 1);
    assert!(!compiled.code.is_empty());
}

#[test]
fn test_compile_deny_script() {
    let source = r#"
            let ip = req.header("X-Real-IP")
            if ip == nil {
                return deny(403, "blocked")
            }
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert_eq!(compiled.local_count, 1);
    assert!(compiled.code.len() <= 20);
}

#[test]
fn test_compile_immutable_assign_error() {
    let source = r#"
            let x = 1
            x = 2
        "#;
    let prog = parse_program(source).unwrap();
    let result = Compiler::new().compile(&prog);
    assert!(result.is_err());
}

#[test]
fn test_compile_mutable_assign() {
    let source = r#"
            let mut x = 1
            x = 2
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    assert_eq!(compiled.local_count, 1);
}

#[test]
fn test_const_dedup() {
    let source = r#"
            let a = "hello"
            let b = "hello"
        "#;
    let prog = parse_program(source).unwrap();
    let compiled = Compiler::new().compile(&prog).unwrap();
    // "hello" should only appear once in the constant pool
    let str_count = compiled
        .constants
        .iter()
        .filter(|c| matches!(c, Constant::Str(s) if s == "hello"))
        .count();
    assert_eq!(str_count, 1);
}
