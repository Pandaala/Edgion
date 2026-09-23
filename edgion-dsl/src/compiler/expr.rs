//! Expression compilation

use super::super::ast::*;
use super::super::bytecode::*;
use super::super::error::CompileError;
use super::builtins::{builtin_arity, checked_builtin_argc, resolve_namespace_method};
use super::Compiler;
use std::collections::HashSet;

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
        if method == "json_select" {
            return self.compile_json_select(object, args);
        }

        // Check whether the object names a builtin namespace.
        if let ExprKind::Ident(namespace) = &object.kind {
            if let Some(builtin_id) = resolve_namespace_method(namespace, method) {
                let argc = checked_builtin_argc(args.len())?;
                // Validate argument count at compile time
                let arity = builtin_arity(&builtin_id);
                if !arity.accepts(args.len()) {
                    return Err(CompileError::new(format!(
                        "{}.{}() expects {} argument(s), got {}",
                        namespace,
                        method,
                        arity,
                        args.len()
                    )));
                }
                // Compile arguments onto stack
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.emit(OpCode::CallBuiltin(builtin_id, argc));
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
            "sha256" => {
                if !args.is_empty() {
                    return Err(CompileError::new("sha256() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodySha256);
            }
            "md5" => {
                if !args.is_empty() {
                    return Err(CompileError::new("md5() requires 0 arguments"));
                }
                self.compile_expr(object)?;
                self.emit(OpCode::BodyMd5);
            }
            "hmac_sha256" => {
                if args.len() != 1 {
                    return Err(CompileError::new("hmac_sha256() requires 1 key argument"));
                }
                self.compile_expr(object)?;
                self.compile_expr(&args[0])?;
                self.emit(OpCode::BodyHmacSha256);
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

    fn compile_json_select(&mut self, object: &Expr, args: &[Expr]) -> Result<(), CompileError> {
        if args.len() != 1 {
            return Err(CompileError::new(
                "json_select() requires 1 non-empty map literal argument",
            ));
        }
        let ExprKind::MapLiteral(pairs) = &args[0].kind else {
            return Err(CompileError::new(
                "json_select() argument must be a non-empty map literal",
            ));
        };
        if pairs.is_empty() {
            return Err(CompileError::new("json_select() map must not be empty"));
        }
        if pairs.len() > JSON_SELECT_MAX_ENTRIES {
            return Err(CompileError::new(format!(
                "json_select() map has {} entries (max {})",
                pairs.len(),
                JSON_SELECT_MAX_ENTRIES
            )));
        }

        let mut names = HashSet::with_capacity(pairs.len());
        let mut entries = Vec::with_capacity(pairs.len());
        for (name, path) in pairs {
            let ExprKind::StringLit(name) = &name.kind else {
                return Err(CompileError::new("json_select() output names must be string literals"));
            };
            let ExprKind::StringLit(path) = &path.kind else {
                return Err(CompileError::new("json_select() paths must be string literals"));
            };
            if name.len() > JSON_SELECT_MAX_NAME_BYTES {
                return Err(CompileError::new(format!(
                    "json_select() output name is {} bytes (max {})",
                    name.len(),
                    JSON_SELECT_MAX_NAME_BYTES
                )));
            }
            if !names.insert(name.as_str()) {
                return Err(CompileError::new(format!(
                    "json_select() output name '{}' is duplicated",
                    name
                )));
            }
            crate::json_path::DefiniteJsonPath::parse(path)
                .map_err(|error| CompileError::new(format!("invalid definite JSON path: {}", error)))?;
            entries.push(JsonSelectEntry {
                name: name.clone(),
                path: path.clone(),
            });
        }

        let plan_idx = self.add_json_select_const(entries)?;
        self.compile_expr(object)?;
        self.emit(OpCode::JsonSelect(plan_idx));
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
            let argc = checked_builtin_argc(args.len())?;
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
            self.emit(OpCode::CallBuiltin(BuiltinId::LogFmt, argc));
            return Ok(());
        }

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
            "trim" => BuiltinId::Trim,
            "split" => BuiltinId::Split,
            "range" => BuiltinId::Range,
            // P3 utility functions
            "hmac_sha256" => BuiltinId::HmacSha256,
            "uuid" => BuiltinId::Uuid,
            "hex_encode" => BuiltinId::HexEncode,
            "hex_decode" => BuiltinId::HexDecode,
            "time_format" => BuiltinId::TimeFormat,
            "ip_in_cidr" => BuiltinId::IpInCidr,
            "join" => BuiltinId::Join,
            "log_warn" => BuiltinId::LogWarn,
            "log_error" => BuiltinId::LogError,
            _ => {
                return Err(CompileError::new(format!("unknown function: {}()", name)));
            }
        };

        // Validate argument count at compile time
        let argc = checked_builtin_argc(args.len())?;
        let arity = builtin_arity(&builtin_id);
        if !arity.accepts(args.len()) {
            return Err(CompileError::new(format!(
                "{}() expects {} argument(s), got {}",
                name,
                arity,
                args.len()
            )));
        }

        for arg in args {
            self.compile_expr(arg)?;
        }
        self.emit(OpCode::CallBuiltin(builtin_id, argc));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::bytecode::{Constant, OpCode, JSON_SELECT_MAX_ENTRIES, JSON_SELECT_MAX_NAME_BYTES};
    use super::super::super::parser::parse_program;
    use super::super::Compiler;

    #[test]
    fn builtin_argument_capacity_is_checked_before_emitting_operands() {
        use crate::bytecode::BuiltinId;

        for count in [1, 255, 256, 257] {
            let source = format!("log_fmt(\"{}\"{})", "{}".repeat(count - 1), ", nil".repeat(count - 1));
            let program = parse_program(&source).unwrap();
            let mut compiler = Compiler::new();
            let result = compiler.compile_stmt(&program.stmts[0]);
            if count <= 255 {
                result.unwrap();
                assert_eq!(
                    compiler.code[count],
                    OpCode::CallBuiltin(BuiltinId::LogFmt, count as u8)
                );
                assert_eq!(
                    compiler.line_runs.iter().map(|(n, _)| usize::from(*n)).sum::<usize>(),
                    compiler.code.len()
                );
            } else {
                assert_eq!(
                    result.unwrap_err().message,
                    "builtin call exceeds maximum argument count (255)"
                );
                assert!(compiler.code.is_empty());
                assert!(compiler.constants.is_empty());
            }
        }
        // Both ordinary and namespace call paths check capacity before any
        // argument is compiled (an undefined operand would otherwise fail).
        for function in ["len", "req.header"] {
            let source = format!("{function}({})", vec!["undefined"; 256].join(","));
            let program = parse_program(&source).unwrap();
            let mut compiler = Compiler::new();
            assert_eq!(
                compiler.compile_stmt(&program.stmts[0]).unwrap_err().message,
                "builtin call exceeds maximum argument count (255)"
            );
            assert!(compiler.code.is_empty());
        }
    }

    #[test]
    fn builtin_argument_capacity_preserves_function_arity() {
        for (source, valid) in [
            ("uuid()", true),
            ("uuid(nil)", false),
            ("len(nil)", true),
            ("len(nil, nil)", false),
            ("req.header(nil)", true),
            ("req.header(nil, nil)", false),
            ("http.call(nil, nil, nil, nil, nil)", true),
            ("http.call(nil, nil, nil, nil, nil, nil)", false),
            ("log_fmt()", false),
        ] {
            let program = parse_program(source).unwrap();
            assert_eq!(Compiler::new().compile(&program).is_ok(), valid, "{source}");
        }
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
            .any(|c| matches!(c, Constant::StreamingRegex(_))));
    }

    #[test]
    fn test_compile_enforces_ranged_builtin_arities() {
        for source in [
            r#"let response = http.call("POST", "https://example.com", nil)"#,
            r#"let response = http.call("POST", "https://example.com", nil, 1000, {})"#,
            r#"let result = redis.set("cache", "key", "value")"#,
            r#"let result = redis.set("cache", "key", "value", 1000)"#,
        ] {
            let program = parse_program(source).unwrap();
            Compiler::new().compile(&program).unwrap();
        }

        for (source, expected) in [
            (
                r#"let response = http.call("POST", "https://example.com")"#,
                "http.call() expects 3 to 5 argument(s), got 2",
            ),
            (
                r#"let response = http.call("POST", "https://example.com", nil, 1000, {}, "extra")"#,
                "http.call() expects 3 to 5 argument(s), got 6",
            ),
            (
                r#"let result = redis.set("cache", "key")"#,
                "redis.set() expects 3 to 4 argument(s), got 2",
            ),
            (
                r#"let result = redis.set("cache", "key", "value", 1000, "extra")"#,
                "redis.set() expects 3 to 4 argument(s), got 5",
            ),
        ] {
            let program = parse_program(source).unwrap();
            let error = Compiler::new().compile(&program).unwrap_err().to_string();
            assert!(error.contains(expected), "expected '{expected}', got: {error}");
        }
    }

    #[test]
    fn test_body_read_requires_explicit_range_and_rejects_removed_method() {
        for (source, expected) in [
            ("req.body.read_all()", "unknown method: .read_all()"),
            ("req.body.read()", "read() requires 2 arguments"),
            ("req.body.read(16)", "read() requires 2 arguments"),
        ] {
            let program = parse_program(source).unwrap();
            let error = Compiler::new().compile(&program).unwrap_err().to_string();
            assert!(error.contains(expected), "expected '{expected}', got: {error}");
        }
    }

    #[test]
    fn test_compile_body_read_and_mutation_methods() {
        let source = r#"
            let chars = req.body.len()
            let empty = req.body.is_empty()
            let bytes = req.body.byte_len()
            let prefix = req.body.read(0, 16)
            let whole = req.body.read(0, req.body.byte_len())
            let digest = req.body.sha256()
            let legacy = req.body.md5()
            let mac = req.body.hmac_sha256("key")
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
            OpCode::BodySha256,
            OpCode::BodyMd5,
            OpCode::BodyHmacSha256,
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
    fn test_compile_body_digest_method_arities() {
        for (source, expected) in [
            ("req.body.md5(1)", "md5() requires 0 arguments"),
            ("req.body.hmac_sha256()", "hmac_sha256() requires 1 key argument"),
            (
                r#"req.body.hmac_sha256("a", "b")"#,
                "hmac_sha256() requires 1 key argument",
            ),
        ] {
            let program = parse_program(source).unwrap();
            let error = Compiler::new().compile(&program).unwrap_err().to_string();
            assert!(error.contains(expected), "expected '{expected}', got: {error}");
        }
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
    fn test_json_select_compiles_one_self_contained_plan_without_makemap() {
        let source = r#"
            let selected = req.body.json_select({
                "id": "$.user.id",
                "name": "$.user.name",
                "id_copy": "$.user.id"
            })
        "#;
        let program = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&program).unwrap();
        let (plan_index, entries) = compiled
            .constants
            .iter()
            .enumerate()
            .find_map(|(index, constant)| match constant {
                Constant::JsonSelect(entries) => Some((index as u16, entries)),
                _ => None,
            })
            .expect("JSON select plan constant");

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "id");
        assert_eq!(entries[0].path, "$.user.id");
        assert_eq!(entries[2].name, "id_copy");
        assert_eq!(entries[2].path, "$.user.id");
        assert!(compiled.code.contains(&OpCode::JsonSelect(plan_index)));
        assert!(!compiled.code.iter().any(|op| matches!(op, OpCode::MakeMap(_))));
    }

    #[test]
    fn test_json_select_rejects_non_literal_or_invalid_plans() {
        for (source, expected) in [
            (r#"let selected = req.body.json_select({})"#, "map must not be empty"),
            (
                r#"let plan = {"id": "$.id"}
                   let selected = req.body.json_select(plan)"#,
                "argument must be a non-empty map literal",
            ),
            (
                r#"let path = "$.id"
                   let selected = req.body.json_select({"id": path})"#,
                "paths must be string literals",
            ),
            (
                r#"let selected = req.body.json_select({"id": "$.*"})"#,
                "invalid definite JSON path",
            ),
            (
                r#"let selected = req.body.json_select({"id": "$.a", "id": "$.b"})"#,
                "output name 'id' is duplicated",
            ),
            (
                r#"let selected = req.body.json_select({"id": "$.id"}, {"name": "$.name"})"#,
                "requires 1 non-empty map literal argument",
            ),
        ] {
            let program = parse_program(source).unwrap();
            let error = Compiler::new().compile(&program).unwrap_err().to_string();
            assert!(error.contains(expected), "expected '{expected}', got: {error}");
        }
    }

    #[test]
    fn test_json_select_accepts_body_value_in_a_local() {
        let source = r#"
            let body = req.body
            let selected = body.json_select({"id": "$.id"})
        "#;
        let program = parse_program(source).unwrap();
        let compiled = Compiler::new().compile(&program).unwrap();
        assert!(compiled.code.iter().any(|op| matches!(op, OpCode::JsonSelect(_))));
    }

    #[test]
    fn test_json_select_enforces_static_entry_and_name_limits() {
        let entries = (0..=JSON_SELECT_MAX_ENTRIES)
            .map(|index| format!(r#""k{index}": "$.v""#))
            .collect::<Vec<_>>()
            .join(",");
        let source = format!("let selected = req.body.json_select({{{entries}}})");
        let program = parse_program(&source).unwrap();
        let error = Compiler::new().compile(&program).unwrap_err().to_string();
        assert!(error.contains("65 entries (max 64)"), "got: {error}");

        let long_name = "n".repeat(JSON_SELECT_MAX_NAME_BYTES + 1);
        let source = format!(r#"let selected = req.body.json_select({{"{long_name}": "$.v"}})"#);
        let program = parse_program(&source).unwrap();
        let error = Compiler::new().compile(&program).unwrap_err().to_string();
        assert!(error.contains("257 bytes (max 256)"), "got: {error}");
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
