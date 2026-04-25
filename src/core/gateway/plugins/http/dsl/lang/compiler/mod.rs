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
    #[allow(dead_code)]
    pub(super) depth: u16,
}

/// Compiler state
pub struct Compiler {
    pub(super) code: Vec<OpCode>,
    pub(super) constants: Vec<Constant>,
    /// Map from constant value to index for deduplication
    pub(super) const_str_map: HashMap<String, u16>,
    pub(super) const_int_map: HashMap<i64, u16>,
    pub(super) locals: Vec<Local>,
    pub(super) next_slot: u16,
    pub(super) scope_depth: u16,
    pub(super) loop_depth: u16,
    pub(super) max_loop_depth: u16,
    /// Per-loop break jump placeholder indices for backpatching
    pub(super) break_patch_stacks: Vec<Vec<usize>>,
    /// Per-loop continue jump placeholder indices for backpatching
    pub(super) continue_patch_stacks: Vec<Vec<usize>>,
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
            locals: Vec::new(),
            next_slot: 0,
            scope_depth: 0,
            loop_depth: 0,
            max_loop_depth: 0,
            break_patch_stacks: Vec::new(),
            continue_patch_stacks: Vec::new(),
        }
    }

    /// Compile a program to bytecode
    pub fn compile(mut self, program: &Program) -> Result<CompiledScript, CompileError> {
        for stmt in &program.stmts {
            self.compile_stmt(stmt)?;
        }
        // Implicit return next() at the end
        if !self.ends_with_return() {
            self.emit(OpCode::ReturnNext);
        }
        Ok(CompiledScript {
            version: BYTECODE_VERSION,
            code: self.code,
            constants: self.constants,
            local_count: self.next_slot,
            max_loop_depth: self.max_loop_depth,
            source: None,
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
