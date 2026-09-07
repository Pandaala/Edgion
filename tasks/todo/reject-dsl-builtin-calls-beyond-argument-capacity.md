# Reject DSL Builtin Calls Beyond Bytecode Argument Capacity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make DSL compilation reject builtin calls whose argument count cannot be represented exactly by the bytecode instruction, preventing arity truncation and operand-stack misexecution.

## Requirements

- Inventory every compiler path that narrows a builtin or call argument count into the bytecode representation.
- Define and enforce one exact maximum at compile time before any narrowing conversion.
- Return a deterministic static validation error without emitting partial or ambiguous bytecode.
- Preserve valid zero-through-maximum calls, variadic builtin behavior, stack accounting, source spans, and Controller/Gateway compiler parity.
- Audit VM decoding and validation so malformed externally constructed bytecode cannot consume a truncated or inconsistent operand count.
- Keep errors static and non-sensitive; add no runtime logs, metrics, or compatibility shim.

## Acceptance criteria

- Compiler tests cover the maximum valid arity and first invalid arity for fixed and variadic builtins.
- VM/static-validator tests reject malformed arity encodings without panic or operand corruption.
- Shared resource and Gateway validation continue to report the same compiler failure class.
- No integration test is required unless compiler/VM tests cannot prove the boundary.
