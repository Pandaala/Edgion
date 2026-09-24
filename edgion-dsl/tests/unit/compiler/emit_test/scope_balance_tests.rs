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
