# List-loop source ownership

Status: deferred for user decision. No runtime change.
Source baseline: 50093d98a.

## Evidence and scope

`edgion-dsl/src/compiler/loop_.rs::compile_for_in` evaluates its iterable once
and stores it in an anonymous local. Each condition emits GetLocal(list) then
ListLen; each entered iteration emits another GetLocal(list) before IndexGet.
Natural completion of an N-item list within all budgets therefore reads that
retained local 2N+1 times, excluding initial evaluation and reads in the body.
Early exits, failures and other loop compilation paths have different counts.

In `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`, GetLocal inspects shape,
charges conservative copy work, clones the value, then pushes it through stack
admission and another container-shape inspection. The canonical Value in
`edgion-dsl/src/value.rs` stores a List as Vec<Arc<str>>. Each clone copies the
container and updates Arc counts; it does not copy string payloads. ListLen
consumes and discards that copy. IndexGet moves one Arc from its consumed copy
with swap_remove, leaving the retained local unchanged.

The repeated container work can grow with both list length and iteration count.
This is cumulative work, not evidence of quadratic retained memory or a measured
request-throughput bottleneck. Current loop, step and value-work limits constrain
execution before arbitrary input sizes or iteration counts can complete.

## Decision requested

Retain the current simple value-copy model, or investigate a bounded compiler/VM
mechanism for reading the retained loop source without repeatedly cloning it.
Possible directions include an explicitly validated local-list read or prepared
iteration state. Length hoisting alone leaves the element-access copy; moving
from the retained list with the current swap_remove changes subsequent order.
No design is selected here.

The source must still be evaluated once and retained independently from authored
variables. General GetLocal last-use moves require control-flow/liveness proof,
not the last textual read; loops, branches and repeated reads invalidate that
shortcut. This task does not authorize a whole-Value copy-on-write migration,
general local-liveness optimizer, string coercion redesign or a new cache.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Measure cumulative allocation/copy
work, retained memory and CPU for empty/small/large lists, short and multibyte
strings, nested loops and early exits. Distinguish construction, traversal and
body work; do not extrapolate an isolated clone benchmark into request throughput.

Preserve the current shape inspection, conservative work charges, step/loop/call
limits, stack limits and failure ordering unless a separately reviewed user
decision explicitly approves a changed budget contract. Removing real copies
does not itself authorize larger scripts to pass existing sandbox budgets.
Any new accounting metadata needs bounded construction and invalidation rules.

An extra hidden local affects monotonic local_count and allocation exhaustion.
Opcode fusion or new instructions affect stack validation, jump/loop structure,
source compilation and untrusted-bytecode acceptance. Derive or validate every
runtime assumption independently at Gateway construction; a compiler assertion
alone is not sufficient. Keep anonymous slots inaccessible to authored names,
and preserve break/continue targets and lexical counter ownership.

Verify single evaluation and ordering, original-variable reuse and reassignment,
iteration snapshot isolation, element order, empty/non-list behavior, nested
loops, break/continue/return, malformed bytecode and exact budget boundaries.
Runtime-independent compiler/value owners must not gain Gateway dependencies.
No new body owner, response aggregation or request-level logging is authorized.

At this baseline, 513 Gateway DSL tests passed. Existing loop-control, anonymous
slot, list order/local-reuse and sandbox-budget tests are starting evidence, not
acceptance of a new execution representation. Original contracts include
`skills/01-architecture/02-gateway/dsl-engine-security-model.md`,
`skills/04-review/dsl/regression-guards/structured-bytecode-control-flow.md`, and
`skills/04-review/dsl/regression-guards/builtin-argument-capacity-and-stack-validation.md`.
