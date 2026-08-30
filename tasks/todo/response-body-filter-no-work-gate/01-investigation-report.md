# Async response-body no-work gate investigation

## Executive conclusion

**Decision: do not implement the public `ProxyHttp` predicate.**

The allocation mechanism is real. A release build of the current Pingora trait
default, invoked through `ProxyHttp`, requests one 16-byte allocation from the
same jemalloc family used by the gateway for every hook call. A release
microbenchmark that mirrors the current Edgion hook shape requests one outer
allocation per call and one additional allocation per awaited
`StreamingResponseProcessor`. A conservative gate eliminates the outer
allocation on a proven-idle later chunk.

The missing evidence is workload prevalence, not mechanism. Under this task's
explicit implementation criteria, missing representative evidence is a no-go,
not a reason to add a speculative public API. This repository
has no production-representative response chunk-count distribution and no data
showing what fraction of chunks are idle after TTFT, streaming processors, and
all effective route, backend, Gateway, and global body-filter scopes are
considered. The isolated benchmark shows the
expected large win when 98 of 100 synthetic chunks are deliberately idle, but
it is not a wire-level gateway benchmark and cannot establish a material
product benefit. On paths that must continue invoking the hook, the extra
predicate was within roughly a few percent of benchmark noise, with a small
apparent regression in the synthetic bandwidth-filter case.

Retain the current awaitable-pipeline cost. Re-evaluate only under one of the
triggers listed at the end of this report, such as an offline production trace,
a representative load-test profile, or allocator sampling that independently
identifies this allocation as material. Do not add permanent per-chunk metrics
or tracing merely to answer this question.

## Baseline and scope

The investigation read the working trees at:

- Edgion commit `00184215e729d88aa7eb4b44e6db1478cb597e50`
- Pingora commit `63bcfccdf1fc0b2d1e82f6c7c7282006b8f384ec`

Both trees contained unrelated uncommitted work, including active
response-pipeline changes in Pingora. The lifecycle conclusions below describe
the working-tree code, not only the named commits. No runtime source file was
modified for this investigation. The benchmark harness lived under ignored
`target/response-body-no-work-bench/` and is not a shipped artifact.

## Verified allocation model

`ProxyHttp` is transformed by `async_trait`. Calling an async method creates a
`Pin<Box<dyn Future + Send>>`; the allocation happens when the hook is invoked,
before it is polled. An immediately-ready default body therefore still pays for
the box.

### Allocation measurements

The harness wrapped `tikv_jemallocator::Jemalloc` in a counting
`GlobalAlloc`, then polled each immediately-ready future once. Counts are
allocation requests and requested bytes, not jemalloc size classes or resident
memory.

| Case | Allocations per call | Requested bytes | Evidence level |
|------|---------------------:|----------------:|----------------|
| Current Pingora default hook | 1 | 16 | Actual `pingora_proxy::ProxyHttp` default |
| Edgion-shaped first chunk with TTFT | 1 | 80 | Simplified isolated model, not the linked gateway binary |
| Edgion-shaped idle later chunk | 1 | 80 | Simplified isolated model |
| Edgion-shaped chunk with one async processor | 2 | 112 | Outer 80 bytes + nested processor 32 bytes |
| Gated idle later chunk | 0 | 0 | Simplified isolated model |
| Conservatively gated terminal event | 1 | 80 | Gate intentionally returns true |

The exact 16-byte Pingora default result was measured against the current
sibling `pingora-proxy` source. The 80-byte Edgion-shaped outer future and
32-byte nested future are compiler-layout results from a simplified harness.
The harness preserves the async-trait nesting and relevant gate branches, but
does not reproduce the real `run_stream_processors` borrow/move structure or
the complete gateway hook. These values must not be presented as exact sizes
from the final linked `edgion-gateway` binary; that requires an in-tree
production-profile harness or allocator sampling of the real binary.

For a synthetic 100-chunk ordinary response, the modeled allocation totals
were:

| Path | Allocations/request | Requested bytes/request |
|------|--------------------:|------------------------:|
| Always invoke | 100 | 8,000 |
| Conservative gate | 2 | 160 |
| Bandwidth filter present | 100 | 8,000 |
| One async processor present | 200 | 11,200 |

The gated ordinary response invokes the hook for the first non-empty chunk
(TTFT) and the final EOS chunk. Keeping EOS unconditional is deliberate. A
future implementation might prove some terminal events skippable, but that is
not required for the safe initial optimization.

### Active-request memory

