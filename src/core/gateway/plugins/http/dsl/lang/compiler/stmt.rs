//! Statement compilation: compile_stmt + compile_if

use super::super::ast::*;
use super::super::bytecode::*;
use super::super::error::CompileError;
use super::Compiler;

impl Compiler {
    pub(super) fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        if let Some(span) = stmt.span {
            self.current_line = span.line;
        }
        match &stmt.kind {
            StmtKind::Let { name, mutable, value } => {
                // Prevent shadowing built-in namespaces
                if matches!(name.as_str(), "req" | "resp" | "ctx") {
                    return Err(CompileError::new(format!(
                        "'{}' is a built-in namespace and cannot be used as a variable name",
                        name
                    )));
                }
                self.compile_expr(value)?;
                let slot = self.declare_local(name, *mutable)?;
                self.emit(OpCode::SetLocal(slot));
            }

            StmtKind::Assign { name, value } => {
                let local = self.resolve_local(name).cloned();
                match local {
                    Some(local) if local.mutable => {
                        self.compile_expr(value)?;
                        self.emit(OpCode::SetLocal(local.slot));
                    }
                    Some(_) => {
                        return Err(CompileError::new(format!(
                            "cannot assign to immutable variable '{}'",
                            name
                        )));
                    }
                    None => {
                        return Err(CompileError::new(format!("undefined variable '{}'", name)));
                    }
                }
            }

            StmtKind::If { branches, else_body } => {
                self.compile_if(branches, else_body)?;
            }

            StmtKind::ForRange {
                var_name,
                start,
                end,
                body,
            } => {
                self.compile_for_range(var_name, start, end, body)?;
            }

            StmtKind::ForIn {
                var_name,
                iterable,
                body,
            } => {
                self.compile_for_in(var_name, iterable, body)?;
            }

            StmtKind::While { condition, body } => {
                self.compile_while(condition, body)?;
            }

            StmtKind::ReturnDeny { status, body } => {
                self.compile_expr(status)?;
                self.compile_expr(body)?;
                self.emit(OpCode::ReturnDeny);
            }

            StmtKind::Break => {
                if self.break_patch_stacks.is_empty() {
                    return Err(CompileError::new("'break' outside of loop"));
                }
                let idx = self.emit(OpCode::Jump(0));
                self.break_patch_stacks.last_mut().unwrap().push(idx);
            }

            StmtKind::Continue => {
                if self.continue_patch_stacks.is_empty() {
                    return Err(CompileError::new("'continue' outside of loop"));
                }
                let idx = self.emit(OpCode::Jump(0));
                self.continue_patch_stacks.last_mut().unwrap().push(idx);
            }

            StmtKind::ReturnNext => {
                self.emit(OpCode::ReturnNext);
            }

            StmtKind::ReturnTerminate => {
                self.emit(OpCode::ReturnTerminate);
            }

            StmtKind::ReturnGrpcError { code, message } => {
                self.compile_expr(code)?;
                self.compile_expr(message)?;
                self.emit(OpCode::ReturnGrpcError);
            }

            StmtKind::ExprStmt { expr } => {
                self.compile_expr(expr)?;
                self.emit(OpCode::Pop); // discard result
            }
        }
        Ok(())
    }

    fn compile_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_body: &Option<Vec<Stmt>>,
    ) -> Result<(), CompileError> {
        let mut end_jumps = Vec::new();

        for (i, (condition, body)) in branches.iter().enumerate() {
            self.compile_expr(condition)?;
            let false_jump = self.emit(OpCode::JumpIfFalse(0)); // placeholder

            self.enter_scope();
            for stmt in body {
                self.compile_stmt(stmt)?;
            }
            self.leave_scope();

            // Jump to end after body (skip remaining branches)
            if i < branches.len() - 1 || else_body.is_some() {
                let end_jump = self.emit(OpCode::Jump(0)); // placeholder
                end_jumps.push(end_jump);
            }

            self.patch_jump(false_jump);
        }

        if let Some(else_stmts) = else_body {
            self.enter_scope();
            for stmt in else_stmts {
                self.compile_stmt(stmt)?;
            }
            self.leave_scope();
        }

        // Patch all end jumps
        for jmp in end_jumps {
            self.patch_jump(jmp);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
