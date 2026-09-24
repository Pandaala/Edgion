use super::*;

#[test]
fn test_validate_simple_script() {
    let source = r#"let x = 42"#;
    let result = compile_dsl_source(source, &ValidationLimits::default());
    assert!(result.is_ok());
}

#[test]
fn test_validate_rejects_too_many_instructions() {
    let limits = ValidationLimits {
        max_instructions: 3,
        ..Default::default()
    };
    let source = r#"
            let a = 1
            let b = 2
            let c = 3
            let d = 4
        "#;
    let result = compile_dsl_source(source, &limits);
    assert!(result.is_err());
}

#[test]
fn test_serialize_deserialize_roundtrip() {
    let source = r#"
            let x = req.header("X-Test")
            if x == nil {
                return deny(403, "blocked")
            }
        "#;
    let bytecode_b64 = compile_dsl_source(source, &ValidationLimits::default()).unwrap();
    let script = CompiledScript::deserialize_base64(&bytecode_b64).unwrap();
    assert!(!script.code.is_empty());
    assert_eq!(script.local_count, 1);
}

#[test]
fn test_compile_with_source_embeds_original_text() {
    let source = r#"let x = 42"#;
    let program = parse_program(source).unwrap();
    let script = Compiler::new().compile_with_source(&program, source).unwrap();
    assert_eq!(script.source.as_deref(), Some(source));
}

#[test]
fn test_compile_without_source_leaves_field_none() {
    // Compiler::compile must not embed source text; source must be None.
    let program = parse_program("let x = 42").unwrap();
    let script = Compiler::new().compile(&program).unwrap();
    assert!(script.source.is_none());
    // Verify `#[serde(skip_serializing_if = "Option::is_none")]` keeps the
    // field out of the JSON payload when source is absent.
    let json = serde_json::to_string(&script).unwrap();
    assert!(
        !json.contains("\"source\""),
        "expected `source` to be omitted from JSON when None, got: {}",
        json
    );
}

#[test]
fn test_validate_rejects_dishonest_max_loop_depth() {
    // Hand-crafted bytecode declares max_loop_depth=0 but actually nests LoopInit pairs
    // beyond the configured limit. Mimics an operator-supplied `bytecode:` payload.
    let limits = ValidationLimits {
        max_loop_depth: 2,
        ..Default::default()
    };
    let mut code = Vec::new();
    for _ in 0..5 {
        code.push(OpCode::LoopInit);
    }
    for _ in 0..5 {
        code.push(OpCode::LoopEnd);
    }
    code.push(OpCode::ReturnNext);
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code,
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0, // dishonest — real depth is 5
        source: None,
        source_map: None,
    };
    let errors = Validator::new(limits).validate(&script);
    let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
    assert!(
        joined.contains("loop nesting too deep"),
        "expected observed-depth limit error, got: {}",
        joined
    );
    assert!(
        joined.contains("declared max_loop_depth"),
        "expected declared/observed mismatch error, got: {}",
        joined
    );
}

#[test]
fn test_validate_accepts_honest_loop_depth() {
    let limits = ValidationLimits::default();
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![
            OpCode::LoopInit,
            OpCode::LoopInit,
            OpCode::LoopEnd,
            OpCode::LoopEnd,
            OpCode::ReturnNext,
        ],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 2,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(limits).validate(&script);
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
}

#[test]
fn test_validate_enforces_ranged_builtin_arities() {
    for (builtin, argc) in [
        (BuiltinId::HttpCall, 2),
        (BuiltinId::HttpCall, 6),
        (BuiltinId::RedisSet, 2),
        (BuiltinId::RedisSet, 5),
    ] {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::CallBuiltin(builtin, argc), OpCode::ReturnNext],
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let joined = Validator::new(ValidationLimits::default())
            .validate(&script)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            joined.contains("expects") && joined.contains(&format!("bytecode says {argc}")),
            "expected argument-count error for {builtin:?}/{argc}, got: {joined}"
        );
    }

    for (builtin, argc) in [
        (BuiltinId::HttpCall, 3),
        (BuiltinId::HttpCall, 5),
        (BuiltinId::RedisSet, 3),
        (BuiltinId::RedisSet, 4),
    ] {
        let mut code = vec![OpCode::LoadNil; usize::from(argc)];
        code.extend([OpCode::CallBuiltin(builtin, argc), OpCode::Pop, OpCode::ReturnNext]);
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code,
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        assert!(
            errors.is_empty(),
            "valid boundary rejected for {builtin:?}/{argc}: {errors:?}"
        );
    }
}

