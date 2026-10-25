---
name: dsl-step-budget-not-cpu-bound
description: Use when assessing DSL opcode/call budgets versus synchronous work; dispatch counts and output-byte caps do not prove a universal CPU bound.
knowledge_type: intentional-design
---

# DSL step budgets count dispatches, not CPU time

## Current contract

`VmState::tick_step` charges one step before opcode dispatch. CallBuiltin also
charges `tick_call` before invoking its handler. These are operator-visible
execution counters. Work inside a synchronous regex operation or builtin is not
charged once per byte and is not preempted by those counters.

Keep this distinction when proposing optimizations. Removing a literal load or
replacing CallBuiltin with a specialized opcode can change step, call or stack
admission even when successful output is identical. A change to charging policy
requires an explicit design and regression coverage; the counting contract alone
is not grounds to dismiss a concrete resource-exhaustion finding.

## Superseded reasoning

The earlier page and VM comment described `max_steps * max_string_len` or
`max_call_count * max_string_len` as a universal CPU bound, claimed no tenant
could exceed a four-cap product, and automatically rejected weighted-budget
proposals. Those conclusions are withdrawn. No complete cost proof supported
them, and the assumed universal runtime input-length bound is false.

- Validator string constants have separate limits (default 4096 bytes per
  string and 32768 bytes for the constant pool). LoadConst reuses a cached Arc;
  VmState::push checks depth and container shape, not every Str's runtime limit.
  Thus a legal literal can exceed a smaller configured max_string_len.
- Split checks list count before construction, but does not check each element's
  string length. Range has its own count ceiling and likewise no per-element
  string check. These are current observations, not approval of permanent
  exemptions or proof of a complete container-memory bound.
- Case conversion and successful Base64/hex decoding check final string bytes
  after their conversion storage is constructed. An output check is not a
  universal input, temporary-allocation or scan-work bound.
- Regex DFA size limits constrain engine memory. They do not themselves prove
  an elapsed-time bound. Analyze the selected operation and input provenance;
  output caps cannot be substituted for a search-complexity analysis.

Other limits still matter: validated program size, container/value work budgets,
body scan budgets, JSON/XML budgets and bounded outbound exchanges each have
specific enforcement points. This page neither removes those protections nor
claims an unbounded execution path. See the [security model](../../../01-architecture/02-gateway/dsl-engine-security-model.md)
for the maintained inventory; derive a cost argument from the actual path.

## Evidence and review criteria

Original sources are Gateway `plugins/http/dsl/lang/vm.rs` (dispatch, LoadConst,
push), `lang/builtins.rs` (Len, Split, Range, conversion/decoding), and
`edgion-dsl/src/validator.rs` (constant limits). The mirrored utility test
`test_runtime_string_limit_is_not_a_universal_input_bound` demonstrates literal,
Split and Range execution with max_string_len set to one. It records current
behavior; it does not approve a new sandbox policy or measure CPU cost.

A future CPU/heap claim must account for input origins, repeated work, algorithm
complexity, temporary owners, persistent generation storage and all applicable
budgets. Re-evaluate on changes to constant admission, container elements,
builtin implementations or execution charging. Weighted accounting may be
considered with evidence and explicit policy review; it is neither mandated nor
rejected by this page. Historical green tests are not a cost proof.
