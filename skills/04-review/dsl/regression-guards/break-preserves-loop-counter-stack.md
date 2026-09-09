---
name: break-preserves-loop-counter-stack
description: Use when reviewing DSL loop compilation, break backpatching, LoopInit/LoopEnd balance, or max_loop_iterations enforcement in nested loops.
knowledge_type: regression-guards
---

# DSL `break` preserves the VM loop-counter stack

## Trigger

The compiler formerly patched `break` jumps to the instruction after
`LoopEnd`. The VM therefore retained the broken loop's counter. When an inner
loop broke during every outer iteration, each new inner `LoopInit` hid the
outer counter and the outer `LoopBack` incremented the leaked inner counter.
The outer loop could bypass `max_loop_iterations` until the independent step
budget or its condition ended execution.

## Correct behavior

Every entered `while`, range `for`, and list `for` emits one `LoopInit` and
reaches exactly one matching `LoopEnd` on either natural exhaustion or
`break`. `compile_while`, `compile_for_range`, and `compile_for_in` capture the
break target before emitting `LoopEnd`, so break backpatches land on the
cleanup opcode. Natural condition exits already land there.

`continue` is intentionally unchanged: it targets the condition check for a
`while` loop and the increment step for both `for` forms. Break remains scoped
to the innermost active loop. Return behavior, loop-limit threshold semantics,
typed runtime errors, bytecode opcodes, and the bytecode version are unchanged.

## Regression evidence

The table-driven Gateway control-flow regression nests an immediately broken
`while`, range `for`, or list `for` inside an outer `while`. With a low loop
limit and a high step budget, all three cases must return
`RuntimeError::LoopLimitExceeded`, never run until `StepLimitExceeded` or
natural completion. The compiler loop tests continue to cover all three loop
forms and nested break/continue patch-stack balance.

Re-evaluate this guard if loop cleanup moves out of `LoopEnd`, a new loop form
is added, or break/continue acquire labeled or multi-level semantics.
