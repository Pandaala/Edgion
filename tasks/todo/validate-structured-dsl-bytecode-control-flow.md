# Validate Structured DSL Bytecode Control Flow

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Reject hand-crafted DSL bytecode whose jumps violate lexical loop ownership, so every accepted loop remains governed by `maxLoopIterations` rather than only the broader step budget.

## Requirements

- Treat configured bytecode as untrusted input at `edgion_dsl::resolve_script`; preserve Controller and Gateway validation parity.
- In the bytecode validator, derive the active lexical loop-stack identity for every instruction boundary from matching `LoopInit` and `LoopEnd` opcodes. Numeric nesting depth alone is insufficient because sibling loops can have equal depth but different counter ownership.
- Reject backward `Jump` and `JumpIfFalse` targets.
- Require ordinary conditional and unconditional jumps to preserve the same active lexical loop-stack identity. A source-level `break` may target its matching `LoopEnd` because that cleanup has not executed at the target instruction.
- Reject jumps that enter a loop, skip loop cleanup, or cross between sibling loops.
- Require `LoopBack` to target backward within the same active innermost lexical loop.
- Retain existing target-bound and linear loop-balance validation as defense in depth.
- Preserve compiler-produced `if`/`else`, logical short-circuit, while, for-range, for-in, continue, and break bytecode without compatibility shims or bytecode-version changes.
- Preserve typed validation failures, the workspace DAG, and existing runtime step-limit defense.
- Update the DSL security model and maintained review knowledge with the accepted structured-control-flow contract.
- Do not modify existing `edgion-tests/**` files; deterministic leaf-crate tests are sufficient.

## Scope

### In scope

- `edgion-dsl` bytecode control-flow validation and public deserialize/resolve boundary tests.
- Directly affected DSL security and maintained review knowledge.

### Out of scope

- Gateway VM dispatch changes, new runtime limits, scalar-range policy, bytecode compatibility layers, logging, metrics, schemas, and integration tests.

## Acceptance criteria

- A balanced bytecode vector containing an ordinary backward jump is rejected before VM construction.
- Jumps that enter a loop, skip its `LoopEnd`, or cross sibling loops are rejected even when source and target have the same numeric nesting depth.
- Invalid forward or cross-loop `LoopBack` instructions are rejected.
- Representative malformed serialized scripts fail through `resolve_script`, not only through a private validator test.
- Valid compiler output for nested loops, break, continue, branches, and short-circuit expressions remains accepted.
- Focused `edgion-dsl` tests and all applicable repository guards pass.
