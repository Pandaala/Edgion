# WAF Current State and Baseline

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
