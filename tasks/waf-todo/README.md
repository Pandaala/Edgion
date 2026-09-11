# WAF Streaming Work Queue

## Purpose

This directory is the canonical execution queue for all remaining Edgion WAF
work. Each numbered file owns one deliverable. The compact completed baseline
is recorded in [00-current-state-and-baseline.md](00-current-state-and-baseline.md).

## Execution order

| Order | Task | Status | Depends on |
|------:|------|--------|------------|
| 1 | [Freeze structured streaming contract](01-structured-streaming-contract.md) | pending | Existing Phase 2 classifier |
| 2 | [Implement incremental JSON tokenizer](02-incremental-json-tokenizer.md) | pending | Task 1 |
| 3 | [Publish bounded JSON values and paths](03-json-value-path-publisher.md) | pending | Task 2 |
| 4 | [Integrate hybrid Phase 2 transaction lanes](04-transaction-hybrid-phase2.md) | pending | Task 3 |
| 5 | [Integrate the Gateway hybrid request-body path](05-gateway-hybrid-request-body-path.md) | pending | Task 4 |
| 6 | [Close JSON streaming validation and performance gates](06-json-validation-and-benchmark.md) | pending | Task 5 |
| 7 | [Implement the bounded incremental XML processor](07-incremental-xml-processor.md) | pending | Task 6 |
| 8 | [Classify response-body rule capabilities](08-response-rule-classifier.md) | pending | Task 7 |
| 9 | [Integrate Phase 4 with RawWindow](09-phase4-rawwindow-integration.md) | pending | Task 8 |
| 10 | [Close Phase 4 validation and Stage 8](10-phase4-validation-and-stage8-closure.md) | pending | Task 9 |

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

When a task starts, change only that file and this index to `in-progress`.
When its exit gate passes, append the exact commands and results to
`00-current-state-and-baseline.md`, mark it `completed`, and advance exactly
one successor.
