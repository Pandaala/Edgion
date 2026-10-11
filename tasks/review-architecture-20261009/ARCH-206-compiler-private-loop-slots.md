---
id: ARCH-206
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-183
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-206 — Keep compiler-owned loop temporaries outside source name resolution

## Current ownership and evidence

`Compiler::compile_for_range` in `edgion-dsl/src/compiler/loop_.rs:31-34` allocates its cached upper bound with `declare_local("__range_end__", false)`. `compile_for_in` registers its retained iterable and mutable counter as `__list__` and `__idx__` (`123-132`). These are physical compiler temporaries, but `declare_local` also appends them to the ordinary source-visible `Local` table (`compiler/emit.rs:142-155`). `resolve_local` searches that table from the newest entry backward (`158-160`).

The parser permits leading underscores and reserves language keywords, not these names (`parser/lit.rs:77-85`; `parser/mod.rs:127-135`). The compiler rejects the three protected namespaces as user variables, but imposes no internal-name reservation. Ordinary identifier expressions and assignments use `resolve_local` (`compiler/expr.rs:39-50`; `compiler/stmt.rs:37-55`).

Consequently, this accepted source has a different result from its documented lexical meaning:

```dsl
let __range_end__ = 99
let mut result = 0
for i in range(0, 1) {
    result = __range_end__
}
return deny(403, to_str(result))
```

The outer authored binding occupies slot 0. The compiler subsequently registers its cached bound, value 1, under the same name in slot 2. The loop body's read resolves to slot 2 and stores 1 in `result`; the returned body is therefore `"1"` rather than `"99"`. This is a static instruction/slot trace, not an executed response observation.

For-in has the same ownership defect. Without an authored declaration, an assignment such as `__idx__ = 100` inside a for-in body is accepted because the compiler registered its hidden counter as mutable. That assignment changes iteration control. Existing step and loop budgets still bound execution; no sandbox-budget bypass is claimed.

The shared source path does not reject these programs later. `compile_dsl_source` compiles and validates the script, and `resolve_script` supplies it to resource validation and Gateway construction. The witness is well inside ordinary instruction/local/constant limits. Validator checks physical local indices, stack flow and lexical loop ownership; it has no source-name table with which to recover the intended binding. Gateway `GetLocal` and `SetLocal` execute the emitted physical slots. Actual `DslConfig::compile_and_validate`, Gateway factories and VM initialization therefore preserve the incorrect lowering rather than repairing it.

## Root cause and bounded improvement

Compiler-owned temporary storage shares the source variable namespace. Separate those two responsibilities within the existing compiler.

Reserve anonymous physical slots for the cached range bound, for-in iterable and for-in counter without adding named `Local` entries. Retain the existing named declaration path for authored variables and the source-visible loop variable. A small private slot-allocation primitive can retain the current u16 overflow check and monotonic `next_slot` accounting; both temporary allocation and named declaration can use it.

Continue emitting the existing GetLocal/SetLocal instructions with the allocated temporary indices. Keep temporary slots included in `local_count`, so Validator and VM admission remain unchanged. No new opcode, bytecode version, runtime name lookup, public framework, language feature or request allocation is required.

Renaming the three synthetic names only moves the collision to other valid identifiers. Reserving a new identifier prefix would reject otherwise legal user variables and is unnecessary when anonymous storage is already represented by numeric slots. A runtime workaround would add request work and cannot reconstruct lexical intent from the current slot-only bytecode.

## Required invariants and verification

Preserve evaluation order and frequency: the range end is evaluated once before its start, and a for-in iterable is evaluated once before iteration. Preserve immutable authored loop variables, legal authored shadowing, outer-binding visibility and restoration, scope cleanup, physical slot limits and total local counts.

Preserve all existing loop control: natural exit and break target LoopEnd; range/for-in continue reaches increment before LoopBack; while continue reaches LoopBack; nested loops retain distinct counters. Keep independent step, loop, call, stack and value-work limits. Keep exact enum dispatch, checked builtin argument counts, constant deduplication, regex/DFA bounds, source-map coverage and independent validation of configured bytecode.

This is a source-lowering correction. Explicit physical slots in existing serialized bytecode retain their existing meaning. Preserve bytecode version 5, source/bytecode precedence, Controller/Gateway validation independence, the leaf dependency boundary, opaque request Body semantics, response-body aggregation rejection and current runtime/session ownership. No new logging, metrics, outbound transport or shared compiled cache is needed.

Existing compiler tests cover scope cleanup and outer-binding visibility for if blocks, nested loop patch-stack balance, loop forms, argument encoding and source maps. Actual Gateway control-flow tests cover break/continue behavior, nested counter ownership and return values. None of the inspected tests exercises collisions with the three compiler temporary names.

Future focused tests should compile the actual shared source pipeline and check the returned result for the range witness; cover authored `__list__` and `__idx__` bindings in nested and sibling loops; reject reads/writes of those names when no authored binding exists; and allow them when legitimately declared, including as loop variables. Retain existing break/continue and local-count coverage. Add focused checks for side-effectful end/iterable evaluation and private-slot overflow boundaries; do not treat these as existing test coverage. A narrow Gateway return-value wiring check can use the existing session fixture without adding production copies for mock convenience.

## Original decisions and limits

The current language contract states that inner blocks can access authored outer variables and only declared mutable variables can be assigned. The existing scope regression test protects that boundary. The original structured-control-flow and argument/stack guards protect different invariants; they do not authorize source access to compiler temporaries.

The accepted custom-VM, explicit-dispatch and giant-match decisions remain intact. The leave-scope/pop-underflow correction also remains intact; anonymous slot allocation does not require a runtime scope framework or changes to loop cleanup.

Existing review artifacts, DSL knowledge and pending tasks were searched for the temporary names and source/hidden-slot ownership; no matching finding was found. This is one root cause across the two loop lowerings, owned by `loop_.rs`. Supporting changes to the compiler's slot allocator are necessary implementation scope, not a separate finding for `emit.rs`.

All conclusions are static. No tests, builds, applications or network exchanges were executed. No deployed incident, exploit, benchmark improvement or process-memory bound is claimed.
