//! AST → Bytecode compiler for EdgionDSL
//!
//! Compiles AST statements and expressions to a flat sequence of bytecode instructions.
//! Key features:
//!   - Local variable resolution with scope tracking
//!   - Mutable variable checking
//!   - Short-circuit evaluation for && and ||
//!   - Jump backpatching for if/else/while/for
//!   - Namespace method resolution (req.header → BuiltinId::ReqHeader)

mod builtins;
mod emit;
mod expr;
mod loop_;
mod stmt;

pub(crate) use builtins::builtin_expected_argc;

use std::collections::HashMap;

use super::ast::*;
use super::bytecode::*;
use super::error::CompileError;

pub(super) const MAX_REGEX_PATTERN_LEN: usize = 1024;

/// Local variable entry
#[derive(Debug, Clone)]
pub(super) struct Local {
    pub(super) name: String,
    pub(super) slot: u16,
    pub(super) mutable: bool,
    pub(super) depth: u16,
}

/// Compiler state
pub struct Compiler {
    pub(super) code: Vec<OpCode>,
    pub(super) constants: Vec<Constant>,
    /// Secondary indices for O(1) constant deduplication (map value → pool index).
    pub(super) const_str_map: HashMap<String, u16>,
    pub(super) const_int_map: HashMap<i64, u16>,
    pub(super) const_regex_map: HashMap<String, u16>,
    pub(super) locals: Vec<Local>,
    pub(super) next_slot: u16,
    pub(super) scope_depth: u16,
    pub(super) loop_depth: u16,
    pub(super) max_loop_depth: u16,
    /// Per-loop break jump placeholder indices for backpatching
    pub(super) break_patch_stacks: Vec<Vec<usize>>,
    /// Per-loop continue jump placeholder indices for backpatching
    pub(super) continue_patch_stacks: Vec<Vec<usize>>,
    /// Run-length encoded mapping from emitted opcode PC to source line.
    /// Each entry `(count, line)` means the next `count` opcodes originate from `line`.
    pub(super) line_runs: Vec<(u16, u32)>,
    /// Source line of the AST node currently being lowered. Defaults to 1 so opcodes
    /// emitted for AST nodes without a span still get a deterministic line attribution.
    pub(super) current_line: u32,
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            const_str_map: HashMap::new(),
            const_int_map: HashMap::new(),
            const_regex_map: HashMap::new(),
            locals: Vec::new(),
            next_slot: 0,
            scope_depth: 0,
            loop_depth: 0,
            max_loop_depth: 0,
            break_patch_stacks: Vec::new(),
            continue_patch_stacks: Vec::new(),
            line_runs: Vec::new(),
            current_line: 1,
        }
    }

    /// Compile a program to bytecode (without embedding source).
    ///
    /// Used by unit tests that hand-build AST without raw source text.
    #[cfg(test)]
    pub fn compile(self, program: &Program) -> Result<CompiledScript, CompileError> {
        self.compile_inner(program, None)
    }

    /// Compile a program to bytecode and embed the original source text.
    ///
    /// The embedded source enables PC → source correlation in runtime error
    /// debug logs and offline disassembly tools.
    pub fn compile_with_source(self, program: &Program, source: &str) -> Result<CompiledScript, CompileError> {
        self.compile_inner(program, Some(source.to_string()))
    }

    fn compile_inner(mut self, program: &Program, source: Option<String>) -> Result<CompiledScript, CompileError> {
        for stmt in &program.stmts {
            self.compile_stmt(stmt)?;
        }
        // Implicit return next() at the end
        if !self.ends_with_return() {
            self.emit(OpCode::ReturnNext);
        }
        let source_map = if self.line_runs.is_empty() {
            None
        } else {
            Some(self.line_runs)
        };
        Ok(CompiledScript {
            version: BYTECODE_VERSION,
            code: self.code,
            constants: self.constants,
            local_count: self.next_slot,
            max_loop_depth: self.max_loop_depth,
            source,
            source_map,
        })
    }

    fn ends_with_return(&self) -> bool {
        matches!(
            self.code.last(),
            Some(OpCode::ReturnNext)
                | Some(OpCode::ReturnDeny)
                | Some(OpCode::ReturnTerminate)
                | Some(OpCode::ReturnGrpcError)
        )
    }
}

#[cfg(test)]
mod source_map_tests {
    use super::super::parser::parse_program;
    use super::*;

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
}
