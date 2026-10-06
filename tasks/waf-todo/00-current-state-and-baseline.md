# WAF Current State and Baseline

## Authoritative checkpoint (2026-10-06)

Status: Current fused JSON baseline verified and independently approved.

The canonical Full JSON entry is now `JsonInspectionSession`: actual capture chunks feed the existing native checker, EOF seals selected facts, and Phase 2 replays transaction effects in source order. JSON-only inspection avoids complete BodyStorage materialization. Prior optimization rounds and FTW records remain valid historical evidence, not fresh proof of this new entry.

Current implementation, verification and independent review are tracked in
[plan 16](16-json-capture-wiring-and-closure.md) and
[validation task 06](06-json-validation-and-benchmark.md).
These checkpoints supersede the historical interfaces and acceptance statuses
below. Historical measurements retain their original scope and revisions.

## Historical checkpoints (superseded design)

> JSON design supersession (2026-10-05): the active replacement and validation
> record is [the fused JSON checker task](../pending/waf-json-checker/waf-json-checker.md).
> Its single grammar pass, selected retention, canonical Bytes publication and
> Full-mode JSON execution replace the earlier narrow subscriber/snapshot
> APIs described below. Earlier measurements and acceptance reports remain
> historical evidence. Broader transport-streaming, XML and response work
> are not completed by this replacement.


## Purpose

This file is the compact handoff baseline for the remaining WAF work. It
replaces the completed historical task tree that was removed after its open
items were migrated into this directory.

## Completed architecture

- `edgion-waf` is a runtime-independent leaf crate with no workspace-internal
  dependencies and no production dependency on another WAF engine.
- The crate owns bounded SecLang parsing, compilation, immutable generations,
  five-phase transaction semantics, body processors, and the native
  Aho-Corasick matcher.
- The native v1 CRS 4.25.1 executable profile admits and passes 4,682/4,682
  stages in accelerated and scalar-reference modes, including 880 explicit
  negative stages across 17 rule families.
- One top-level `spec.waf` logical plugin spans the accepted phases.
- Controller validation compiles declarative rules for status. `conf_sync`
  sends declarative configuration, never a native matcher image. The Gateway
  compiles an architecture-local immutable generation and atomically publishes
  a complete `PluginRuntime` while in-flight requests retain their pinned Arc.
- Global, Gateway, and Route candidates select one WAF by highest priority;
  equal values prefer Route, then Gateway, then Global, then declaration order.
- Backend-scoped WAF rejects HTTPRoute and GRPCRoute with
  `Accepted=False/UnsupportedValue`, participates in plugin dependency requeue,
  preserves delegated namespaces, and retains a Gateway fail-closed backstop.

## Completed streaming and matcher work

- Exact raw phrase rules scan post-mutation request chunks with constant-state
  Aho-Corasick and do not populate `BodyStorage`.
- Cross-chunk matches, body/work limits, abandoned input, request retry
  latching, and exact-once Phase 2 metrics are covered.
- A compiler-proven standalone positive raw phrase can terminate before its
  triggering chunk is forwarded; source-order-sensitive units finalize at EOS.
- Per-request WAF transaction, owner, phase claims, stream state, counters,
  timing, and RAII active-byte accounting live in `EdgionHttpContext.waf`.
- The bounded 2-gram accelerator uses one 65,536-byte immutable table per
  selected matcher and no body-length-dependent request allocation.
- A dependency-free tracked benchmark asserts scalar equivalence, intended
  accelerator selection, exact acceleration memory, and stable scratch.

## Observability baseline

The fixed-label metric set includes compile count/time, live generation bytes,
active request runtime bytes, inspected body bytes, and execution count/time by
phase. Structured-parser and response-window outcomes must extend this set
without resource, route, rule, path, or client labels.

## Latest recorded ARM64 smoke results

| Case | Prefilter off | Prefilter on | Accelerator |
|------|--------------:|-------------:|-------------|
| 64 patterns, zero hit | 244.30 MiB/s | 1,293.48 MiB/s | TwoGram |
| 64 patterns, late hit | 247.83 MiB/s | 1,281.05 MiB/s | TwoGram |
| 256 patterns, high entropy | 162.54 MiB/s | 1,847.54 MiB/s | TwoGram |
| Rare-byte zero hit | 225.11 MiB/s | 9,663.28 MiB/s | RareBytes |

These Apple M4 results are same-host smoke evidence, not portable thresholds.

## Latest recorded validation

- `edgion-waf` library: 349/349 tests passed.
- Gateway WAF adapter: 8/8 tests passed.
- HTTP delegation: 30/30 tests passed.
- gRPC delegation: 34/34 tests passed.
- Controller backend-WAF ownership and derived-namespace tests passed.
- Workspace check and clippy passed; clippy printed unrelated repository
  warnings.
- CRD, agent-document, metrics-inventory, SSA, Pingora-source, formatting, and
  diff guards passed.
- The full `cargo test --all` and dynamic integration suite were not rerun for
  the latest streaming checkpoint.

## Remaining mainline

The numbered tasks in `README.md` are authoritative. The mainline exit blockers
are incremental JSON/XML Phase 2, their Gateway hybrid lane and metrics, and
canonical `RawWindow` Phase 4 response scanning.

## S2 lexical foundation checkpoint (2026-09-29)

