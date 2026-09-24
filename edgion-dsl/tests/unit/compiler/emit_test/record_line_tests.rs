use super::super::Compiler;
use super::*;

#[test]
fn test_record_line_saturates_at_u16_max() {
    // A real-world parser cannot drive line_runs past u16::MAX (max_ast_nodes = 2000),
    // so exercise the boundary by emitting through the public emit() path with current_line
    // pinned. After emitting u16::MAX + 1 opcodes on a single line we expect exactly two runs:
    // (u16::MAX, 1) followed by (1, 1).
    let mut c = Compiler::new();
    c.current_line = 1;
    for _ in 0..(u16::MAX as u32 + 1) {
        c.emit(OpCode::LoadNil);
    }
    assert_eq!(c.line_runs.len(), 2);
    assert_eq!(c.line_runs[0], (u16::MAX, 1));
    assert_eq!(c.line_runs[1], (1, 1));
}
