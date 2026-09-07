# D0094 — Reject DSL Builtin Calls Beyond Bytecode Argument Capacity

## Meta

- Status: needs-implementation
- Severity: high
- Discovered in: B0129 plugin runtime log analysis
- Category: compiler-validation
- Canonical task: [`tasks/todo/reject-dsl-builtin-calls-beyond-argument-capacity.md`](../../../todo/reject-dsl-builtin-calls-beyond-argument-capacity.md)

## Evidence

The DSL compiler narrows builtin call argument counts with unchecked `as u8`. Variadic calls such as `log_fmt` can contain more than 255 arguments when configured stack limits permit them, so bytecode records a different arity and the VM can consume the wrong operands rather than rejecting the script.

## Why deferred

The compiler-owned correction belongs in the `edgion-dsl` anchor and must audit every builtin encoding, static validation, VM decoding, and compiler test. B0129 only bounds Gateway diagnostic rendering and cannot treat one call site as the canonical compiler fix.

## Integration-test impact

Compiler and VM unit tests should fully prove the arity boundary. No integration issue is currently needed, and existing frozen integration files must not be modified.