#[test]
fn test_validate_rejects_loadconst_regex() {
    // Hand-crafted bytecode: LoadConst(0) where constants[0] is a Regex.
    // The validator must reject this because Regex constants cannot be used as values.
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
        constants: vec![Constant::Regex("\\d+".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
    assert!(
        joined.contains("(Regex) which cannot be used as a value"),
        "expected LoadConst-Regex error, got: {}",
        joined
    );
}

#[test]
fn test_validate_rejects_regex_opcode_oob_index() {
    // All three regex opcodes must reject an out-of-bounds constant index.
    for code in [
        vec![OpCode::Matches(5), OpCode::ReturnNext],
        vec![OpCode::RegexFind(5), OpCode::ReturnNext],
        vec![OpCode::RegexReplace(5), OpCode::ReturnNext],
    ] {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code,
            constants: vec![Constant::Regex("\\d+".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("but pool size is"),
            "expected out-of-bounds error, got: {}",
            joined
        );
    }
}

#[test]
fn test_validate_rejects_regex_opcode_non_regex_constant() {
    // All three regex opcodes must reject a valid index that points to a non-Regex constant.
    for code in [
        vec![OpCode::Matches(0), OpCode::ReturnNext],
        vec![OpCode::RegexFind(0), OpCode::ReturnNext],
        vec![OpCode::RegexReplace(0), OpCode::ReturnNext],
    ] {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code,
            constants: vec![Constant::Str("not-a-regex".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("which is not a"),
            "expected wrong constant-kind error, got: {}",
            joined
        );
    }
}

#[test]
fn test_validate_requires_streaming_regex_for_matches() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::Matches(0), OpCode::ReturnNext],
        constants: vec![Constant::Regex("word".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let joined = Validator::new(ValidationLimits::default())
        .validate(&script)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("which is not a StreamingRegex"));
}

#[test]
fn test_validate_rejects_unicode_word_boundary_in_injected_streaming_regex() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::Matches(0), OpCode::ReturnNext],
        constants: vec![Constant::StreamingRegex(r"\bword\b".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let joined = Validator::new(ValidationLimits::default())
        .validate(&script)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("invalid streaming regex constant"));
}

#[test]
fn test_validate_rejects_invalid_or_mistyped_json_path_constant() {
    for constants in [
        vec![Constant::JsonPath("$.*".to_string())],
        vec![Constant::Str("$.ok".to_string())],
    ] {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::JsonGet(0), OpCode::ReturnNext],
            constants,
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        assert!(!errors.is_empty());
    }
}

#[test]
fn test_validate_rejects_old_bytecode_version_for_direct_callers() {
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::ReturnNext],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let joined = Validator::new(ValidationLimits::default())
        .validate(&script)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("unsupported bytecode version"));
}

#[test]
fn test_validate_rejects_loadconst_regex_non_zero_index() {
    // Regex at index 1 in a mixed pool — verifies the type-check branch fires
    // independently of the bounds-check branch (which only triggers when idx >= pool_size).
    // Guards against an accidental idx == 0 short-circuit regression.
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::LoadConst(1), OpCode::ReturnNext],
        constants: vec![Constant::Str("normal".to_string()), Constant::Regex("\\w+".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
    assert!(
        joined.contains("references constant 1 (Regex) which cannot be used as a value"),
        "expected LoadConst-Regex error at non-zero index, got: {}",
        joined
    );
}

#[test]
fn test_validate_rejects_oversized_makemap() {
    // Hand-crafted bytecode: MakeMap(n) where n exceeds max_map_size.
    // The validator must catch injected oversized map operands before they reach the runtime.
    let limits = ValidationLimits {
        max_map_size: 4,
        ..Default::default()
    };
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::MakeMap(5), OpCode::ReturnNext],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(limits).validate(&script);
    let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
    assert!(
        joined.contains("MakeMap at instruction 0 requires 5 entries but limit is 4"),
        "expected MakeMap limit error, got: {}",
        joined
    );
}

fn json_select_script(entries: Vec<JsonSelectEntry>) -> CompiledScript {
    CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![
            OpCode::CallBuiltin(BuiltinId::ReqBody, 0),
            OpCode::JsonSelect(0),
            OpCode::Pop,
            OpCode::ReturnNext,
        ],
        constants: vec![Constant::JsonSelect(entries)],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    }
}