The box is transient and lives only while the hook future is in flight; it is
not retained for the lifetime of the request. As an illustrative upper bound,
10,000 simultaneously suspended default hooks account for 160,000 requested
bytes, while the Edgion-shaped outer futures account for 800,000 requested
bytes, plus approximately 320,000 requested bytes per simultaneously suspended
nested processor layer. This is a layout-derived bound, not measured process
RSS. No representative concurrency profile was available.

## Call path and lifecycle audit

The shared Pingora dispatch is:

```text
H1 / H2 / custom response pump
  -> HttpProxy::upstream_filter
     -> Header: upstream_response_filter
     -> Body or UpgradedBody: upstream_response_body_filter
     -> Trailer: upstream_response_trailer_filter
  -> final non-informational Header(..., true)
     -> response_filter
     -> terminal_upstream_body_filter(None, true)
```

The Edgion hook performs, in order:

1. first-non-empty-body time recording;
2. `ai_stream_processor` and `stream_processors`, with an async-trait future for
   each processor actually called;
3. HTTPRoute rule-level synchronous response-body filters;
4. resolved Gateway-level synchronous response-body filters;
5. resolved GatewayClass-global synchronous response-body filters.

The current body hook does **not** dispatch the matched GRPCRoute rule runtime
or the selected HTTP/GRPC BackendRef runtime, even though those runtimes can
contain `ExtensionRefFilter`, which implements `UpstreamResponseBodyFilter`.
The sibling sync and async response-header stages do dispatch those scopes.
This is an adjacent correctness finding, recorded separately in
`tasks/todo/route-backend-response-body-filter-dispatch-gaps.md`; it is not an
optimization assumption and is not fixed by this investigation.

Important lifecycle findings:

- **Header EOS:** the body hook runs after `response_filter` with
  `body = None, end_of_stream = true`. Processors may synthesize data or
  terminate. A first implementation should always invoke this event.
- **Ordinary body:** every upstream `Body` task invokes the hook, including the
  final body task. Empty non-terminal H2 reads are discarded before task
  creation, so they do not create arbitrary extra hook calls.
- **Upgraded body:** `UpgradedBody` uses the same hook but does not receive the
  synthetic terminal call. Conservatively keep all upgraded calls; the
  predicate must distinguish upgraded state through `Session` or a typed event.
- **Cache:** `from_cache` paths intentionally bypass `upstream_filter`; cached
  body tasks do not invoke this upstream hook. A gate changes only live origin
  processing and cache-admission passes.
- **Sink termination:** termination is produced inside the hook. The H1, H2,
  and custom pumps stop before a later invocation, so a false predicate does
  not need to inspect stale sink output. The gate must still run whenever a
  processor capable of using the sink is installed.
- **Local replies:** Edgion response-stage local replies are written directly
  by `response_filter`, which returns an abort sentinel. Phase 8 is never
  reached, so `local_reply_body` is not an independent reason to call the body
  hook.
- **Retries:** TTFT is stored on the current upstream-attempt record. The gate
  must query the current attempt rather than use a request-global boolean.
- **Hot reload:** route, Gateway, and global plugin runtimes are immutable
  request-local snapshots before response processing. Inspecting their body
  filter counts requires no store lookup or lock and remains stable for that
  request.
- **Configuration resolution:** global and Gateway runtimes are resolved during
  the request stage. The per-chunk decision must never repeat this resolution.
- **Trailers:** the upstream body hook is not the trailer hook. A trailer may be
  converted to a downstream body buffer later, but that does not create another
  upstream body-filter event.

The audited seam and sink tests already exercise H1, H2, custom connectors,
header-only terminal responses, emitted chunks, termination, cache admission,
cache readback, downstream body suppression, and post-terminal failures. They
are suitable locations for a later predicate regression matrix.

## Candidate conservative predicate

The Pingora default must return `true`, preserving behavior for every existing
implementation:

```rust
fn should_run_upstream_response_body_filter(
    &self,
    _session: &Session,
    _body: &Option<Bytes>,
    _end_of_stream: bool,
    _ctx: &Self::CTX,
) -> bool {
    true
}
```

An Edgion override may return `false` only when all of these facts are proven:

```text
not end_of_stream
and not an upgraded response-body event
and no first-body timestamp is still required for this upstream attempt
and ai_stream_processor is absent
and stream_processors is empty
and both possible route runtimes have zero upstream-response-body filters
and both possible selected backend runtimes have zero such filters
and every resolved Gateway runtime has zero such filters
and every resolved global runtime has zero such filters
```

If any state is missing, unresolved, or inconsistent, return `true`.

### Safety of each false result

