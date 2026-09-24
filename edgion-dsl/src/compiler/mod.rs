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

pub(crate) use builtins::builtin_arity;

use std::collections::HashMap;

use super::ast::*;
use super::bytecode::*;
use super::error::CompileError;

pub(super) const MAX_REGEX_PATTERN_LEN: usize = 1024;
pub(crate) const STREAMING_REGEX_DFA_SIZE_LIMIT: usize = 1 << 20;

/// Validate that a `.matches()` pattern can be built as the bounded forward
/// DFA used by incremental String/Body matching.
pub(crate) fn validate_streaming_regex(pattern: &str) -> Result<(), String> {
    let mut builder = regex_automata::dfa::dense::Builder::new();
    builder.configure(regex_automata::dfa::dense::Config::new().dfa_size_limit(Some(STREAMING_REGEX_DFA_SIZE_LIMIT)));
    builder.build(pattern).map(|_| ()).map_err(|error| error.to_string())
}

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
    pub(super) const_streaming_regex_map: HashMap<String, u16>,
    pub(super) const_json_path_map: HashMap<String, u16>,
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
            const_streaming_regex_map: HashMap::new(),
            const_json_path_map: HashMap::new(),
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
#[path = "../../tests/unit/compiler/mod_test/source_map_tests.rs"]
mod source_map_tests;
