use super::super::parser::parse_program;
use super::*;

impl Compiler {
    /// Compile a program to bytecode without embedding source text.
    pub fn compile(self, program: &Program) -> Result<CompiledScript, CompileError> {
        self.compile_inner(program, None)
    }
}

#[test]
fn test_line_of_distinguishes_lines() {
    // line 1: `let x = 1`
    // line 2: `let y = 2 / 0`
    let prog = parse_program("let x = 1\nlet y = 2 / 0\n").expect("parse");
    let script = Compiler::new().compile(&prog).expect("compile");

    let div_pc = script
        .code
        .iter()
        .position(|op| matches!(op, OpCode::Div))
        .expect("Div opcode must be emitted");
    assert_eq!(script.line_of(div_pc), Some(2));

    let first_load_const_pc = script
        .code
        .iter()
        .position(|op| matches!(op, OpCode::LoadConst(_)))
        .expect("LoadConst opcode must be emitted");
    assert_eq!(script.line_of(first_load_const_pc), Some(1));
}

#[test]
fn test_every_op_has_a_line() {
    let prog = parse_program("let x = 1\nlet y = 2 / 0\n").expect("parse");
    let script = Compiler::new().compile(&prog).expect("compile");
    for pc in 0..script.code.len() {
        assert!(
            script.line_of(pc).is_some(),
            "no line for pc {} (op {:?})",
            pc,
            script.code[pc]
        );
    }
}

#[test]
fn test_source_map_uses_run_length_compression() {
    // A single-line script should collapse to a single RLE run covering the whole code.
    let prog = parse_program("let x = 1 + 2 + 3 + 4 + 5\n").expect("parse");
    let script = Compiler::new().compile(&prog).expect("compile");
    let runs = script.source_map.as_ref().expect("source_map present");
    assert_eq!(
        runs.len(),
        1,
        "single-line script should collapse to 1 run, got {:?}",
        runs
    );
    assert_eq!(runs[0].1, 1);
    assert_eq!(runs[0].0 as usize, script.code.len());
}
