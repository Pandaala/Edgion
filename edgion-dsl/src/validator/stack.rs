//! Operand-stack heights at reachable control-flow joins and exits.

use crate::bytecode::OpCode;
use crate::error::ValidationError;

/// Called after instruction-count, operand, and jump-target validation.
/// Each PC is scheduled once, bounding time and memory by the code length.
pub(super) fn validate(code: &[OpCode]) -> Result<(), ValidationError> {
    let mut heights = vec![None; code.len() + 1];
    heights[0] = Some(0usize);
    let mut pending = vec![(0, 0usize)];
    while let Some((pc, height)) = pending.pop() {
        if pc == code.len() {
            if height != 0 {
                return Err(ValidationError::new("operand stack is not empty at end of bytecode"));
            }
            continue;
        }
        let op = code[pc];
        let (pops, pushes) = stack_effect(op);
        if height < pops {
            return Err(ValidationError::new(format!(
                "operand stack underflow at instruction {pc}: requires {pops}, has {height}"
            )));
        }
        let next_height = height - pops + pushes;
        if matches!(
            op,
            OpCode::ReturnNext | OpCode::ReturnTerminate | OpCode::ReturnDeny | OpCode::ReturnGrpcError
        ) {
            if next_height != 0 {
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
            match heights[target] {
                Some(previous) if previous != next_height => {
                    return Err(ValidationError::new(format!(
                        "inconsistent operand stack height at instruction {target}: {previous} versus {next_height}"
                    )));
                }
                Some(_) => {}
                None => {
                    heights[target] = Some(next_height);
                    pending.push((target, next_height));
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
    use crate::bytecode::{BuiltinId, CompiledScript, OpCode, BYTECODE_VERSION};
    use crate::validator::{compile_dsl_source, ValidationLimits, Validator};

    fn errors(code: Vec<OpCode>, max_loop_depth: u16) -> Vec<String> {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code,
            constants: vec![],
            local_count: 0,
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
            code.extend([CallBuiltin(BuiltinId::LogFmt, argc), Pop, ReturnNext]);
            let errors = errors(code, 0);
            assert!(errors.iter().any(|e| e.contains(expected)), "{errors:?}");
        }
        for argc in [1, 255] {
            let mut code = vec![LoadNil; argc as usize];
            code.extend([CallBuiltin(BuiltinId::LogFmt, argc), Pop, ReturnNext]);
            assert!(errors(code, 0).is_empty());
        }
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
