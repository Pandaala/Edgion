# WAF Streaming Work Queue

## Purpose

This directory is the canonical execution queue for all remaining Edgion WAF
work. Each numbered file owns one deliverable. The compact completed baseline
is recorded in [00-current-state-and-baseline.md](00-current-state-and-baseline.md).

## Current checkpoint (2026-10-06)

The fused native JSON checker and prior allocation optimizations are implemented.
[Plan 16](16-json-capture-wiring-and-closure.md) connects the existing parser to
real request-body capture chunks, adds provisional capacity and exact inspected
source accounting, and reconciles the old queue. Completed validation and final
Astra approval are tracked in [task 06](06-json-validation-and-benchmark.md).

JSON-only Full inspection retains selected facts rather than a DOM or complete
field map and avoids an additional complete body materialization. Canonical
BodyStorage still stores the proxied request for forwarding/retry. Raw visibility
may require one materialization. EOF sealing and canonical source-order effects
remain before upstream dispatch. These changes do not imply universal CPU gains,
cache-free forwarding or promotion of the reserved StreamingStructured classifier.
Older sections in tasks 00–05 are explicitly historical, superseded records.

The original JSON rule classification has also been reconciled with current
source ownership and capabilities; its
[current contract](../pending/waf-json-rule-classification/08-current-contract.md)
separates immutable analyst history from baseline-bound mechanism assessments
and untested original-unit admission. Its current/historical artifact checks are
read-only by default. This documentation correction introduces no WAF behavior
or performance change.

The bounded native XML deliverable is also complete: actual capture/spill wiring,
strict grammar/namespace/duplicate checks, shared compiled facts and canonical
replay, final WafDynamic and release evidence passed independent Astra review.
Its [results](../pending/waf-xml-checker/06-performance-results.md) disclose
significant workload regressions as well as gains; this is not an overall CPU
or allocation-total improvement claim. Public XML selectors remain the documented
subset. JSON classification hashes remain historical, not refreshed acceptance.

## Execution order

| Order | Task | Status | Depends on |
|------:|------|--------|------------|
| 1 | [Freeze structured streaming contract](01-structured-streaming-contract.md) | completed for current JSON; general classifier deferred | Existing Phase 2 classifier |
| 2 | [Implement incremental JSON tokenizer](02-incremental-json-tokenizer.md) | completed (native lexer) | Task 1 |
| 3 | [Publish bounded JSON values and paths](03-json-value-path-publisher.md) | completed (fused selected retention) | Task 2 |
| 4 | [Integrate hybrid Phase 2 transaction lanes](04-transaction-hybrid-phase2.md) | completed for current JSON; general scheduler deferred | Task 3 |
| 5 | [Integrate the Gateway JSON request-body path](05-gateway-hybrid-request-body-path.md) | completed (actual capture chunks) | Task 4 |
| 6 | [Close JSON streaming validation and performance gates](06-json-validation-and-benchmark.md) | completed (current fused Full JSON) | Task 5 |
| 7 | [Implement the bounded incremental XML processor](07-incremental-xml-processor.md) | completed; final Astra approval | Task 6 |
| 8 | [Classify response-body rule capabilities](08-response-rule-classifier.md) | pending | Task 7 |
| 9 | [Integrate Phase 4 with RawWindow](09-phase4-rawwindow-integration.md) | pending | Task 8 |
| 10 | [Close Phase 4 validation and Stage 8](10-phase4-validation-and-stage8-closure.md) | pending | Task 9 |

Tasks 01–05 describe the current JSON deliverable and preserve earlier designs
as historical evidence. XML implementation, measured validation and final independent Astra review are
complete. Task 08 is next for scoped planning; response work remains pending. General classifier
promotion and a simultaneous three-lane scheduler require a separate scoped plan.

## JSON follow-up candidates

The current deliverable and task 06 are closed. A user-requested independent size
audit found two measurement candidates: avoid artificial 64KiB partitioning for a
proven resident single-segment cache, and cap token growth softly using validated
cached source length. These are not implemented or part of the completed gate;
see [plan 16's size audit](16-json-capture-wiring-and-closure.md).

## Deferred optimization investigations

These tasks are not part of the Stage 8 critical path and start only when their
activation condition is true.

| Task | Status | Activation |
|------|--------|------------|
| [Hyperscan or VectorScan](11-hyperscan-vectorscan-investigation.md) | pending | Measured multi-regex bottleneck after Stage 8 |
| [Native SIMD packed search](12-native-simd-packed-search-investigation.md) | pending | Measured scalar phrase bottleneck |
| [Premultiplied DFA state layout](13-dfa-state-layout-investigation.md) | pending | DFA row indexing is material in a profile |
| [Regex literal prefilter](14-regex-literal-prefilter-investigation.md) | pending | Exact regex execution is material in a profile |
| [x86-64 portability evidence](15-x86-64-portability-evidence.md) | pending | x86-64 enters supported deployment scope |

## Standing constraints

- `edgion-waf` remains a runtime-independent leaf crate.
- No external WAF engine or streaming parser becomes a production dependency.
- Raw phrase inspection remains a complete constant-state stream and never
  depends on the compatibility snapshot lane.
- Only the canonical request `BodyStorage` may materialize a request snapshot.
- The main upstream response remains streaming and never gains a response-side
  `BodyStorage`.
- Per-request WAF state remains in the dedicated `EdgionHttpContext.waf` box.
- Request-path labels remain low-cardinality and the body path emits no tracing.
- Hyperscan/VectorScan remains an optional candidate-only investigation in
  `11-hyperscan-vectorscan-investigation.md`.
- Compilation-lock changes and PluginStore-wide memory admission budgets remain
  measurement-driven work, not implicit parts of this queue.

## Status update rule

Track implementation and acceptance separately. The continuation above reconciles
historical work spread across tasks 01, 03 and 04; keep one implementation batch
active even when several deliverables retain open acceptance gates.
When its exit gate passes, append the exact commands and results to
`00-current-state-and-baseline.md`, mark it `completed`, and advance exactly
one successor.
