use super::*;

fn compile(source: &str) -> CompiledScript {
    resolve_script(Some(source), None, &ValidationLimits::default()).expect("source must compile")
}

#[test]
fn resolve_script_rejects_uncompilable_source() {
    let err = resolve_script(Some("this is not valid dsl @@@"), None, &ValidationLimits::default())
        .expect_err("uncompilable source must fail");
    assert!(err.contains("DSL compile errors"), "got: {err}");
}

#[test]
fn legacy_client_ip_builtin_is_rejected() {
    let err = resolve_script(Some("let ip = req.client_ip"), None, &ValidationLimits::default())
        .expect_err("the legacy client_ip builtin must not remain as an alias");
    assert!(err.contains("DSL compile errors"), "got: {err}");

    compile("let direct = req.direct_peer_ip\nlet remote = req.remote_ip");
}

#[test]
fn resolve_script_requires_source_or_bytecode() {
    let err = resolve_script(None, None, &ValidationLimits::default()).expect_err("empty input must fail");
    assert_eq!(err, "either source or bytecode must be provided");
}

#[test]
fn resolve_script_bytecode_takes_priority_and_roundtrips() {
    let script = compile("let a = 1");
    let b64 = script.serialize_base64().expect("serialize");
    // bytecode wins even when source is also present (and uncompilable)
    let resolved = resolve_script(Some("not valid @@@"), Some(&b64), &ValidationLimits::default())
        .expect("bytecode path must succeed");
    assert_eq!(resolved.code.len(), script.code.len());
}

#[test]
fn resolve_script_rejects_unstructured_bytecode_jumps() {
    let cases = [
        (
            "ordinary back edge",
            vec![OpCode::LoopInit, OpCode::Jump(-1), OpCode::LoopEnd],
        ),
        (
            "conditional back edge",
            vec![
                OpCode::LoopInit,
                OpCode::LoadTrue,
                OpCode::JumpIfFalse(-2),
                OpCode::LoopEnd,
            ],
        ),
        (
            "enter loop",
            vec![OpCode::Jump(1), OpCode::LoopInit, OpCode::LoadTrue, OpCode::LoopEnd],
        ),
        ("skip cleanup", vec![OpCode::LoopInit, OpCode::Jump(1), OpCode::LoopEnd]),
        (
            "cross sibling loops",
            vec![
                OpCode::LoopInit,
                OpCode::Jump(2),
                OpCode::LoopEnd,
                OpCode::LoopInit,
                OpCode::LoadTrue,
                OpCode::LoopEnd,
            ],
        ),
        (
            "forward LoopBack",
            vec![OpCode::LoopInit, OpCode::LoopBack(0), OpCode::LoopEnd],
        ),
        (
            "cross-loop LoopBack",
            vec![
                OpCode::LoopInit,
                OpCode::LoadTrue,
                OpCode::LoopEnd,
                OpCode::LoopInit,
                OpCode::LoopBack(-3),
                OpCode::LoopEnd,
            ],
        ),
    ];
    for (name, mut code) in cases {
        code.push(OpCode::ReturnNext);
        let script = CompiledScript {
            version: crate::bytecode::BYTECODE_VERSION,
            code,
            constants: vec![],
            local_count: 0,
            max_loop_depth: 1,
            source: None,
            source_map: None,
        };
        let encoded = script.serialize_base64().unwrap();
        let error = resolve_script(None, Some(&encoded), &ValidationLimits::default()).expect_err(name);
        assert!(error.contains("bytecode validation failed"), "{name}: {error}");
    }
}

#[test]
fn resolve_script_accepts_compiler_control_flow() {
    let source = r#"
            let mut n = 0
            while n < 4 {
                n = n + 1
                if n == 2 { continue }
                for i in range(0, 3) {
                    if i == 1 { break }
                }
            }
            for item in split("a,b", ",") {
                if item == "a" { continue }
            }
            let result = (true && false) || true
        "#;
    let script = resolve_script(Some(source), None, &ValidationLimits::default()).unwrap();
    let encoded = script.serialize_base64().unwrap();
    resolve_script(None, Some(&encoded), &ValidationLimits::default()).unwrap();
}

