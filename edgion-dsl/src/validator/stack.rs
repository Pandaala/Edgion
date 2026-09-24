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
#[path = "../../tests/unit/validator/stack_test.rs"]
mod tests;
