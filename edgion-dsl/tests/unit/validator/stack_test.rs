use crate::bytecode::{BuiltinId, CompiledScript, Constant, OpCode, BYTECODE_VERSION};
use crate::validator::{compile_dsl_source, ValidationLimits, Validator};

fn errors(code: Vec<OpCode>, max_loop_depth: u16) -> Vec<String> {
    errors_with_constants(code, vec![], 0, max_loop_depth)
}

fn errors_with_constants(
    code: Vec<OpCode>,
    constants: Vec<Constant>,
    local_count: u16,
    max_loop_depth: u16,
) -> Vec<String> {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code,
        constants,
        local_count,
        max_loop_depth,
        source: None,
        source_map: None,
    };
    Validator::new(ValidationLimits::default())
        .validate(&script)
        .into_iter()
        .map(|e| e.message)
        .collect()
}

#[test]
fn builtin_argument_capacity_rejects_truncated_bytecode() {
    use OpCode::*;
    for (count, argc, expected) in [(256, 0, "expects 1 to 255"), (257, 1, "not empty")] {
        let mut code = vec![LoadNil; count];
        if argc == 1 {
            code[count - 1] = LoadConst(0);
        }
        code.extend([CallBuiltin(BuiltinId::LogFmt, argc), Pop, ReturnNext]);
        let errors = errors_with_constants(code, vec![Constant::Str(String::new())], 0, 0);
        assert!(errors.iter().any(|e| e.contains(expected)), "{errors:?}");
    }
    for argc in [1, 255] {
        let mut code = vec![LoadConst(0)];
        code.extend(vec![LoadNil; argc as usize - 1]);
        code.extend([CallBuiltin(BuiltinId::LogFmt, argc), Pop, ReturnNext]);
        assert!(errors_with_constants(code, vec![Constant::Str(String::new())], 0, 0).is_empty());
    }
}

#[test]
fn log_fmt_requires_literal_template_provenance() {
    use OpCode::*;
    let constants = vec![Constant::Str("X-Token".into()), Constant::Int(42)];
    let cases = [
        (
            vec![
                LoadConst(0),
                CallBuiltin(BuiltinId::ReqHeader, 1),
                CallBuiltin(BuiltinId::LogFmt, 1),
                Pop,
                ReturnNext,
            ],
            0,
        ),
        (
            vec![
                LoadConst(0),
                SetLocal(0),
                GetLocal(0),
                CallBuiltin(BuiltinId::LogFmt, 1),
                Pop,
                ReturnNext,
            ],
            1,
        ),
        (
            vec![
                LoadConst(0),
                LoadConst(0),
                Add,
                CallBuiltin(BuiltinId::LogFmt, 1),
                Pop,
                ReturnNext,
            ],
            0,
        ),
        (
            vec![LoadConst(1), CallBuiltin(BuiltinId::LogFmt, 1), Pop, ReturnNext],
            0,
        ),
    ];
    for (code, local_count) in cases {
        let errors = errors_with_constants(code, constants.clone(), local_count, 0);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must come directly from a string constant")),
            "{errors:?}"
        );
    }

    let valid = vec![
        LoadConst(0),
        LoadConst(0),
        CallBuiltin(BuiltinId::ReqHeader, 1),
        CallBuiltin(BuiltinId::LogFmt, 2),
        Pop,
        ReturnNext,
    ];
    assert!(errors_with_constants(valid, constants, 0, 0).is_empty());
}

#[test]
fn log_fmt_rechecks_template_when_later_branch_weakens_join() {
    use OpCode::*;
    let constants = vec![Constant::Str("X-Token".into()), Constant::Str("other".into())];
    let code = vec![
        LoadTrue,
        JumpIfFalse(2),
        LoadConst(0),
        Jump(2),
        LoadConst(0),
        CallBuiltin(BuiltinId::ReqHeader, 1),
        CallBuiltin(BuiltinId::LogFmt, 1),
        Pop,
        ReturnNext,
    ];
    let errors = errors_with_constants(code, constants.clone(), 0, 0);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("must come directly from a string constant")),
        "{errors:?}"
    );

    let code = vec![
        LoadTrue,
        JumpIfFalse(2),
        LoadConst(0),
        Jump(1),
        LoadConst(1),
        CallBuiltin(BuiltinId::LogFmt, 1),
        Pop,
        ReturnNext,
    ];
    assert!(errors_with_constants(code, constants, 0, 0).is_empty());
}

#[test]
fn log_fmt_rechecks_template_when_loop_back_weakens_entry() {
    use OpCode::*;
    let code = vec![
        LoadConst(0),
        LoopInit,
        LoadTrue,
        JumpIfFalse(2),
        Jump(5),
        LoadNil,
        Pop,
        LoadConst(0),
        CallBuiltin(BuiltinId::ReqHeader, 1),
        LoopBack(-8),
        LoopEnd,
        CallBuiltin(BuiltinId::LogFmt, 1),
        Pop,
        ReturnNext,
    ];
    let errors = errors_with_constants(code, vec![Constant::Str("X-Token".into())], 0, 1);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("must come directly from a string constant")),
        "{errors:?}"
    );
}

#[test]
fn rejects_inconsistent_operand_flow_before_execution() {
    use OpCode::*;
    for (code, loops, expected) in [
        (
            vec![LoadNil, CallBuiltin(BuiltinId::LogFmt, 2), Pop, ReturnNext],
            0,
            "underflow",
        ),
        (
            vec![LoadTrue, JumpIfFalse(2), LoadNil, Jump(0), ReturnNext],
            0,
            "inconsistent",
        ),
        (
            vec![LoopInit, LoadNil, LoopBack(-2), LoopEnd, ReturnNext],
            1,
            "inconsistent",
        ),
        (vec![LoadNil], 0, "not empty"),
        (vec![LoadNil, ReturnNext], 0, "not empty"),
        (vec![LoadNil, ReturnTerminate], 0, "not empty"),
        (vec![LoadNil, ReturnDeny], 0, "underflow"),
        (vec![LoadNil, LoadNil, LoadNil, ReturnGrpcError], 0, "not empty"),
    ] {
        let errors = errors(code, loops);
        assert!(errors.iter().any(|e| e.contains(expected)), "{errors:?}");
    }
}

#[test]
fn accepts_compiler_control_flow_and_nested_operands() {
    for source in [
        r#"log_fmt("{} {}", to_str(len("x")), true && (false || true))"#,
        r#"for i in range(0, 3) { if i == 1 { continue } if i == 2 { break } log(to_str(i)) }"#,
        r#"for v in split("a,b", ",") { if v == "a" { continue } if v == "b" { break } }"#,
        r#"let mut i = 0 while i < 3 { i = i + 1 if i == 1 { continue } if i == 2 { break } }"#,
        r#"for i in range(0, 3) { return deny(403, "blocked") }"#,
        r#"for v in split("a,b", ",") { return grpc_error(7, "blocked") }"#,
        "while true { return terminate() }",
        "if true { return next() }",
        "return next() log(\"unreachable\")",
        r#"let m = {"key": "value"} log(m["key"])"#,
    ] {
        assert!(
            compile_dsl_source(source, &ValidationLimits::default()).is_ok(),
            "{source}"
        );
    }
    use OpCode::*;
    for code in [
        vec![],
        vec![LoadNil, Pop],
        vec![LoadNil, LoadNil, ReturnDeny],
        vec![LoadNil, LoadNil, ReturnGrpcError],
        vec![ReturnNext, Pop],
    ] {
        assert!(errors(code, 0).is_empty());
    }
}
