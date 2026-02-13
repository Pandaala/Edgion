//! AST → Bytecode compiler for EdgionDSL
//!
//! Compiles AST statements and expressions to a flat sequence of bytecode instructions.
//! Key features:
//!   - Local variable resolution with scope tracking
//!   - Mutable variable checking
//!   - Short-circuit evaluation for && and ||
//!   - Jump backpatching for if/else/while/for
//!   - Namespace method resolution (req.header → BuiltinId::ReqHeader)

use std::collections::HashMap;

use super::ast::*;
use super::bytecode::*;
use super::error::CompileError;

/// Local variable entry
#[derive(Debug, Clone)]
struct Local {
    name: String,
    slot: u16,
    mutable: bool,
    #[allow(dead_code)]
    depth: u16,
}

/// Compiler state
pub struct Compiler {
    code: Vec<OpCode>,
    constants: Vec<Constant>,
    /// Map from constant value to index for deduplication
    const_str_map: HashMap<String, u16>,
    const_int_map: HashMap<i64, u16>,
    locals: Vec<Local>,
    next_slot: u16,
    scope_depth: u16,
    loop_depth: u16,
    max_loop_depth: u16,
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
            Some(OpCode::ReturnNext) | Some(OpCode::ReturnDeny)
        )
    }

    fn emit(&mut self, op: OpCode) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        idx
    }

    fn current_offset(&self) -> usize {
        self.code.len()
    }

    /// Patch a jump instruction at `idx` to jump to current position
    fn patch_jump(&mut self, idx: usize) {
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

    fn add_str_const(&mut self, s: &str) -> u16 {
        if let Some(&idx) = self.const_str_map.get(s) {
            return idx;
        }
        let idx = self.constants.len() as u16;
        self.constants.push(Constant::Str(s.to_string()));
        self.const_str_map.insert(s.to_string(), idx);
        idx
    }

    fn add_int_const(&mut self, n: i64) -> u16 {
        if let Some(&idx) = self.const_int_map.get(&n) {
            return idx;
        }
        let idx = self.constants.len() as u16;
        self.constants.push(Constant::Int(n));
        self.const_int_map.insert(n, idx);
        idx
    }

    fn add_regex_const(&mut self, pattern: &str) -> Result<u16, CompileError> {
        // Validate regex at compile time
        regex::Regex::new(pattern)
            .map_err(|e| CompileError::new(format!("invalid regex '{}': {}", pattern, e)))?;
        let idx = self.constants.len() as u16;
        self.constants.push(Constant::Regex(pattern.to_string()));
        Ok(idx)
    }

    // ==================== Locals ====================

    fn declare_local(&mut self, name: &str, mutable: bool) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.locals.push(Local {
            name: name.to_string(),
            slot,
            mutable,
            depth: self.scope_depth,
        });
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    // ==================== Statement Compilation ====================

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match &stmt.kind {
            StmtKind::Let {
                name,
                mutable,
                value,
            } => {
                self.compile_expr(value)?;
                let slot = self.declare_local(name, *mutable);
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
                        return Err(CompileError::new(format!(
                            "undefined variable '{}'",
                            name
                        )));
                    }
                }
            }

            StmtKind::If {
                branches,
                else_body,
            } => {
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

            StmtKind::ReturnNext => {
                self.emit(OpCode::ReturnNext);
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

            for stmt in body {
                self.compile_stmt(stmt)?;
            }

            // Jump to end after body (skip remaining branches)
            if i < branches.len() - 1 || else_body.is_some() {
                let end_jump = self.emit(OpCode::Jump(0)); // placeholder
                end_jumps.push(end_jump);
            }

            self.patch_jump(false_jump);
        }

        if let Some(else_stmts) = else_body {
            for stmt in else_stmts {
                self.compile_stmt(stmt)?;
            }
        }

        // Patch all end jumps
        for jmp in end_jumps {
            self.patch_jump(jmp);
        }

        Ok(())
    }

    fn compile_for_range(
        &mut self,
        var_name: &str,
        start: &Expr,
        end: &Expr,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        // Initialize: i = start
        self.compile_expr(start)?;
        let var_slot = self.declare_local(var_name, true);
        self.emit(OpCode::SetLocal(var_slot));
        self.emit(OpCode::LoopInit);

        let loop_start = self.current_offset();

        // Condition: i < end
        self.emit(OpCode::GetLocal(var_slot));
        self.compile_expr(end)?;
        self.emit(OpCode::Less);
        let exit_jump = self.emit(OpCode::JumpIfFalse(0));

        // Body
        for stmt in body {
            self.compile_stmt(stmt)?;
        }

        // Increment: i = i + 1
        self.emit(OpCode::GetLocal(var_slot));
        let one = self.add_int_const(1);
        self.emit(OpCode::LoadConst(one));
        self.emit(OpCode::Add);
        self.emit(OpCode::SetLocal(var_slot));

        // Loop back
        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        self.loop_depth -= 1;

        Ok(())
    }

    fn compile_for_in(
        &mut self,
        var_name: &str,
        iterable: &Expr,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        // Evaluate iterable → list
        self.compile_expr(iterable)?;
        let list_slot = self.declare_local("__list__", false);
        self.emit(OpCode::SetLocal(list_slot));

        // index = 0
        let zero = self.add_int_const(0);
        self.emit(OpCode::LoadConst(zero));
        let idx_slot = self.declare_local("__idx__", true);
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
        self.emit(OpCode::ListGet);
        let var_slot = self.declare_local(var_name, false);
        self.emit(OpCode::SetLocal(var_slot));

        // Body
        for stmt in body {
            self.compile_stmt(stmt)?;
        }

        // Increment: idx = idx + 1
        self.emit(OpCode::GetLocal(idx_slot));
        let one = self.add_int_const(1);
        self.emit(OpCode::LoadConst(one));
        self.emit(OpCode::Add);
        self.emit(OpCode::SetLocal(idx_slot));

        // Loop back
        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        self.loop_depth -= 1;

        Ok(())
    }

    fn compile_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        self.loop_depth += 1;
        if self.loop_depth > self.max_loop_depth {
            self.max_loop_depth = self.loop_depth;
        }

        self.emit(OpCode::LoopInit);
        let loop_start = self.current_offset();

        self.compile_expr(condition)?;
        let exit_jump = self.emit(OpCode::JumpIfFalse(0));

        for stmt in body {
            self.compile_stmt(stmt)?;
        }

        let back_offset = loop_start as i32 - (self.current_offset() as i32 + 1);
        self.emit(OpCode::LoopBack(back_offset));

        self.patch_jump(exit_jump);
        self.loop_depth -= 1;

        Ok(())
    }

    // ==================== Expression Compilation ====================

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        match &expr.kind {
            ExprKind::StringLit(s) => {
                let idx = self.add_str_const(s);
                self.emit(OpCode::LoadConst(idx));
            }

            ExprKind::IntLit(n) => {
                let idx = self.add_int_const(*n);
                self.emit(OpCode::LoadConst(idx));
            }

            ExprKind::BoolLit(true) => {
                self.emit(OpCode::LoadTrue);
            }

            ExprKind::BoolLit(false) => {
                self.emit(OpCode::LoadFalse);
            }

            ExprKind::NilLit => {
                self.emit(OpCode::LoadNil);
            }

            ExprKind::Ident(name) => {
                let local = self.resolve_local(name);
                match local {
                    Some(local) => {
                        self.emit(OpCode::GetLocal(local.slot));
                    }
                    None => {
                        return Err(CompileError::new(format!(
                            "undefined variable '{}'",
                            name
                        )));
                    }
                }
            }

            ExprKind::BinaryOp { op, left, right } => {
                match op {
                    // Short-circuit &&
                    BinOp::And => {
                        self.compile_expr(left)?;
                        let false_jump = self.emit(OpCode::JumpIfFalse(0));
                        self.compile_expr(right)?;
                        let end_jump = self.emit(OpCode::Jump(0));
                        self.patch_jump(false_jump);
                        self.emit(OpCode::LoadFalse);
                        self.patch_jump(end_jump);
                    }
                    // Short-circuit ||
                    BinOp::Or => {
                        self.compile_expr(left)?;
                        // If truthy, skip right side
                        self.emit(OpCode::Not);
                        let false_jump = self.emit(OpCode::JumpIfFalse(0));
                        self.compile_expr(right)?;
                        let end_jump = self.emit(OpCode::Jump(0));
                        self.patch_jump(false_jump);
                        self.emit(OpCode::LoadTrue);
                        self.patch_jump(end_jump);
                    }
                    _ => {
                        self.compile_expr(left)?;
                        self.compile_expr(right)?;
                        let opcode = match op {
                            BinOp::Add => OpCode::Add,
                            BinOp::Sub => OpCode::Sub,
                            BinOp::Mul => OpCode::Mul,
                            BinOp::Div => OpCode::Div,
                            BinOp::Eq => OpCode::Equal,
                            BinOp::Ne => OpCode::NotEqual,
                            BinOp::Lt => OpCode::Less,
                            BinOp::Gt => OpCode::Greater,
                            BinOp::Le => OpCode::LessEqual,
                            BinOp::Ge => OpCode::GreaterEqual,
                            _ => unreachable!(),
                        };
                        self.emit(opcode);
                    }
                }
            }

            ExprKind::UnaryOp { op, operand } => {
                self.compile_expr(operand)?;
                match op {
                    UnaryOp::Not => {
                        self.emit(OpCode::Not);
                    }
                    UnaryOp::Neg => {
                        self.emit(OpCode::Neg);
                    }
                }
            }

            ExprKind::MethodCall {
                object,
                method,
                args,
            } => {
                self.compile_method_call(object, method, args)?;
            }

            ExprKind::FnCall { name, args } => {
                self.compile_fn_call(name, args)?;
            }

            ExprKind::FieldAccess { object, field } => {
                // Field access is sugar for 0-arg method call
                self.compile_method_call(object, field, &[])?;
            }
        }
        Ok(())
    }

    fn compile_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<(), CompileError> {
        // Check if object is a namespace (req, resp, ctx)
        if let ExprKind::Ident(namespace) = &object.kind {
            if let Some(builtin_id) = resolve_namespace_method(namespace, method) {
                // Compile arguments onto stack
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.emit(OpCode::CallBuiltin(builtin_id, args.len() as u8));
                return Ok(());
            }
        }

        // String methods
        match method {
            "starts_with" => {
                if args.len() != 1 {
                    return Err(CompileError::new("starts_with() requires 1 argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::StartsWith);
            }
            "ends_with" => {
                if args.len() != 1 {
                    return Err(CompileError::new("ends_with() requires 1 argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::EndsWith);
            }
            "contains" => {
                if args.len() != 1 {
                    return Err(CompileError::new("contains() requires 1 argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::Contains);
            }
            "matches" => {
                if args.len() != 1 {
                    return Err(CompileError::new("matches() requires 1 string literal argument"));
                }
                // Regex pattern must be a string literal (compile-time validation)
                let pattern = match &args[0].kind {
                    ExprKind::StringLit(s) => s.clone(),
                    _ => {
                        return Err(CompileError::new(
                            "matches() pattern must be a string literal",
                        ));
                    }
                };
                let regex_idx = self.add_regex_const(&pattern)?;
                self.compile_expr(object)?;
                self.emit(OpCode::Matches(regex_idx));
            }
            _ => {
                return Err(CompileError::new(format!(
                    "unknown method: .{}()",
                    method
                )));
            }
        }
        Ok(())
    }

    fn compile_fn_call(&mut self, name: &str, args: &[Expr]) -> Result<(), CompileError> {
        let builtin_id = match name {
            "log" => BuiltinId::Log,
            "len" => BuiltinId::Len,
            "substr" => BuiltinId::Substr,
            "to_int" => BuiltinId::ToInt,
            "to_str" => BuiltinId::ToStr,
            "to_upper" => BuiltinId::ToUpper,
            "to_lower" => BuiltinId::ToLower,
            "base64_encode" => BuiltinId::Base64Encode,
            "base64_decode" => BuiltinId::Base64Decode,
            "url_encode" => BuiltinId::UrlEncode,
            "url_decode" => BuiltinId::UrlDecode,
            "sha256" => BuiltinId::Sha256,
            "md5" => BuiltinId::Md5,
            "time_now" => BuiltinId::TimeNow,
            "regex_find" => BuiltinId::RegexFind,
            "regex_replace" => BuiltinId::RegexReplace,
            _ => {
                return Err(CompileError::new(format!(
                    "unknown function: {}()",
                    name
                )));
            }
        };

        for arg in args {
            self.compile_expr(arg)?;
        }
        self.emit(OpCode::CallBuiltin(builtin_id, args.len() as u8));
        Ok(())
    }
}

/// Resolve namespace.method to BuiltinId
fn resolve_namespace_method(namespace: &str, method: &str) -> Option<BuiltinId> {
    match (namespace, method) {
        // req.* read
        ("req", "header") => Some(BuiltinId::ReqHeader),
        ("req", "method") => Some(BuiltinId::ReqMethod),
        ("req", "path") => Some(BuiltinId::ReqPath),
        ("req", "query") => Some(BuiltinId::ReqQuery),
        ("req", "query_string") => Some(BuiltinId::ReqQueryString),
        ("req", "cookie") => Some(BuiltinId::ReqCookie),
        ("req", "client_ip") => Some(BuiltinId::ReqClientIp),
        ("req", "remote_ip") => Some(BuiltinId::ReqRemoteIp),
        ("req", "path_param") => Some(BuiltinId::ReqPathParam),
        ("req", "header_names") => Some(BuiltinId::ReqHeaderNames),
        ("req", "scheme") => Some(BuiltinId::ReqScheme),
        ("req", "host") => Some(BuiltinId::ReqHost),
        ("req", "uri") => Some(BuiltinId::ReqUri),
        ("req", "content_type") => Some(BuiltinId::ReqContentType),
        ("req", "has_header") => Some(BuiltinId::ReqHasHeader),

        // req.* mutation
        ("req", "set_header") => Some(BuiltinId::ReqSetHeader),
        ("req", "append_header") => Some(BuiltinId::ReqAppendHeader),
        ("req", "remove_header") => Some(BuiltinId::ReqRemoveHeader),
        ("req", "set_uri") => Some(BuiltinId::ReqSetUri),
        ("req", "set_host") => Some(BuiltinId::ReqSetHost),
        ("req", "set_method") => Some(BuiltinId::ReqSetMethod),

        // resp.*
        ("resp", "set_header") => Some(BuiltinId::RespSetHeader),
        ("resp", "append_header") => Some(BuiltinId::RespAppendHeader),
        ("resp", "remove_header") => Some(BuiltinId::RespRemoveHeader),

        // ctx.*
        ("ctx", "get") => Some(BuiltinId::CtxGet),
        ("ctx", "set") => Some(BuiltinId::CtxSet),
        ("ctx", "remove") => Some(BuiltinId::CtxRemove),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::edgion_plugins::dsl::lang::parser::parse_program;

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
    fn test_compile_string_methods() {
        let source = r#"
            let s = "hello"
            let b1 = s.starts_with("he")
            let b2 = s.ends_with("lo")
            let b3 = s.contains("ll")
        "#;
        let prog = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&prog).unwrap();
        assert!(compiled.code.contains(&OpCode::StartsWith));
        assert!(compiled.code.contains(&OpCode::EndsWith));
        assert!(compiled.code.contains(&OpCode::Contains));
    }

    #[test]
    fn test_compile_matches() {
        let source = r#"
            let ua = "bot-crawler"
            let is_bot = ua.matches("(?i)bot|crawler")
        "#;
        let prog = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&prog).unwrap();
        assert!(compiled
            .constants
            .iter()
            .any(|c| matches!(c, Constant::Regex(_))));
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
