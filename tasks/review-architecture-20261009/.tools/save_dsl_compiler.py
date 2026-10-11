import record
body='''
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

Future focused tests should compile the actual shared source pipeline and check the returned result for the range witness; cover authored `__list__` and `__idx__` bindings in nested and sibling loops; reject reads/writes of those names when no authored binding exists; and allow them when legitimately declared, including as loop variables. Preserve existing break/continue, side-effectful end/iterable evaluation, local-count and overflow tests. A narrow Gateway return-value wiring check can use the existing session fixture without adding production copies for mock convenience.

## Original decisions and limits

The current language contract states that inner blocks can access authored outer variables and only declared mutable variables can be assigned. The existing scope regression test protects that boundary. The original structured-control-flow and argument/stack guards protect different invariants; they do not authorize source access to compiler temporaries.

The accepted custom-VM, explicit-dispatch and giant-match decisions remain intact. The leave-scope/pop-underflow correction also remains intact; anonymous slot allocation does not require a runtime scope framework or changes to loop cleanup.

Existing review artifacts, DSL knowledge and pending tasks were searched for the temporary names and source/hidden-slot ownership; no matching finding was found. This is one root cause across the two loop lowerings, owned by `loop_.rs`. Supporting changes to the compiler's slot allocator are necessary implementation scope, not a separate finding for `emit.rs`.

All conclusions are static. No tests, builds, applications or network exchanges were executed. No deployed incident, exploit, benchmark improvement or process-memory bound is claimed.
'''
record.candidate('ARCH-206','compiler-private-loop-slots','PART-183','/root/analyze_conversion','behavior-defect','P2',body)
d={p:'No separate actionable finding: compiler-owned explicit lowering, bounded constant/regex admission, exact arity and fallible scope cleanup retained.' for p in record.state()['partitions']['PART-183']['paths']}
d['edgion-dsl/src/compiler/loop_.rs']='ARCH-206: named compiler temporaries contaminate source name resolution in both loop lowerings; anonymous physical slots preserve existing bytecode/runtime contracts.'
d['edgion-dsl/src/compiler/emit.rs']='No separate finding: named allocation is the necessary supporting seam of ARCH-206; bounded indexes and fallible scope cleanup remain coherent.'
record.partition('PART-183','/root/analyze_conversion',d,{'AGENTS.md':'Original full retained, only root AGENTS.','skills/SKILL.md':'Original full retained.','skills/00-design-rules.md':'Original full retained and applicable gates checked.','skills/04-review/knowledge-routing.md':'Original full retained.','skills/04-review/dsl/SKILL.md':'Full original.','skills/03-coding/dsl/SKILL.md':'Full original.','skills/01-architecture/02-gateway/dsl-engine.md':'Full original 1-254.','skills/01-architecture/02-gateway/dsl-engine-security-model.md':'Original trust and loader/runtime bounds 1-205.','skills/04-review/dsl/intentional-design/dsl-design-decisions.md':'Current trust/engine ADR 1-105.','skills/04-review/dsl/intentional-design/giant-match-extract-fn-declined.md':'Full original; retained exact dispatch.','skills/04-review/dsl/intentional-design/compiler-leave-scope-assert.md':'Full original; fallible cleanup remains. Historical no-fallible-work premise does not describe current recursive compilation, but failure discards consumed compiler.','skills/04-review/dsl/regression-guards/dfa-size-limit.md':'Full original.','skills/04-review/dsl/regression-guards/structured-bytecode-control-flow.md':'Full original.','skills/04-review/dsl/regression-guards/builtin-argument-capacity-and-stack-validation.md':'Full original.','edgion-dsl/src/parser/mod.rs':'Full original.','edgion-dsl/src/parser/lit.rs':'Full original.','edgion-dsl/src/parser/stmt.rs':'Actual declarations/loops/assignments 1-28,42-82,145-246,325-364.','edgion-dsl/src/validator.rs':'Full original including recovered gaps.','edgion-dsl/src/validator/stack.rs':'Full original.','edgion-dsl/src/script.rs':'Full original.','edgion-dsl/src/bytecode.rs':'Actual version/builtin/local count and loader 1-23,230-337,399-471.','edgion-dsl/tests/unit/compiler/expr_test.rs':'All actual 398-line tests.','edgion-dsl/tests/unit/compiler/loop__test.rs':'All actual 107-line tests.','edgion-dsl/tests/unit/compiler/stmt_test.rs':'All actual 64-line tests, outer scope visibility.','edgion-dsl/tests/unit/compiler/emit_test/scope_balance_tests.rs':'All actual 35-line tests.','edgion-dsl/tests/unit/compiler/emit_test/record_line_tests.rs':'All actual 18-line tests.','edgion-dsl/tests/unit/compiler/mod_test/source_map_tests.rs':'All actual 61-line tests and test-only compile wrapper.','edgion-gateway/src/plugins/http/dsl/lang/vm.rs':'Actual initialization/locals/counters/results 184-404,435-475,1062-1115,1200-1237.','edgion-gateway/src/plugins/http/dsl/lang/builtins.rs':'Actual ToStr 925-940.','edgion-gateway/src/plugins/http/dsl/plugin.rs':'Shared preparation and request execution 48-140.','edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs':'Actual shared compilation and response prohibition 369-404.','edgion-gateway/tests/unit/plugins/http/dsl/mod_test/control_flow.rs':'All actual 206-line test bodies.','docs/en/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax.md':'Explicit declaration/assignment/lexical contract 86-132.'},['ARCH-206'],'Complete six-file full static primary review and actual tests/call chains. Slot trace is not executed observation. No runtime budget bypass, performance result or deployed incident claimed. Supporting emit/stmt seams share one root cause, not duplicate candidates.')
print(record.state()['candidates']['ARCH-206'])
