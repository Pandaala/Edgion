---
name: builtin-argument-capacity-and-stack-validation
description: Review DSL builtin argument encoding, variadic log calls, and bytecode operand-stack validation.
knowledge_type: regression-guards
---

# Builtin argument capacity and operand-stack validation

## Conclusion and scope

`OpCode::CallBuiltin(BuiltinId, u8)` represents at most 255 arguments. Every
compiler path carrying a source argument count uses `checked_builtin_argc`
before compiling operands. Each builtin also retains its own arity contract;
`LogFmt` requires 1 through 255 total arguments (template plus up to 254 values).
The bytecode representation and VM hot path are unchanged.

After existing structural checks pass, `Validator` verifies reachable operand
stack heights: no underflow, matching heights at control-flow joins/back edges,
and no residual operands at returns or fallthrough. The opcode stack-effect
match is exhaustive; each reachable PC is scheduled once, so analysis is linear
in the instruction-bounded code length. Return instructions end propagation.

## Rationale and evidence

The former `args.len() as u8` appeared in namespace, ordinary, and special
`log_fmt` compilation paths. Fixed/ranged builtins already rejected excess
arguments, but `log_fmt` only checked template syntax and placeholder count.
256 arguments became zero; 257 became one. The VM then either rejected the
missing template or consumed the last value as the unmasked template, leaving
the other operands on the stack. The latter is reachable with a sufficiently
large configured stack limit; the default stack limit could fail earlier.

The correction is owned by `edgion-dsl/src/compiler/{expr,builtins}.rs` and
`edgion-dsl/src/validator/stack.rs` (entry point: `src/validator.rs`).
Serde's `u8` decoder already rejects out-of-range wire counts without narrowing.
Stack validation additionally rejects old truncated payloads whose encoded count
is itself in range but whose operands remain on the stack.

`DslConfig::compile_and_validate` and both Gateway factories use the shared
`resolve_script` boundary. This preserves the independent Gateway validation
required by [the security model](../../../01-architecture/02-gateway/dsl-engine-security-model.md)
and the [design rules](../../../00-design-rules.md).

## Alternatives and limits

Widening the opcode or increasing `maxStackDepth` does not repair unchecked
narrowing. The compiler returns a static capacity error before compiling the
call's operands; it publishes no partial script.

This is stack-height validation, not a type or provenance verifier. Balanced
hand-crafted bytecode may deliberately supply a dynamic template or clean up
extra operands itself. Template provenance still needs a separate review; do
not claim arbitrary bytecode templates are safe. Unreachable instructions
retain structural/arity checks but do not participate in height propagation.

## Verification and re-evaluation

Regression tests cover 255/256/257 arguments, no partial operand emission,
fixed/ranged arities, invalid wire counts, old truncated bytecode, stack underflow,
join/back-edge conflicts, terminal residue, nested calls, short circuiting, and
all three loop forms. Gateway tests cover source and bytecode rejection parity;
the existing 255-argument logging test covers VM execution and masking modes.

Verified during this fix:

- `cargo test -p edgion-dsl --lib --quiet`: 125 passed.
- `cargo test -p edgion-gateway --lib plugins::http::dsl:: -- --nocapture`:
  454 passed; the subsequently added
  `truncated_builtin_bytecode_is_rejected_by_both_factories` test passed separately.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets`: passed without warnings.
- Agent-doc, SSA-force, metrics inventory, Pingora source policy, and Gateway API
  transform contract checks: passed.
- Independent read-only review: LGTM.

Re-evaluate whenever an opcode's stack effect, builtin arity,
control-flow semantics, bytecode encoding, or loading boundary changes.

## Resolution references

- `reject-dsl-builtin-calls-beyond-argument-capacity` — source count truncation
  and inconsistent bytecode operands; corrected in this change.
