# Investigate a no-work gate for the async response-body filter

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-11 |
| Status | blocked |
| Profile | investigation |
| Priority | P3 |
| Source finding | Pingora review issue 23: `pingora/tasks/pingora-review/23-async-per-chunk-boxed-future.md` |

## Investigation result

Investigation work completed on 2026-08-11 with a **do not implement** decision;
the measured cost does not justify a speculative public API without
production-representative evidence. Formal task closure remains blocked by the
repository's currently failing Pingora source-policy guard. See
[`01-investigation-report.md`](01-investigation-report.md).

The boxed-future mechanism is confirmed, but the public no-work predicate is
declined. The current decision is to retain the awaitable pipeline cost and
re-evaluate only when the report's re-evaluation triggers are met.

## AI guide summary

Investigate whether a conservative synchronous predicate, evaluated before
Pingora calls `ProxyHttp::upstream_response_body_filter`, can avoid the
`async_trait` boxed-future allocation on Edgion response chunks that have no
body-filter work. The investigation must measure the real allocation and
throughput impact, define a predicate that cannot suppress required behavior,
and assess the public-API and maintenance cost. It must not implement the
optimization. The intended outcome is an evidence-backed go/no-go decision and,
only if justified, a separate implementation task.

## Background

The Edgion Pingora fork changed `ProxyHttp::upstream_response_body_filter` from
synchronous to asynchronous so Edgion's writable streaming-response processor
pipeline can await per-chunk processors. Because `ProxyHttp` uses
`#[async_trait]`, invoking the hook creates a boxed future for every upstream
response-body chunk, including requests for which the hook has no meaningful
work.

The current Edgion call chain is:

```text
Pingora response-body pump
  -> ProxyHttp::upstream_response_body_filter (async_trait boxed future)
  -> Edgion run_stream_processors (async)
  -> StreamingResponseProcessor::on_chunk (async_trait boxed future per processor)
  -> synchronous UpstreamResponseBodyFilter chains
```

The Edgion hook also records time-to-first-body, so an otherwise idle request
may still require its first non-empty chunk to enter the hook. After that value
is recorded, later chunks with no installed processors or synchronous body
filters may have no work.

The proposed direction is a conservative predicate whose default preserves
current runtime behavior. Whether adding it is source- and ecosystem-compatible
is part of the investigation, not an assumption. Conceptually:

```rust
fn should_run_upstream_response_body_filter(
    &self,
    session: &Session,
    body: &Option<Bytes>,
    end_of_stream: bool,
    ctx: &Self::CTX,
) -> bool {
    true
}
```

Pingora would call the async hook only when this predicate returns `true`.
Edgion could override it using request-local state. A false negative is a
correctness bug; uncertain states must therefore return `true`.

## Investigation questions

1. How many heap allocations and allocated bytes does one invocation of the
   default and Edgion implementations create with the actual allocator and
   release build settings?
2. What is the CPU, throughput, and tail-latency difference between always
   invoking the async hook and skipping a proven no-work invocation?
3. What are the observed chunk-count distributions for representative small
   HTTP responses, large downloads, SSE, gRPC streaming, and AI streaming?
4. What fraction of production-representative Edgion chunks have no work after
   accounting for TTFT recording, writable processors, and every Route,
   Gateway, and GatewayClass-global body-filter chain?
5. Can the predicate be computed without allocation, locking, logging, store
   lookup, or configuration resolution on the per-chunk path?
6. Can the predicate remain conservative during route snapshots, retries,
   terminal header-EOS callbacks, sink termination, local replies, cache paths,
   upgraded bodies, and configuration hot reload?
7. Does one extra branch and the required context inspection cost more than it
   saves for one-chunk responses?
8. Is a public `ProxyHttp` predicate acceptable for the fork, or can the same
   conservative decision live in an existing internal dispatch abstraction
   without coupling Pingora to Edgion context internals?
9. How much allocation remains in the nested
   `StreamingResponseProcessor::on_chunk` async-trait chain when the outer hook
   cannot be skipped? The investigation must report this separately rather
   than attributing all per-chunk allocations to the Pingora hook.

## Required call-path audit

Read and trace at least:

- `../pingora/pingora-proxy/src/proxy_trait.rs`
- `../pingora/pingora-proxy/src/lib.rs`
- `../pingora/pingora-proxy/src/proxy_h1.rs`
- `../pingora/pingora-proxy/src/proxy_h2.rs`
- `../pingora/pingora-proxy/src/proxy_custom.rs`
- `../pingora/pingora-proxy/tests/test_upstream_response_body_sink.rs`
- Relevant seam scenarios under `../pingora/pingora-proxy/tests/seam/`
- `edgion-gateway/src/routes/http/proxy_http/mod.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- `edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs`
- `edgion-gateway/src/plugins/runtime/traits/streaming_response_processor.rs`
- `edgion-gateway/src/ctx.rs`
- AI streaming-processor implementations under `edgion-gateway/src/ai/`

The Pingora repository is a sibling worktree at `../pingora` when this task is
run from the Edgion repository root. Resolve paths against the actual workspace
rather than assuming both projects share one Cargo workspace.

## Candidate predicate inputs to validate

The investigation should determine whether all of the following can be checked
cheaply and completely. This list is a hypothesis, not an approved design:

- A first non-empty upstream body chunk still needs TTFT recording.
- `ctx.ai_stream_processor` is present.
- `ctx.stream_processors` is non-empty.
- The route-level runtime has upstream response-body filters.
- Any resolved Gateway-level runtime has upstream response-body filters.
- Any resolved GatewayClass-global runtime has upstream response-body filters.
- A terminal or synthetic event has required semantics independent of the
  processor/filter lists.
- Sink or local-reply state requires the hook even if plugin lists are empty.

If any required condition cannot be observed without expensive work or without
duplicating fragile dispatch knowledge, recommend against the optimization.

## Measurement plan

Use a release-mode benchmark or focused harness. Debug-build timings are not a
decision signal. At minimum measure:

| Scenario | Required comparison |
|----------|---------------------|
| Default `ProxyHttp`, one body chunk | Always-call baseline versus no-work skip |
| Edgion ordinary response, one chunk | Include TTFT work |
| Edgion ordinary response, 100+ chunks | First-chunk work followed by idle chunks |
| Synchronous bandwidth body filter | Predicate must continue invoking every chunk |
| AI/SSE processor chain | Predicate must continue invoking; report nested allocations separately |
| Header-only / terminal Header-EOS | Verify exact callback behavior and allocations |

Report:

- Allocations and allocated bytes per request and per chunk.
- CPU time or cycles per chunk.
- Throughput.
- p50, p95, and p99 latency where the harness can measure them reliably.
- Active-request memory at representative concurrency.
- Allocator and toolchain versions.
- Noise controls, number of repetitions, and confidence or observed variance.

Do not add permanent metrics or per-chunk tracing to collect this evidence.

## Decision criteria

Recommend implementation only when all of the following hold:

1. The predicate is demonstrably conservative across the audited lifecycle.
2. Its steady-state evaluation is allocation-free and does not perform locks,
   logging, store lookups, or configuration resolution.
3. It removes a material fraction of allocations in at least one
   production-representative workload.
4. The release-mode benchmark shows a repeatable benefit larger than benchmark
   noise, with no meaningful regression for one-chunk responses.
5. The public-API and documentation burden is proportionate to the measured
   gain.
6. Tests can distinguish a correct skip from accidentally suppressing TTFT,
   bandwidth throttling, stream mutation, injected chunks, termination, or EOS
   behavior.

If these criteria are not met, record the boxed future as an accepted cost of
the awaitable response-body pipeline and do not create an implementation task.

## Deliverable

Add an English investigation report in this directory named
`01-investigation-report.md` containing:

- Executive conclusion: implement / do not implement / blocked on production
  profiling.
- Verified allocation model, including nested processor allocations.
- Call-path and lifecycle findings.
- Benchmark method and raw summary tables.
- Proposed predicate and proof that each false result is safe, if recommending
  implementation.
- API compatibility and maintenance assessment.
- Exact implementation scope and regression-test plan, if recommending
  implementation.
- Re-evaluation triggers if recommending against implementation.

Do not modify Pingora or Edgion runtime code as part of this investigation.

## Scope

### In scope

- Read-only analysis of the Pingora and Edgion response-body paths.
- Temporary or benchmark-only measurement harnesses that are not shipped as
  runtime changes.
- A conservative no-work predicate before the async Pingora hook.
- Quantifying outer-hook and nested-processor async-trait allocations.
- Public API, lifecycle, testability, and maintenance analysis.

### Out of scope

- Caching or storing a future/boxed-future pointer in `EdgionHttpContext`.
- Changing the hook to explicit RPITIT / `impl Future`.
- Reverting the Pingora hook or `StreamingResponseProcessor` to synchronous.
- Adding separate general-purpose synchronous and asynchronous body-filter
  pipelines.
- Custom allocators, unsafe reusable future storage, arenas, or future pools.
- Coalescing transport chunks or changing chunk boundaries.
- Runtime implementation of the predicate.
- Unrelated response-body, AI, plugin, or allocator refactoring.

## Affected modules if later approved

| Module | Potential impact |
|--------|------------------|
| Pingora `pingora-proxy` | Conservative dispatch predicate and lifecycle tests |
| Edgion HTTP proxy | Request-local no-work decision |
| Edgion plugin runtime | Read-only inspection of body-filter presence |
| Edgion AI streaming | Must remain behaviorally unchanged; allocation baseline only |

## Source finding disposition

This investigation supersedes Pingora review issue 23. The finding established
that the per-chunk boxed future exists, but did not establish that its measured
cost justifies a public API optimization. This task owns that decision. The
original review finding can be closed without a runtime change once this task
is linked from the review index or closure record and all required repository
checks pass.
