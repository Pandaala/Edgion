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
