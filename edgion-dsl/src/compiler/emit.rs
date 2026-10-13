//! Bytecode emission, constants, and scope/local management

use super::super::bytecode::*;
use super::super::error::CompileError;
use super::{validate_streaming_regex, Compiler, Local, MAX_REGEX_PATTERN_LEN};

impl Compiler {
    pub(super) fn emit(&mut self, op: OpCode) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        self.record_line();
        idx
    }

    /// Append the current source line to `line_runs` using run-length encoding.
    /// Extends the trailing run when the line matches and the count hasn't saturated;
    /// otherwise starts a new run. Keeps the map's PC coverage in lock-step with `code`.
    fn record_line(&mut self) {
        let line = self.current_line;
        match self.line_runs.last_mut() {
            Some((count, last_line)) if *last_line == line && *count < u16::MAX => {
                *count += 1;
            }
            _ => self.line_runs.push((1, line)),
        }
    }

    pub(super) fn current_offset(&self) -> usize {
        self.code.len()
    }

    /// Patch a jump instruction at `idx` to jump to current position
    pub(super) fn patch_jump(&mut self, idx: usize) {
        let target = self.code.len() as i32;
        let source = idx as i32 + 1; // jump is relative to NEXT instruction
        let offset = target - source;
        match &mut self.code[idx] {
            OpCode::Jump(ref mut o) => *o = offset,
            OpCode::JumpIfFalse(ref mut o) => *o = offset,
            _ => {}
        }
    }

    // ==================== Constants ====================

    /// Check that the constants pool won't overflow u16.
    fn check_const_limit(&self) -> Result<u16, CompileError> {
        let len = self.constants.len();
        if len >= u16::MAX as usize {
            return Err(CompileError::new(
                "constant pool overflow: too many constants (max 65535)",
            ));
        }
        Ok(len as u16)
    }

    pub(super) fn add_str_const(&mut self, s: &str) -> Result<u16, CompileError> {
        if let Some(&idx) = self.const_str_map.get(s) {
            return Ok(idx);
        }
        let idx = self.check_const_limit()?;
        let owned = s.to_string();
        self.const_str_map.insert(owned.clone(), idx);
        self.constants.push(Constant::Str(owned));
        Ok(idx)
    }

    pub(super) fn add_int_const(&mut self, n: i64) -> Result<u16, CompileError> {
        if let Some(&idx) = self.const_int_map.get(&n) {
            return Ok(idx);
        }
        let idx = self.check_const_limit()?;
        self.const_int_map.insert(n, idx);
        self.constants.push(Constant::Int(n));
        Ok(idx)
    }

    pub(super) fn add_regex_const(&mut self, pattern: &str) -> Result<u16, CompileError> {
        if pattern.len() > MAX_REGEX_PATTERN_LEN {
            return Err(CompileError::new(format!(
                "regex pattern too long: {} (max {})",
                pattern.len(),
                MAX_REGEX_PATTERN_LEN
            )));
        }
        if let Some(&idx) = self.const_regex_map.get(pattern) {
            return Ok(idx);
        }
        // Validate regex at compile time; cap DFA memory to mirror the runtime VM limit.
        regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .dfa_size_limit(1 << 20)
            .build()
            .map_err(|e| CompileError::new(format!("invalid regex '{}': {}", pattern, e)))?;
        let idx = self.check_const_limit()?;
        let owned = pattern.to_string();
        self.const_regex_map.insert(owned.clone(), idx);
        self.constants.push(Constant::Regex(owned));
        Ok(idx)
    }

    pub(super) fn add_streaming_regex_const(&mut self, pattern: &str) -> Result<u16, CompileError> {
        if pattern.len() > MAX_REGEX_PATTERN_LEN {
            return Err(CompileError::new(format!(
                "regex pattern too long: {} (max {})",
                pattern.len(),
                MAX_REGEX_PATTERN_LEN
            )));
        }
        if let Some(&idx) = self.const_streaming_regex_map.get(pattern) {
            return Ok(idx);
        }
        validate_streaming_regex(pattern)
            .map_err(|error| CompileError::new(format!("regex is not compatible with streaming matches: {}", error)))?;
        let idx = self.check_const_limit()?;
        let owned = pattern.to_string();
        self.const_streaming_regex_map.insert(owned.clone(), idx);
        self.constants.push(Constant::StreamingRegex(owned));
        Ok(idx)
    }

    pub(super) fn add_json_path_const(&mut self, path: &str) -> Result<u16, CompileError> {
        if let Some(&idx) = self.const_json_path_map.get(path) {
            return Ok(idx);
        }
        crate::json_path::DefiniteJsonPath::parse(path)
            .map_err(|error| CompileError::new(format!("invalid definite JSON path: {}", error)))?;
        let idx = self.check_const_limit()?;
        let owned = path.to_string();
        self.const_json_path_map.insert(owned.clone(), idx);
        self.constants.push(Constant::JsonPath(owned));
        Ok(idx)
    }

    pub(super) fn add_json_select_const(&mut self, entries: Vec<JsonSelectEntry>) -> Result<u16, CompileError> {
        let idx = self.check_const_limit()?;
        self.constants.push(Constant::JsonSelect(entries));
        Ok(idx)
    }

    // ==================== Locals ====================

    /// Reserve physical storage without exposing a source-level binding.
    pub(super) fn allocate_local_slot(&mut self) -> Result<u16, CompileError> {
        if self.next_slot == u16::MAX {
            return Err(CompileError::new("too many local variables (max 65534)"));
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        Ok(slot)
    }

    pub(super) fn declare_local(&mut self, name: &str, mutable: bool) -> Result<u16, CompileError> {
        let slot = self.allocate_local_slot()?;
        self.locals.push(Local {
            name: name.to_string(),
            slot,
            mutable,
            depth: self.scope_depth,
        });
        Ok(slot)
    }

    pub(super) fn resolve_local(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    pub(super) fn enter_scope(&mut self) {
        self.scope_depth += 1;
    }

    pub(super) fn leave_scope(&mut self) -> Result<(), CompileError> {
        // scope_depth == 0 here is a compiler-internal invariant violation
        // (an unbalanced enter/leave) that no user-supplied DSL source can reach.
        // Per the graceful-degrade policy, surface it as a CompileError instead of
        // panicking, so a future compiler bug fails the config load cleanly rather
        // than unwinding the preparse task.
        if self.scope_depth == 0 {
            return Err(CompileError::new("leave_scope called at depth 0"));
        }
        // Pop locals declared in the current scope
        while let Some(local) = self.locals.last() {
            if local.depth < self.scope_depth {
                break;
            }
            self.locals.pop();
        }
        self.scope_depth = self.scope_depth.saturating_sub(1);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/compiler/emit_test/record_line_tests.rs"]
mod record_line_tests;

#[cfg(test)]
#[path = "../../tests/unit/compiler/emit_test/scope_balance_tests.rs"]
mod scope_balance_tests;
