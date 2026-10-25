# Evaluate preparation of literal DSL time formats

Status: deferred for user decision. No runtime change.
Source baseline: 782beda86.

## Evidence and decision

`edgion-dsl/src/compiler/expr.rs::compile_fn_call` compiles both `time_format`
arguments in source order and emits generic `CallBuiltin(TimeFormat, 2)`.
Formats may be dynamic. Gateway `plugins/http/dsl/lang/builtins.rs` converts
the format argument, checks its 256-byte bound, then handles the timestamp.
Nil or out-of-range timestamps return nil without formatting. Other timestamp
types fail with TypeError. A valid timestamp formats through Chrono's fallible
writer; malformed formats produce ApiError, and successful output receives the
UTF-8 byte-limit check before becoming an independent Arc string.

Pinned Chrono 0.4.45's DateTime::format creates StrftimeItems. DelayedFormat's
write_to clones that iterator and consumes its parsed items on each invocation.
A literal executed repeatedly therefore repeats parsing. The intermediate
Display result buffer was already removed; this proposal concerns parsing,
not that completed optimization. Vm::new currently prepares regex and JSON
plans but has no time-format plan.

Chrono offers parse_to_owned and format_with_items, but collecting owned items
adds persistent metadata. parse_to_owned rejects malformed items immediately;
using that failure as script/config rejection changes lazy runtime behavior,
including an unreachable call or invalid format paired with a nil timestamp.
Borrowed items also need a lifetime design; they cannot simply borrow a movable
String stored alongside the plan. No representation or new opcode is selected.

Decide whether to measure a literal-only prepared path or retain parsing at
execution. With formats capped at 256 bytes, lookup/ownership overhead may
outweigh avoided parsing for short or infrequently executed formats. No
bottleneck or net throughput/memory improvement has been measured.

## Acceptance before implementation

- Use IMPL and an exact reviewed source allow-list. Preparation belongs to an
  immutable compiled/plugin generation with explicit count, byte and ownership
  bounds and release on replacement. No request-time lazy/global mutable cache.
  Preparing every unrelated string constant is not authorized.
- Preserve dynamic formats and their existing coercion/evaluation order. Keep
  the 256-byte format gate before timestamp handling, nil/out-of-range behavior,
  exact TypeError/ApiError fields and original format diagnostics. A prepared
  invalid marker must not raise an error until the original execution point.
  Do not log supplied formats or weaken panic/error-policy isolation.
- Preserve successful UTF-8 output and exact StringTooLong fields, including
  empty output, escaped percent, UTC fields, whitespace, multibyte literals,
  partial-prefix format failure and invalid-format priority over output limits.
  Keep the direct fallible writer; do not reintroduce Display::to_string.
- Preserve maxSteps, maxCallCount and maxStackDepth admission and failure order.
  A dedicated opcode that omits LoadConst can remove a step and a stack-depth
  check; omitting CallBuiltin can skip tick_call. Output parity alone is not
  sandbox-budget parity. LoadConst does not apply maxStringLen to its literal.
- Any new serialized constant/opcode needs compiler, schema-version policy,
  stack validator, independent Gateway artifact validation and execution review.
  Preserve the runtime-independent DSL leaf and Controller/Gateway dependency
  boundary. Cross-crate edits require their own frozen allow-list; this task
  authorizes none. Do not trust controller-prepared metadata without validation.
- Compare repeated and one-shot literal calls, dynamic fallback, short/long
  formats, valid/invalid formats and nonexecuted branches. Measure parsing work,
  lookup cost, allocations and retained metadata, including generation overlap.
  Retain current tests and add differential and sandbox-boundary coverage before
  accepting an implementation. DSL-01 input-string ownership remains separate.

Original sources: `edgion-dsl/src/{compiler/expr.rs,compiler/builtins.rs,bytecode.rs,validator.rs}`;
Gateway `plugins/http/dsl/{plugin.rs,lang/vm.rs,lang/builtins.rs}`; pinned Chrono
`format/{strftime.rs,formatting.rs}` and `datetime/mod.rs`. The error and sandbox
contracts are in `skills/03-coding/dsl/builtin-error-semantics.md`,
`skills/04-review/dsl/regression-guards/chrono-display-panic.md` and
`skills/01-architecture/02-gateway/dsl-engine-security-model.md`.