Task 02's transport-neutral tokenizer is complete as an isolated leaf API. It
supports bounded incremental JSON syntax and provisional tokens only; no
structured execution lane or Gateway integration is enabled. Task 01 remains
open for exact publication/eligibility and metadata-budget proofs.

Fresh commands: `cargo test -p edgion-waf --lib json_stream` (15 passed),
`cargo test -p edgion-waf --lib --quiet` (415 passed, 2 ignored),
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
formatting and project static guards passed. The explicit pinned CRS target/
operator inventory passed: 289 Phase 2 units, 215 snapshot and 74 raw classes;
modified-corpus rejection passed. Full reproduction, corpus/projection hashes,
code hashes, independent two-round review evidence and limits are in tasks 01
and 02. No FTW verdict or Gateway wire acceptance is claimed by these results.

Dependency audit has identical baseline/working findings, including the
Wasmtime advisory now tracked in `tasks/todo/wasmtime-rustsec-2026-0316-triage.md`.
No dependency-policy exemption or production package-version change was made.

## S19 withdrawal and continuation (2026-10-05)

Starting HEAD `6c89d137`. Independent review and coordinator revalidation confirmed
that restoring only json_rules.rs and its mounted tests to `da048aef` removes
only the S19 experiment. S15 scalar reuse and S18 metadata reductions remain;
S18 CPU calibration is unresolved. Historical measurements are retained in task
01, without treating their process IDs as live work.

Fresh `cargo test -p edgion-waf --lib --quiet` passed 519 tests, with 2 ignored.
Fresh `cargo test -p edgion-waf --test json_execution_api --quiet` passed 5 tests.
Logs: `/tmp/waf-continue-s19-tests.log`, `/tmp/waf-continue-api-tests.log`.
No full workspace test was run. Current adapter plan and exact remaining gates
are in task 05; the index now distinguishes implemented leaf APIs from acceptance.

## Gateway JSON snapshot delivery (2026-10-05)

An explicit full-body JSON snapshot adapter is implemented and independently
reviewed, including structural CRD coverage, mounted tests and mirrored WafDynamic
fixtures. Gateway WAF tests passed 29 cases; resource WAF tests passed 22; the
focused CRD parity test passed, with 19 structural and 8 condition coverage tests.
Workspace format/check/clippy and all nine quick-check gates passed, as did the
data-plane tracing guard.

Coherent integration preparation passed. Shared `Gateway/WafDynamic` then passed
in 81 seconds, with H1/H2 exact forwarding, spill, invalid-retention, cancellation,
deletion/recreation and in-flight rule/budget/mode pinning. Final metrics/config,
requeue and cache-residue guards passed. Report:
`tmp/integration_testing/testing_20261005_103649/report.log`.
The narrowly selected scenario's macOS loopback preflight was corrected after
independent fixture verification; no system network settings were changed.

Task 05 records the exact delivery and commands. This closes the adapter checkpoint,
not simultaneous three-lane execution, compiler promotion or Stage 8. Task 06
still owns differential/performance and provisional-memory/inspected-byte gates.
No default activation, performance gain, commit or push is claimed.

## Bounded native XML delivery (2026-10-06)

Task 07 is complete. XmlStream and the Json/Xml inspection domain validate and
match incrementally without a DOM or complete extracted value map. Contiguous
values borrow; cross-chunk/decoded values and scoped names reuse bounded scratch.
Canonical request BodyStorage remains the only forwarding/retry cache. Four
measured optimizations and all required independent-review corrections are closed.

Final commands and results (current frozen source):

- `cargo test -p edgion-waf --lib --quiet`: 679 passed, 2 existing ignored.
- `cargo test -p edgion-waf --test xml_execution_api --quiet`: 3 passed.
- `cargo test -p edgion-waf --test json_execution_api --quiet`: 9 passed.
- `cargo test -p edgion-gateway --lib json_capture --quiet`: 10 passed.
- `cargo test -p edgion-resources --lib xml_ --quiet`: 2 passed.
- `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`
  and `cargo fmt --all -- --check`: passed, no compiler/lint warnings.
- Agent docs, SSA force, metrics, Pingora source, transform contract, unit layout
  and Gateway no-tracing guards: passed. Baseline 643 to final 681 unit identities,
  removed 0 and added 38. No full workspace test was requested.
- `./edgion-tests/integration/scripts/integration/run_integration.sh -r Gateway -i WafDynamic`:
  passed against rebuilt final artifacts, 101-second scenario; 17 checks passed,
  one intended skip, zero failures; canonical service cleanup completed.
- `cargo bench --offline -p edgion-waf --bench xml_execution --bench xml_chunks --bench json_execution --no-run`
  then the frozen standalone `run_benchmarks.py`: all 12 serial runs exited 0.
  Common-valid XML 352 digests match old/final; each XML/JSON chunk direction
  contains 2080/96 passing rows. Final JSON owned controls show no consistent
  over-10% change in both directions; their semantics use unchanged harness
  assertions, not a cross-version digest.

Independent Astra final verdict: APPROVE, no unresolved Must Fix. Exact source,
locks, binaries, live report, raw measurements and review binding are retained in
`evidence/xml-inspection-2026-10-06/`. All workload regressions are disclosed in
[the results](../pending/waf-xml-checker/06-performance-results.md), including the
approximately 15-times controls/capture wide-attribute case. Capacity counters
do not establish allocation totals or RSS. XML subset delivery does not promote
the general classifier, establish full CRS/FTW or start response Phase 4. No commit
or push was authorized. Task 08 is the next planning scope.
