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
and no residual operands at returns or fallthrough. It also tracks whether each
stack value comes directly from `LoadConst` of `Constant::Str`. The first
`LogFmt` argument must have that provenance on every reachable path. At joins,
provenance weakens by intersection; a weakened state is propagated again,
including around loops. The opcode stack-effect match is exhaustive and return
instructions end propagation. Analysis remains bounded by the instruction limit,
but is no longer a single visit per PC or linear in code length.

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

The provenance check is specific to `LogFmt`; it is not a general bytecode type
verifier or a placeholder-count check. `GetLocal`, operations, and builtin
results cannot supply the template even if their runtime value happens to be a
string. Unreachable instructions retain structural/arity checks but do not
participate in stack or provenance propagation. The VM and bytecode version do
not change; this validation rejects previously accepted dynamic templates.

## Verification and re-evaluation

Regression tests cover 255/256/257 arguments, no partial operand emission,
fixed/ranged arities, invalid wire counts, old truncated bytecode, stack underflow,
join/back-edge conflicts, terminal residue, nested calls, short circuiting, and
all three loop forms. Gateway tests cover source and bytecode rejection parity;
the existing 255-argument logging test covers VM execution and masking modes.

The template-provenance correction adds hand-crafted bytecode tests for dynamic
request-header templates, local-variable and operation results, non-string
constants, literal templates with nested value calls, branch joins, and loop
back-edge weakening. A Gateway test checks shared config validation and both
request/response factories with bytecode taking priority over valid source.

Verification for the template-provenance correction (2026-09-23):

- `cargo test -p edgion-dsl --lib --quiet`: 128 passed.
- `cargo test -p edgion-gateway --lib dynamic_log_template_bytecode_is_rejected_by_both_factories --quiet`:
  1 passed.
- `cargo test -p edgion-gateway --lib test_log_fmt_ --quiet`: 10 passed,
  including existing masking-mode coverage.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets`, agent-doc validation, SSA-force,
  metrics inventory, Pingora source policy, and Gateway API transform-contract
  checks: passed.
- Independent read-only Astra review: LGTM.

Historical verification for the argument-capacity correction (not rerun by this record):

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
  and inconsistent bytecode operands; corrected in the earlier change.
- `tasks/todo/validate-dsl-bytecode-log-template-provenance.md` (Issue N/A) —
  dynamic `log_fmt` templates in hand-crafted bytecode; corrected by shared
  literal-template provenance validation.
