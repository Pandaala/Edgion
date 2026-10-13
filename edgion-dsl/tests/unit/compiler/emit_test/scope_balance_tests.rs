use super::super::super::bytecode::OpCode;
use super::super::super::parser::parse_program;
use super::super::Compiler;

#[test]
fn leave_scope_at_depth_zero_returns_err_not_panic() {
    // scope_depth starts at 0; an unbalanced leave must surface a CompileError
    // (graceful degrade) instead of panicking.
    let mut c = Compiler::new();
    assert!(c.leave_scope().is_err());
}

#[test]
fn balanced_enter_leave_scope_is_ok() {
    let mut c = Compiler::new();
    c.enter_scope();
    assert!(c.leave_scope().is_ok());
}

#[test]
fn if_block_preserves_outer_binding_and_discards_inner_binding() {
    let valid = "let outer = 1\nif true { let inner = 2 }\nlet result = outer";
    let program = parse_program(valid).expect("valid scoped DSL should parse");
    let compiled = Compiler::new()
        .compile(&program)
        .expect("outer binding should remain visible after the if block");
    assert!(compiled.code.iter().any(|op| matches!(op, OpCode::GetLocal(0))));

    let invalid = "let outer = 1\nif true { let inner = 2 }\nlet result = inner";
    let program = parse_program(invalid).expect("invalid scoped DSL should still parse");
    let error = Compiler::new()
        .compile(&program)
        .expect_err("inner binding must not escape the if block");
    assert!(error.message.contains("undefined variable 'inner'"), "{error}");
}

#[test]
fn anonymous_slots_share_limits_without_creating_bindings() {
    let mut compiler = Compiler::new();
    assert_eq!(compiler.allocate_local_slot().unwrap(), 0);
    assert!(compiler.locals.is_empty());
    assert_eq!(compiler.declare_local("authored", true).unwrap(), 1);
    assert_eq!(compiler.resolve_local("authored").unwrap().slot, 1);
    compiler.next_slot = u16::MAX - 1;
    assert_eq!(compiler.allocate_local_slot().unwrap(), u16::MAX - 1);
    assert_eq!(compiler.next_slot, u16::MAX);
    for error in [
        compiler.allocate_local_slot().unwrap_err(),
        compiler.declare_local("overflow", false).unwrap_err(),
    ] {
        assert_eq!(error.message, "too many local variables (max 65534)");
    }
    assert_eq!(compiler.next_slot, u16::MAX);
    assert_eq!(compiler.locals.len(), 1);
    assert!(compiler.resolve_local("overflow").is_none());
}
