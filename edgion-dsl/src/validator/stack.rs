//! Operand-stack heights and literal-template provenance at reachable control-flow joins.

use crate::bytecode::{BuiltinId, Constant, OpCode};
use crate::error::ValidationError;

/// Called after instruction-count, operand, and jump-target validation.
/// A stack slot is marked only when its value comes directly from a string constant.
/// Join states weaken monotonically, so loops converge within the bounded code size.
pub(super) fn validate(code: &[OpCode], constants: &[Constant]) -> Result<(), ValidationError> {
    let mut states = vec![None; code.len() + 1];
    states[0] = Some(Vec::<bool>::new());
    let mut pending = vec![0];
    while let Some(pc) = pending.pop() {
        let mut stack = states[pc].as_ref().expect("scheduled instruction has a state").clone();
        if pc == code.len() {
            if !stack.is_empty() {
                return Err(ValidationError::new("operand stack is not empty at end of bytecode"));
            }
            continue;
        }
        let op = code[pc];
        let (pops, pushes) = stack_effect(op);
        if stack.len() < pops {
            return Err(ValidationError::new(format!(
                "operand stack underflow at instruction {pc}: requires {pops}, has {}",
                stack.len()
            )));
        }
        if let OpCode::CallBuiltin(BuiltinId::LogFmt, argc) = op {
            if !stack[stack.len() - usize::from(argc)] {
                return Err(ValidationError::new(format!(
                    "log_fmt template at instruction {pc} must come directly from a string constant"
                )));
            }
        }
        stack.truncate(stack.len() - pops);
        stack.extend(std::iter::repeat_n(false, pushes));
        if let OpCode::LoadConst(index) = op {
            *stack.last_mut().expect("LoadConst pushes a value") =
                matches!(constants.get(usize::from(index)), Some(Constant::Str(_)));
        }
        if matches!(
            op,
            OpCode::ReturnNext | OpCode::ReturnTerminate | OpCode::ReturnDeny | OpCode::ReturnGrpcError
        ) {
            if !stack.is_empty() {
                return Err(ValidationError::new(format!(
                    "operand stack is not empty at return instruction {pc}"
                )));
            }
            continue;
        }

        let mut successors = [None, None];
        match op {
            OpCode::Jump(offset) | OpCode::LoopBack(offset) => {
                successors[0] = Some((pc as i64 + 1 + i64::from(offset)) as usize);
            }
            OpCode::JumpIfFalse(offset) => {
                successors[0] = Some((pc as i64 + 1 + i64::from(offset)) as usize);
                successors[1] = Some(pc + 1);
            }
            _ => successors[0] = Some(pc + 1),
        }
        for target in successors.into_iter().flatten() {
            match &mut states[target] {
                Some(previous) if previous.len() != stack.len() => {
                    return Err(ValidationError::new(format!(
                        "inconsistent operand stack height at instruction {target}: {} versus {}",
                        previous.len(),
                        stack.len()
                    )));
                }
                Some(previous) => {
                    let mut changed = false;
                    for (slot, incoming) in previous.iter_mut().zip(&stack) {
                        if *slot && !incoming {
                            *slot = false;
                            changed = true;
                        }
                    }
                    if changed {
                        pending.push(target);
                    }
                }
                None => {
                    states[target] = Some(stack.clone());
                    pending.push(target);
                }
            }
        }
    }
    Ok(())
}

fn stack_effect(op: OpCode) -> (usize, usize) {
    use OpCode::*;
    match op {
        LoadConst(_) | LoadNil | LoadTrue | LoadFalse | GetLocal(_) => (0, 1),
        Pop | SetLocal(_) | JumpIfFalse(_) => (1, 0),
        Add | Sub | Mul | Div | Equal | NotEqual | Less | Greater | LessEqual | GreaterEqual | StartsWith
        | EndsWith | Contains | RegexReplace(_) | BodyHmacSha256 | BodyAppend | BodyPrepend | IndexGet => (2, 1),
        Neg | Not | Matches(_) | RegexFind(_) | TextLen | TextIsEmpty | BodyByteLen | BodySha256 | BodyMd5
        | JsonGet(_) | JsonSelect(_) | BodyClear | ListLen => (1, 1),
        BodyRead | BodyRemove | BodyReplace => (3, 1),
        MakeMap(count) => (usize::from(count) * 2, 1),
        CallBuiltin(_, argc) => (usize::from(argc), 1),
        ReturnDeny | ReturnGrpcError => (2, 0),
        Jump(_) | LoopBack(_) | LoopInit | LoopEnd | ReturnNext | ReturnTerminate => (0, 0),
    }
}

#[cfg(test)]
mod tests {
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
}
