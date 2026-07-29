//! Expression compilation

use super::super::ast::*;
use super::super::bytecode::*;
use super::super::error::CompileError;
use super::builtins::{builtin_expected_argc, resolve_namespace_method};
use super::Compiler;

impl Compiler {
    pub(super) fn compile_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        if let Some(span) = expr.span {
            self.current_line = span.line;
        }
        match &expr.kind {
            ExprKind::StringLit(s) => {
                let idx = self.add_str_const(s)?;
                self.emit(OpCode::LoadConst(idx));
            }

            ExprKind::IntLit(n) => {
                let idx = self.add_int_const(*n)?;
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
                        return Err(CompileError::new(format!("undefined variable '{}'", name)));
                    }
                }
            }

            ExprKind::BinaryOp { op, left, right } => {
                match op {
                    // Short-circuit && — always yields a Bool (Logical AND).
                    BinOp::And => {
                        self.compile_expr(left)?;
                        let false_jump = self.emit(OpCode::JumpIfFalse(0));
                        self.compile_expr(right)?;
                        // Coerce the RHS to Bool(is_truthy) via double Not so the result
                        // type is consistent with the LoadFalse short-circuit branch.
                        self.emit(OpCode::Not);
                        self.emit(OpCode::Not);
                        let end_jump = self.emit(OpCode::Jump(0));
                        self.patch_jump(false_jump);
                        self.emit(OpCode::LoadFalse);
                        self.patch_jump(end_jump);
                    }
                    // Short-circuit || — always yields a Bool (Logical OR).
                    BinOp::Or => {
                        self.compile_expr(left)?;
                        // If truthy, skip right side
                        self.emit(OpCode::Not);
                        let false_jump = self.emit(OpCode::JumpIfFalse(0));
                        self.compile_expr(right)?;
                        // Coerce the RHS to Bool(is_truthy) via double Not so the result
                        // type is consistent with the LoadTrue short-circuit branch.
                        self.emit(OpCode::Not);
                        self.emit(OpCode::Not);
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
                            // And/Or are handled in dedicated arms above
                            BinOp::And | BinOp::Or => {
                                return Err(CompileError::new("internal error: And/Or should be handled above"));
                            }
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

            ExprKind::MethodCall { object, method, args } => {
                self.compile_method_call(object, method, args)?;
            }

            ExprKind::FnCall { name, args } => {
                self.compile_fn_call(name, args)?;
            }

            ExprKind::FieldAccess { object, field } => {
                // Field access is sugar for 0-arg method call
                self.compile_method_call(object, field, &[])?;
            }

            ExprKind::MapLiteral(pairs) => {
                let n = pairs.len();
                if n > u16::MAX as usize {
                    return Err(CompileError::new("map literal exceeds maximum key count (65535)"));
                }
                // Push key/value pairs interleaved: key0, val0, key1, val1, ...
                for (key, val) in pairs {
                    self.compile_expr(key)?;
                    self.compile_expr(val)?;
                }
                self.emit(OpCode::MakeMap(n as u16));
            }

            ExprKind::Index { object, key } => {
                self.compile_expr(object)?;
                self.compile_expr(key)?;
                self.emit(OpCode::IndexGet);
            }
        }
        Ok(())
    }

    fn compile_method_call(&mut self, object: &Expr, method: &str, args: &[Expr]) -> Result<(), CompileError> {
        // Check if object is a namespace (req, resp, ctx)
        if let ExprKind::Ident(namespace) = &object.kind {
            if let Some(builtin_id) = resolve_namespace_method(namespace, method) {
                // Validate argument count at compile time
                if let Some(expected) = builtin_expected_argc(&builtin_id) {
                    if args.len() != expected {
                        return Err(CompileError::new(format!(
                            "{}.{}() expects {} argument(s), got {}",
                            namespace,
                            method,
                            expected,
                            args.len()
                        )));
                    }
                }
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
            "len" => {
                if !args.is_empty() {
                    return Err(CompileError::new("len() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::TextLen);
            }
            "is_empty" => {
                if !args.is_empty() {
                    return Err(CompileError::new("is_empty() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::TextIsEmpty);
            }
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
                        return Err(CompileError::new("matches() pattern must be a string literal"));
                    }
                };
                let regex_idx = self.add_streaming_regex_const(&pattern)?;
                self.compile_expr(object)?;
                self.emit(OpCode::Matches(regex_idx));
            }
            "byte_len" => {
                if !args.is_empty() {
                    return Err(CompileError::new("byte_len() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodyByteLen);
            }
            "read" => {
                if args.len() != 2 {
                    return Err(CompileError::new("read() requires 2 arguments"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.compile_expr(&args[1])?;
                self.emit(OpCode::BodyRead);
            }
            "read_all" => {
                if !args.is_empty() {
                    return Err(CompileError::new("read_all() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodyReadAll);
            }
            "sha256" => {
                if !args.is_empty() {
                    return Err(CompileError::new("sha256() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodySha256);
            }
            "json_get" => {
                if args.len() != 1 {
                    return Err(CompileError::new("json_get() requires 1 string literal path argument"));
                }
                let ExprKind::StringLit(path) = &args[0].kind else {
                    return Err(CompileError::new("json_get() path must be a string literal"));
                };
                let path_idx = self.add_json_path_const(path)?;
                self.compile_expr(object)?;
                self.emit(OpCode::JsonGet(path_idx));
            }
            "clear" => {
                if !args.is_empty() {
                    return Err(CompileError::new("clear() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodyClear);
            }
            "append" => {
                if args.len() != 1 {
                    return Err(CompileError::new("append() requires 1 argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::BodyAppend);
            }
            "prepend" => {
                if args.len() != 1 {
                    return Err(CompileError::new("prepend() requires 1 argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::BodyPrepend);
            }
            "remove" => {
                if args.len() != 2 {
                    return Err(CompileError::new("remove() requires 2 arguments"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.compile_expr(&args[1])?;
                self.emit(OpCode::BodyRemove);
            }
            "replace" => {
                if args.len() != 2 {
                    return Err(CompileError::new("replace() requires 2 arguments"));
                }
                if matches!(&args[0].kind, ExprKind::StringLit(from) if from.is_empty()) {
                    return Err(CompileError::new("replace() search string must not be empty"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.compile_expr(&args[1])?;
                self.emit(OpCode::BodyReplace);
            }
            _ => {
                return Err(CompileError::new(format!("unknown method: .{}()", method)));
            }
        }
        Ok(())
    }

    fn compile_fn_call(&mut self, name: &str, args: &[Expr]) -> Result<(), CompileError> {
        // regex_find / regex_replace require a string literal pattern — consistent with matches().
        // Dynamic patterns are rejected at compile time to avoid per-request regex compilation.
        if name == "regex_find" {
            if args.len() != 2 {
                return Err(CompileError::new(format!(
                    "regex_find() requires 2 arguments, got {}",
                    args.len()
                )));
            }
            match &args[1].kind {
                ExprKind::StringLit(pattern) => {
                    let idx = self.add_regex_const(pattern)?;
                    self.compile_expr(&args[0])?;
                    self.emit(OpCode::RegexFind(idx));
                    return Ok(());
                }
                _ => return Err(CompileError::new("regex_find() pattern must be a string literal")),
            }
        } else if name == "regex_replace" {
            if args.len() != 3 {
                return Err(CompileError::new(format!(
                    "regex_replace() requires 3 arguments, got {}",
                    args.len()
                )));
            }
            match &args[1].kind {
                ExprKind::StringLit(pattern) => {
                    let idx = self.add_regex_const(pattern)?;
                    self.compile_expr(&args[0])?;
                    self.compile_expr(&args[2])?;
                    self.emit(OpCode::RegexReplace(idx));
                    return Ok(());
                }
                _ => return Err(CompileError::new("regex_replace() pattern must be a string literal")),
            }
        } else if name == "json_get" {
            if args.len() != 2 {
                return Err(CompileError::new(format!(
                    "json_get() requires 2 arguments, got {}",
                    args.len()
                )));
            }
            let ExprKind::StringLit(path) = &args[1].kind else {
                return Err(CompileError::new("json_get() path must be a string literal"));
            };
            let path_idx = self.add_json_path_const(path)?;
            self.compile_expr(&args[0])?;
            self.emit(OpCode::JsonGet(path_idx));
            return Ok(());
        } else if name == "log_fmt" {
            // log_fmt(template_literal, sensitive_arg1, sensitive_arg2, ...)
            //
            // Template MUST be a string literal — this preserves the safety contract
            // that plain content in the rendered entry only originates from script
            // source, never from request data. Trailing args are all treated as
            // sensitive (passed through `apply_sensitive` at runtime).
            if args.is_empty() {
                return Err(CompileError::new(
                    "log_fmt() requires at least 1 argument (the template string literal)",
                ));
            }
            let template = match &args[0].kind {
                ExprKind::StringLit(s) => s.clone(),
                _ => {
                    return Err(CompileError::new("log_fmt() template must be a string literal"));
                }
            };
            let placeholder_count = template.matches("{}").count();
            let value_count = args.len() - 1;
            if placeholder_count != value_count {
                return Err(CompileError::new(format!(
                    "log_fmt() template has {} `{{}}` placeholder(s) but {} value argument(s) supplied",
                    placeholder_count, value_count
                )));
            }
            // Push template + each value onto the stack in source order.
            for arg in args {
                self.compile_expr(arg)?;
            }
            self.emit(OpCode::CallBuiltin(BuiltinId::LogFmt, args.len() as u8));
            return Ok(());
        }

        // (builtin_id, expected_argc) — None means variadic / no check
        let (builtin_id, expected_argc): (BuiltinId, Option<usize>) = match name {
            "log" => (BuiltinId::Log, Some(1)),
            "len" => (BuiltinId::Len, Some(1)),
            "substr" => (BuiltinId::Substr, Some(3)),
            "to_int" => (BuiltinId::ToInt, Some(1)),
            "to_str" => (BuiltinId::ToStr, Some(1)),
            "to_upper" => (BuiltinId::ToUpper, Some(1)),
            "to_lower" => (BuiltinId::ToLower, Some(1)),
            "base64_encode" => (BuiltinId::Base64Encode, Some(1)),
            "base64_decode" => (BuiltinId::Base64Decode, Some(1)),
            "url_encode" => (BuiltinId::UrlEncode, Some(1)),
            "url_decode" => (BuiltinId::UrlDecode, Some(1)),
            "sha256" => (BuiltinId::Sha256, Some(1)),
            "md5" => (BuiltinId::Md5, Some(1)),
            "time_now" => (BuiltinId::TimeNow, Some(0)),
            "trim" => (BuiltinId::Trim, Some(1)),
            "split" => (BuiltinId::Split, Some(2)),
            "range" => (BuiltinId::Range, Some(2)),
            // P3 utility functions
            "hmac_sha256" => (BuiltinId::HmacSha256, Some(2)),
            "uuid" => (BuiltinId::Uuid, Some(0)),
            "hex_encode" => (BuiltinId::HexEncode, Some(1)),
            "hex_decode" => (BuiltinId::HexDecode, Some(1)),
            "time_format" => (BuiltinId::TimeFormat, Some(2)),
            "ip_in_cidr" => (BuiltinId::IpInCidr, Some(2)),
            "join" => (BuiltinId::Join, Some(2)),
            "log_warn" => (BuiltinId::LogWarn, Some(1)),
            "log_error" => (BuiltinId::LogError, Some(1)),
            _ => {
                return Err(CompileError::new(format!("unknown function: {}()", name)));
            }
        };

        // Validate argument count at compile time
        if let Some(expected) = expected_argc {
            if args.len() != expected {
                return Err(CompileError::new(format!(
                    "{}() expects {} argument(s), got {}",
                    name,
                    expected,
                    args.len()
                )));
            }
        }

        for arg in args {
            self.compile_expr(arg)?;
        }
        self.emit(OpCode::CallBuiltin(builtin_id, args.len() as u8));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::bytecode::{Constant, OpCode};
    use super::super::super::parser::parse_program;
    use super::super::Compiler;

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
            .any(|c| matches!(c, Constant::StreamingRegex(_))));
    }

    #[test]
    fn test_compile_body_read_and_mutation_methods() {
        let source = r#"
            let chars = req.body.len()
            let empty = req.body.is_empty()
            let bytes = req.body.byte_len()
            let prefix = req.body.read(0, 16)
            let whole = req.body.read_all()
            let digest = req.body.sha256()
            let selected = req.body.json_get("$.user[0].name")
            req.body.clear()
            req.body.append("tail")
            req.body.prepend("head")
            req.body.remove(1, 2)
            req.body.replace("from", "to")
        "#;
        let program = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&program).unwrap();
        for expected in [
            OpCode::TextLen,
            OpCode::TextIsEmpty,
            OpCode::BodyByteLen,
            OpCode::BodyRead,
            OpCode::BodyReadAll,
            OpCode::BodySha256,
            OpCode::BodyClear,
            OpCode::BodyAppend,
            OpCode::BodyPrepend,
            OpCode::BodyRemove,
            OpCode::BodyReplace,
        ] {
            assert!(compiled.code.contains(&expected), "missing opcode {expected:?}");
        }
        let json_path_index = compiled
            .constants
            .iter()
            .position(|constant| matches!(constant, Constant::JsonPath(path) if path == "$.user[0].name"))
            .unwrap() as u16;
        assert!(compiled.code.contains(&OpCode::JsonGet(json_path_index)));
    }

    #[test]
    fn test_json_get_function_and_method_share_definite_path_constants() {
        let source = r#"
            let a = json_get("{\"a\":1}", "$.a")
            let b = req.body.json_get("$.a")
        "#;
        let program = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&program).unwrap();
        assert_eq!(
            compiled
                .constants
                .iter()
                .filter(|constant| matches!(constant, Constant::JsonPath(path) if path == "$.a"))
                .count(),
            1
        );
        assert_eq!(
            compiled
                .code
                .iter()
                .filter(|opcode| matches!(opcode, OpCode::JsonGet(_)))
                .count(),
            2
        );
    }

    #[test]
    fn test_json_get_rejects_dynamic_and_indefinite_paths() {
        for source in [
            r#"let path = "$.a"
               let value = req.body.json_get(path)"#,
            r#"let value = json_get("{}", "a.b")"#,
            r#"let value = req.body.json_get("$.*")"#,
        ] {
            let program = parse_program(source).unwrap();
            assert!(Compiler::new().compile(&program).is_err());
        }
    }

    #[test]
    fn test_matches_rejects_unicode_word_boundaries_but_accepts_ascii_boundaries() {
        let unicode = parse_program(r#"let found = "word".matches("\\bword\\b")"#).unwrap();
        let error = Compiler::new().compile(&unicode).unwrap_err().to_string();
        assert!(error.contains("not compatible with streaming matches"));

        let ascii = parse_program(r#"let found = "word".matches("(?-u:\\b)word(?-u:\\b)")"#).unwrap();
        Compiler::new().compile(&ascii).unwrap();

        // regex_find is not a streaming Body operation and keeps its existing dialect.
        let find = parse_program(r#"let found = regex_find("word", "\\bword\\b")"#).unwrap();
        Compiler::new().compile(&find).unwrap();
    }

    #[test]
    fn test_body_replace_rejects_literal_empty_search() {
        let program = parse_program(r#"req.body.replace("", "x")"#).unwrap();
        let error = Compiler::new().compile(&program).unwrap_err().to_string();
        assert!(error.contains("must not be empty"));
    }
}