#[test]
fn resolve_script_rejects_deep_serialized_loop_nesting() {
    let mut code = vec![OpCode::LoopInit; 5_000];
    code.extend(std::iter::repeat_n(OpCode::LoopEnd, 5_000));
    let script = CompiledScript {
        version: crate::bytecode::BYTECODE_VERSION,
        code,
        constants: vec![],
        local_count: 0,
        max_loop_depth: 5_000,
        source: None,
        source_map: None,
    };
    let encoded = script.serialize_base64().unwrap();
    let error = resolve_script(None, Some(&encoded), &ValidationLimits::default()).unwrap_err();
    assert!(error.contains("too many instructions"), "{error}");
    assert!(error.contains("loop nesting too deep"), "{error}");
}

#[test]
fn check_rejects_unknown_redis_link() {
    let script = compile("let r = redis.get(\"mylink\", \"mykey\")");
    let err = check_redis_link_names(&script, &HashSet::new())
        .expect_err("redis call referencing an undeclared link must fail");
    assert!(err.contains("linkSysRefs"), "got: {err}");
}

#[test]
fn redis_rate_limit_source_bytecode_and_alias_contract() {
    let mut script = compile("let r = redis.rate_limit(\"cache\", \"requests\", \"key\", 1)");
    assert!(script.references_builtin(crate::bytecode::BuiltinId::RedisRateLimit));
    assert!(check_redis_link_names(&script, &HashSet::new()).is_err());
    assert!(check_redis_link_names(&script, &HashSet::from(["cache"])).is_ok());
    let encoded = script.serialize_base64().unwrap();
    let decoded = resolve_script(None, Some(&encoded), &ValidationLimits::default()).unwrap();
    assert!(decoded.references_builtin(crate::bytecode::BuiltinId::RedisRateLimit));
    for argc in [3, 5] {
        for op in &mut script.code {
            if let OpCode::CallBuiltin(crate::bytecode::BuiltinId::RedisRateLimit, count) = op {
                *count = argc;
            }
        }
        let encoded = script.serialize_base64().unwrap();
        assert!(resolve_script(None, Some(&encoded), &ValidationLimits::default()).is_err());
    }
    for source in [
        "redis.rate_limit(\"cache\", \"requests\", \"key\")",
        "redis.rate_limit(\"cache\", \"requests\", \"key\", 1, 2)",
    ] {
        assert!(resolve_script(Some(source), None, &ValidationLimits::default()).is_err());
    }
    let dynamic = compile("redis.rate_limit(\"unknown\", \"requests\", \"key:\" + req.remote_ip, 1)");
    assert!(check_redis_link_names(&dynamic, &HashSet::new()).is_ok());
}

#[test]
fn check_accepts_declared_link_with_multi_instruction_sibling_arg() {
    // A non-literal (multi-instruction) argument after the link name makes
    // the i-argc positional guess unreliable; the link-name check must be
    // skipped (not rejected). The link "cache" is correctly declared.
    let script = compile("let r = redis.get(\"cache\", req.header(\"X-Key\"))");
    let names: HashSet<&str> = ["cache"].into_iter().collect();
    assert!(check_redis_link_names(&script, &names).is_ok());
}

#[test]
fn check_fails_open_when_layout_is_ambiguous() {
    // Documented fail-open behavior: when a sibling argument is a
    // multi-instruction sub-expression, the positional guess is unreliable,
    // so the link-name check is skipped rather than risk a false rejection.
    // "typo" is undeclared, but the config still passes static validation
    // (the undeclared link surfaces at runtime via ok=false, not as a 500).
    let script = compile("let r = redis.get(\"typo\", req.header(\"X-Key\"))");
    assert!(check_redis_link_names(&script, &HashSet::new()).is_ok());
}
