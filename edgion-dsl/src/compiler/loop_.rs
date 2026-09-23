//! Loop compilation: for-range, for-in, while

use super::super::ast::*;
use super::super::bytecode::*;
use super::super::error::CompileError;
use super::Compiler;

impl Compiler {
    pub(super) fn compile_for_range(
        &mut self,
        var_name: &str,
        start: &Expr,
        end: &Expr,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        if matches!(var_name, "req" | "resp" | "ctx") {
            return Err(CompileError::new(format!(
                "'{}' is a built-in namespace and cannot be used as a loop variable",
                var_name
            )));
        }
        self.enter_scope();
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        self.break_patch_stacks.push(vec![]);
        self.continue_patch_stacks.push(vec![]);

        // Evaluate end expression ONCE before the loop and store in a local
        self.compile_expr(end)?;
        let end_slot = self.declare_local("__range_end__", false)?;
        self.emit(OpCode::SetLocal(end_slot));

        // Initialize: i = start (immutable to user, compiler manages increment)
        self.compile_expr(start)?;
        let var_slot = self.declare_local(var_name, false)?;
        self.emit(OpCode::SetLocal(var_slot));
        self.emit(OpCode::LoopInit);

        let loop_start = self.current_offset();

        // Condition: i < end (using cached end value)
        self.emit(OpCode::GetLocal(var_slot));
        self.emit(OpCode::GetLocal(end_slot));
        self.emit(OpCode::Less);
        let exit_jump = self.emit(OpCode::JumpIfFalse(0));

        // Body
        self.enter_scope();
        for stmt in body {
            self.compile_stmt(stmt)?;
        }
        self.leave_scope()?;

        // continue target = start of increment
        let continue_target = self.current_offset();

        // Increment: i = i + 1
        self.emit(OpCode::GetLocal(var_slot));
        let one = self.add_int_const(1)?;
        self.emit(OpCode::LoadConst(one));
        self.emit(OpCode::Add);
        self.emit(OpCode::SetLocal(var_slot));

        // Loop back
        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        let break_target = self.current_offset();
        self.emit(OpCode::LoopEnd);

        let break_patches = self
            .break_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: break_patch_stacks underflow"))?;
        for idx in break_patches {
            let offset = break_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }
        let continue_patches = self
            .continue_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: continue_patch_stacks underflow"))?;
        for idx in continue_patches {
            let offset = continue_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }

        self.loop_depth -= 1;
        self.leave_scope()?;

        Ok(())
    }

    pub(super) fn compile_for_in(
        &mut self,
        var_name: &str,
        iterable: &Expr,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        if matches!(var_name, "req" | "resp" | "ctx") {
            return Err(CompileError::new(format!(
                "'{}' is a built-in namespace and cannot be used as a loop variable",
                var_name
            )));
        }
        self.enter_scope();
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        self.break_patch_stacks.push(vec![]);
        self.continue_patch_stacks.push(vec![]);

        // Evaluate iterable → list
        self.compile_expr(iterable)?;
        let list_slot = self.declare_local("__list__", false)?;
        self.emit(OpCode::SetLocal(list_slot));

        // index = 0
        let zero = self.add_int_const(0)?;
        self.emit(OpCode::LoadConst(zero));
        let idx_slot = self.declare_local("__idx__", true)?;
        self.emit(OpCode::SetLocal(idx_slot));
        self.emit(OpCode::LoopInit);

        let loop_start = self.current_offset();

        // Condition: idx < len(list)
        self.emit(OpCode::GetLocal(idx_slot));
        self.emit(OpCode::GetLocal(list_slot));
        self.emit(OpCode::ListLen);
        self.emit(OpCode::Less);
        let exit_jump = self.emit(OpCode::JumpIfFalse(0));

        // var = list[idx]
        self.emit(OpCode::GetLocal(list_slot));
        self.emit(OpCode::GetLocal(idx_slot));
        self.emit(OpCode::IndexGet);
        let var_slot = self.declare_local(var_name, false)?;
        self.emit(OpCode::SetLocal(var_slot));

        // Body
        self.enter_scope();
        for stmt in body {
            self.compile_stmt(stmt)?;
        }
        self.leave_scope()?;

        // continue target = start of increment
        let continue_target = self.current_offset();

        // Increment: idx = idx + 1
        self.emit(OpCode::GetLocal(idx_slot));
        let one = self.add_int_const(1)?;
        self.emit(OpCode::LoadConst(one));
        self.emit(OpCode::Add);
        self.emit(OpCode::SetLocal(idx_slot));

        // Loop back
        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        let break_target = self.current_offset();
        self.emit(OpCode::LoopEnd);

        let break_patches = self
            .break_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: break_patch_stacks underflow"))?;
        for idx in break_patches {
            let offset = break_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }
        let continue_patches = self
            .continue_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: continue_patch_stacks underflow"))?;
        for idx in continue_patches {
            let offset = continue_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }

        self.loop_depth -= 1;
        self.leave_scope()?;

        Ok(())
    }

    pub(super) fn compile_while(&mut self, condition: &Expr, body: &[Stmt]) -> Result<(), CompileError> {
        self.enter_scope();
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        self.break_patch_stacks.push(vec![]);
        self.continue_patch_stacks.push(vec![]);

        self.emit(OpCode::LoopInit);
        let loop_start = self.current_offset(); // LoopBack target = condition check

        self.compile_expr(condition)?;
        let exit_jump = self.emit(OpCode::JumpIfFalse(0));

        self.enter_scope();
        for stmt in body {
            self.compile_stmt(stmt)?;
        }
        self.leave_scope()?;

        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        let continue_target = self.current_offset();
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        let break_target = self.current_offset();
        self.emit(OpCode::LoopEnd);

        // Backpatch break jumps → LoopEnd so every entered loop pops its counter.
        let break_patches = self
            .break_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: break_patch_stacks underflow"))?;
        for idx in break_patches {
            let offset = break_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }
        // Backpatch continue jumps to LoopBack so each iteration is counted.
        let continue_patches = self
            .continue_patch_stacks
            .pop()
            .ok_or_else(|| CompileError::new("internal: continue_patch_stacks underflow"))?;
        for idx in continue_patches {
            let offset = continue_target as i32 - (idx as i32 + 1);
            if let OpCode::Jump(ref mut o) = self.code[idx] {
                *o = offset;
            }
        }

        self.loop_depth -= 1;
        self.leave_scope()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
