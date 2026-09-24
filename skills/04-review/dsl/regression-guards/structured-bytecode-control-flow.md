---
name: structured-bytecode-control-flow
description: Use when reviewing DSL bytecode jumps, lexical loop ownership, or maxLoopIterations enforcement.
knowledge_type: regression-guards
---

# DSL bytecode preserves lexical loop ownership

## Conclusion and scope

`edgion_dsl::resolve_script` validates configured bytecode before either
Controller acceptance or Gateway VM construction. Ordinary `Jump` and
`JumpIfFalse` instructions must move forward and preserve the identity of the
active lexical loop stack. A jump from within a loop may target its `LoopEnd`,
which performs cleanup on execution. `LoopBack` must move backward within its
own innermost lexical loop. Equal numeric nesting depth does not establish
equal loop ownership: sibling loops have distinct counters.

## Rationale and evidence

The VM increments a loop counter only at `LoopBack`. The former validator
checked target bounds and linear `LoopInit`/`LoopEnd` balance, but accepted an
ordinary backward jump that repeatedly executed a loop body without reaching
`LoopBack`. It also accepted jumps that crossed loop boundaries and bypassed
counter cleanup. `maxSteps` still bounded total execution, but the accepted
bytecode could evade the configured `maxLoopIterations` contract.

The validator now records each instruction boundary's innermost active
`LoopInit` index and checks every jump against it. Each index has a fixed
lexical parent chain, so equal indices imply equal active loop stacks. The
compiler sends `while` `continue` to `LoopBack` before revisiting its
condition; the two `for` forms send `continue` to their increment step and
then `LoopBack`. See `edgion-dsl/src/{validator.rs,compiler/loop_.rs,script.rs}`
and `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/control_flow.rs`.

## Alternatives and limits

Counting all backward jumps at VM runtime would still need loop ownership
validation to choose the correct counter and prevent skipped cleanup, while
adding work to the request path. Relying only on `maxSteps` preserves a total
instruction bound but does not enforce the separately configured loop limit.
The validator proves jump structure, not the number of runtime iterations.
Runtime `LoopBack` and step checks remain necessary.

## Verification and re-evaluation

Leaf-crate tests cover malformed serialized jumps through `resolve_script`
and valid compiler output; the Gateway test covers repeated `while continue`
with a low loop limit and a larger step budget. Re-evaluate if new control-flow
opcodes, labeled loop exits, or a different loop-counter scheme are introduced.

## Resolution references

- `tasks/todo/validate-structured-dsl-bytecode-control-flow.md` (Issue N/A):
  structured bytecode validation and `while continue` counter correction.