fn json_select_entry(name: impl Into<String>, path: impl Into<String>) -> JsonSelectEntry {
    JsonSelectEntry {
        name: name.into(),
        path: path.into(),
    }
}

#[test]
fn test_validate_json_select_accepts_duplicate_paths() {
    let script = json_select_script(vec![
        json_select_entry("first", "$.user.id"),
        json_select_entry("second", "$.user.id"),
    ]);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(errors.is_empty(), "duplicate paths must be valid: {errors:?}");
}

#[test]
fn test_validate_rejects_json_select_opcode_reference_errors() {
    for script in [
        CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::JsonSelect(1), OpCode::ReturnNext],
            constants: vec![Constant::JsonSelect(vec![json_select_entry("id", "$.id")])],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        },
        CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::JsonSelect(0), OpCode::ReturnNext],
            constants: vec![Constant::JsonPath("$.id".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        },
    ] {
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        assert!(!errors.is_empty(), "hostile opcode reference must fail");
    }
}

#[test]
fn test_validate_rejects_json_select_entry_count_limits() {
    let empty = json_select_script(vec![]);
    let errors = Validator::new(ValidationLimits::default()).validate(&empty);
    assert!(errors.iter().any(|error| error.to_string().contains("allowed 1..=64")));

    let too_many = json_select_script(
        (0..65)
            .map(|index| json_select_entry(format!("key{index}"), "$.value"))
            .collect(),
    );
    let errors = Validator::new(ValidationLimits::default()).validate(&too_many);
    assert!(errors.iter().any(|error| error.to_string().contains("has 65 entries")));

    let smaller_map_limit = json_select_script(vec![
        json_select_entry("first", "$.a"),
        json_select_entry("second", "$.b"),
    ]);
    let errors = Validator::new(ValidationLimits {
        max_map_size: 1,
        ..Default::default()
    })
    .validate(&smaller_map_limit);
    assert!(errors.iter().any(|error| error.to_string().contains("allowed 1..=1")));
}

#[test]
fn test_validate_rejects_json_select_name_path_and_pool_violations() {
    let invalid = json_select_script(vec![
        json_select_entry("duplicate", "$.ok"),
        json_select_entry("duplicate", "$.*"),
        json_select_entry("n".repeat(JSON_SELECT_MAX_NAME_BYTES + 1), "$.ok"),
    ]);
    let joined = Validator::new(ValidationLimits::default())
        .validate(&invalid)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("duplicate output name 'duplicate'"), "got: {joined}");
    assert!(joined.contains("invalid JSON select path"), "got: {joined}");
    assert!(joined.contains("name is too long: 257 bytes"), "got: {joined}");

    let script = json_select_script(vec![json_select_entry("name", "$.path")]);
    let joined = Validator::new(ValidationLimits {
        max_string_const_len: 4,
        max_const_pool_bytes: 5,
        ..Default::default()
    })
    .validate(&script)
    .iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join("|");
    assert!(joined.contains("path exceeds string constant limit"), "got: {joined}");
    assert!(joined.contains("constant pool too large"), "got: {joined}");
}

#[test]
fn test_validate_rejects_loading_json_select_as_value() {
    let script = CompiledScript {
        code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
        ..json_select_script(vec![json_select_entry("id", "$.id")])
    };
    let joined = Validator::new(ValidationLimits::default())
        .validate(&script)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
    assert!(
        joined.contains("(JsonSelect) which cannot be used as a value"),
        "got: {joined}"
    );
}

#[test]
fn test_full_pipeline_enforces_json_select_max_map_size() {
    let limits = ValidationLimits {
        max_map_size: 1,
        ..Default::default()
    };
    let result = compile_dsl_source(
        r#"let selected = req.body.json_select({"first": "$.a", "second": "$.b"})"#,
        &limits,
    );
    let joined = result.expect_err("configured maxMapSize must reject plan").join("|");
    assert!(joined.contains("allowed 1..=1"), "got: {joined}");
}

#[test]
fn test_full_pipeline() {
    let source = r#"
            let ua = req.header("User-Agent")
            if ua == nil {
                return deny(403, "Missing User-Agent")
            }
            if ua.contains("bot") {
                return deny(403, "Bot blocked")
            }
            req.set_header("X-Processed", "true")
        "#;
    let bytecode = compile_dsl_source(source, &ValidationLimits::default());
    assert!(bytecode.is_ok());
}