| Condition | Why false is safe only when it holds |
|-----------|--------------------------------------|
| Non-terminal | EOS processors may flush buffered frames, inject output, or finalize state. |
| Non-upgraded | Upgraded streams have different terminal semantics; the initial change need not optimize them. |
| TTFT already satisfied or inapplicable | Skipping the first non-empty chunk would suppress `bt`. |
| No async processors | Every installed processor must see every chunk regardless of stream classification. |
| No HTTP/GRPC route filters | ExtensionRef wrappers may resolve to bandwidth filters. Current dispatch omits GRPCRoute and must be fixed separately before a gate relies on this scope. |
| No HTTP/GRPC selected-backend filters | BackendRef ExtensionRefs can carry the same body-stage work. Current dispatch omits both backend scopes and must be fixed separately. |
| No Gateway filters | Gateway annotations are a separate response-onion scope. |
| No global filters | GatewayClass-global filters are another independent scope. |

All checks can be allocation-free and lock-free against current request-local
state. Directly scanning the two resolved runtime vectors is complete but adds
per-chunk branches. A precomputed `has_sync_response_body_filters` bit is
cheaper, but it may be set only after all request-stage plugin resolution is
complete and must remain tied to the same immutable snapshots. Live processor
slots and per-attempt TTFT state still need direct checks.

The proposed Pingora signature cannot directly distinguish `Body` from
`UpgradedBody`. Before implementation, either prove that `session.was_upgraded()`
is correct for H1 and custom dispatch at this exact boundary or pass a typed
body-event kind into the predicate. Silently treating upgraded chunks as
ordinary idle chunks is not acceptable.

## Release microbenchmark

### Method

- Host: Apple M4, 10 logical CPUs, 16 GiB RAM, macOS 15.5.
- Toolchain: `rustc 1.96.1`, Cargo 1.96.1, LLVM 22.1.2.
- Macro: `async-trait 0.1.92`.
- Allocator: `tikv-jemallocator 0.6.1`, jemalloc
  `5.3.0-1-ge13ca993e8ccb9ba9847cc330696e02839f328f7`.
- Build: release optimization, thin LTO, one codegen unit.
- Timing: 41 batches of 20,000 synthetic requests per case; the first batch was
  discarded. The complete experiment was executed five times. Each future was
  polled directly with a no-op waker, avoiding executor and network costs.
- Noise controls: release build, warm-up batch, batched timing, five process
  executions. CPU affinity and frequency locking were unavailable on this
  host.

The following table reports the median of each percentile across the five
executions. Each p50/p95/p99 is a percentile across 40 **batch-average**
nanoseconds-per-request observations, not a percentile of individual request
latencies and not end-to-end proxy tail latency.

| Scenario | Gate | p50 batch avg ns | p95 batch avg ns | p99 batch avg ns | Requests/s from p50 |
|----------|------|-----------------:|-----------------:|-----------------:|--------------------:|
| Ordinary, one chunk including TTFT | no | 19.50 | 20.17 | 20.54 | 51.28 M |
| Ordinary, one chunk including TTFT | yes | 19.35 | 20.00 | 20.76 | 51.68 M |
| Ordinary, 100 chunks | no | 1,063.93 | 1,090.82 | 1,098.61 | 0.940 M |
| Ordinary, 100 chunks | yes | 99.82 | 101.24 | 102.74 | 10.018 M |
| One sync bandwidth filter, 100 chunks | no | 1,066.64 | 1,095.70 | 1,097.55 | 0.938 M |
| One sync bandwidth filter, 100 chunks | yes | 1,089.79 | 1,114.74 | 1,119.91 | 0.918 M |
| One async processor, 100 chunks | no | 1,856.56 | 1,903.48 | 1,939.74 | 0.539 M |
| One async processor, 100 chunks | yes | 1,842.28 | 1,915.56 | 1,947.96 | 0.543 M |

Interpretation:

- The one-chunk case showed no meaningful regression, but the operation is too
  small for this harness to resolve a reliable sub-nanosecond branch cost.
- The 100-chunk idle case removes 98 of 100 outer calls and is therefore much
  faster by construction. This validates the mechanism, not workload value.
- The bandwidth-filter path cannot skip. Its median p50 was about 2.2% slower
  with the predicate in this run summary, while individual executions varied;
  this is a warning that the branch is not free.
- The async-processor path also cannot skip. The outer allocation remains and
  each processor retains its nested allocation. Results were within run-to-run
  variation.
- CPU cycles were not available on this macOS host. Nanoseconds are reported
  instead.

### Missing production evidence

No source, test result, or existing metric provides a production-representative
distribution for small HTTP, large downloads, SSE, gRPC streaming, or AI
streaming. Test fixtures demonstrate shapes ranging from one/few chunks through
more than 100 chunks and byte-at-a-time SSE, but fixture counts are deliberate
edge cases, not traffic samples.

The fraction of idle chunks is also unknown. It depends on route mix, the
prevalence of `BandwidthLimit`, Gateway and global plugin attachments, AI usage
processors, translations, and response size. Without both distributions, an
allocation saved per eligible chunk cannot be converted into gateway CPU,
throughput, p99 latency, or active-memory impact.

## API compatibility and maintenance assessment

A new synchronous trait method with a default of `true` is source compatible
for existing `ProxyHttp` implementations and preserves behavior by default.
It does not remove the existing async method or require downstream implementors
to change. Edgion builds only against its fork, so upstream crate-version
compatibility is controlled.

The maintenance cost is still non-zero:

- the public trait must document event ordering and the false-negative safety
  contract;
- both ordinary/upgraded dispatch and synthetic Header-EOS dispatch must honor
  the predicate consistently;
- every future source of response-body work must update the Edgion decision;
- duplicated knowledge between the decision and the real dispatch can drift;
- seam tests must prove that TTFT, throttling, mutation, injection,
  termination, and EOS are never suppressed.

An internal-only gate avoids a public method but cannot skip creation of the
outer async-trait future if placed inside Edgion's async hook. To remove that
allocation, the decision must occur in Pingora before calling the hook, which
requires either a public trait seam or a fork-internal dispatch abstraction
that can query application state without depending on Edgion types. The public
defaulted predicate is the cleaner of those two designs if profiling later
justifies it.

## Decision criteria assessment

| Criterion | Result |
|-----------|--------|
| Conservative across lifecycle | Plausible but not yet regression-tested; upgraded-event identification needs resolution. |
| Allocation-free, lock-free predicate | Achievable against request-local snapshots. |
| Removes material allocations in representative workload | **Unknown; production idle-chunk fraction is missing.** |
| Repeatable release benefit above noise | Proven only in the synthetic 100-chunk idle model, not a real gateway workload. |
| API burden proportionate to gain | Cannot decide until workload benefit is known. |
| Tests distinguish required behavior | Existing seams are suitable, but predicate-specific tests do not exist. |

The implementation threshold is therefore not met.

## Exact implementation scope if profiling justifies it

Create a separate implementation task limited to:

1. Pingora `ProxyHttp`: add the default-true synchronous predicate and its
   precise safety documentation.
2. Pingora shared dispatch: consult it before ordinary body calls and terminal
   synthetic calls; preserve default behavior and delay semantics.
3. First close `route-backend-response-body-filter-dispatch-gaps`, then have
   the Edgion request context represent HTTPRoute, GRPCRoute, HTTP BackendRef,
   and gRPC BackendRef body-filter presence. Add only the minimum precomputed
   bit if direct snapshot inspection is measurably too expensive.
4. Edgion proxy: implement the conservative live checks for TTFT, processors,
   immutable plugin snapshots, EOS, and upgrades.
5. Tests: cover default compatibility, first-body TTFT, later idle skip,
   bandwidth filtering, every streaming-processor slot, injected chunks,
   termination, ordinary EOS, Header EOS, H1/H2/custom pumps, cache admission,
   cache hits, retries, upgrades, HTTPRoute and GRPCRoute rules, both selected
   backend types, and hot-reload snapshot stability.
6. Benchmark: run the real gateway binary or an in-tree production-profile
   harness with the shipped allocator and representative response fixtures.

Do not combine this work with RPITIT conversion, a split sync/async body
pipeline, future caching, unsafe storage, arenas, chunk coalescing, or unrelated
response-pipeline refactoring.

## Re-evaluation triggers

Re-open the implementation decision when at least one of these is available:

- offline production traces provide response chunk counts and enough
  request-local classification to estimate the idle fraction without adding
  permanent per-chunk telemetry;
- a representative load test shows that no-work outer-hook allocations are a
  material allocator or CPU hotspot in the production-profile gateway binary;
- allocator sampling attributes a meaningful fraction of gateway allocations
  to `ProxyHttp::upstream_response_body_filter` rather than nested processors;
- an internal Pingora dispatch change makes the gate substantially cheaper or
  removes the public-API burden;
- a compiler or `async_trait` change removes the outer box, making the task
  obsolete.

Until then, retain the boxed future as the measured but not yet justified cost
of Edgion's awaitable response-body pipeline.
