---
name: wasm-request-execution-lifecycle
description: Use when changing Wasm request bindings, callback frames, execution admission, attached response hooks, or timer-root lifetime.
knowledge_type: regression-guards
---

# Wasm request execution lifecycle

A configured plugin owns immutable configuration, publication, callout authority,
and explicitly shared host data. A request binding pins one loaded generation,
exclusive VM and HTTP context through its terminal hook. The instance owns guest
linear memory, root and HTTP objects. Process-wide engine/instance admission and
artifact bytes do not imply globally shared plugin state.

The sparse request owner uses opaque entry identity, not source hashes. It admits
at most 64 bindings and rejects a repeated installation of the same entry; it
does not silently merge multiple invocations. A short
lock transfers an execution loan; no guest call or network wait holds that lock.
Cancellation closes the owner and discards the VM, even if a loan returns later.
Normal logging terminal, including a healthy transport-error terminal, runs
synchronous done/delete once. A cancelled execution future or dropped request
owner never re-enters guest code. Deferred done is unsupported and discards.

Callback frames release snapshots, current body slices, pending operations,
callout responses, and collection capacity after output extraction. Closed idle
VMs retain no request payload. Guest memory is not scrubbed: warm instances may
retain their root state, and the guest remains responsible for its own memory.
Callout tokens are monotonic per VM with checked exhaustion. Any undelivered
token makes the VM ineligible for reuse, including early resume and round caps.

Per-runtime active admission (default 32, maximum 64) spans OCI generations.
The configurable process limit (default 128 actual instances) includes idle pools and dedicated
persistent timer VMs; the permit is released after Store destruction. This is
an instance-count bound, not a heap/RSS guarantee or fairness promise. Long
streams hold admission. Timer root fields survive successful ticks but are not
shared with request roots; only explicit host KV crosses those heaps.

Explicit request `response` hooks use the existing EntryBundle and stream/trailer
seams. Missing request bindings leave companions inert. Headers use actual EOS;
body observations preserve bytes flushed before trailers without inventing EOS.
Body and trailer frames are read-only; failure disables that binding and records
one bounded request diagnostic while the stream continues. No response buffering,
per-chunk PluginLog, or tracing is added. Standalone synchronous entries retain
independent per-hook contexts and do not share these bindings.

Both standalone and attached response-body callbacks use the same reduced
execution budget: `max(timeoutMs / 10, 10)` milliseconds, plus the existing epoch
interruption slack. The attached path selects this budget per callback without
changing the persistent lease budget for headers, request body or cleanup. The
Astra P2 review found that using the request lease's full timeout here enlarged
the blocking window for every chunk. The busy-loop regression
`attached_body_deadline_discards_binding_and_preserves_later_chunks` checks the
reduced deadline, discarded binding/VM, one diagnostic and unchanged subsequent
chunks. It exercises the observer seam, not end-to-end transport delivery.

Evidence lives in the real Rust SDK `lifecycle-demo` fixture and gateway tests
`attached_test.rs`, `request_test.rs`, `vm_manager_test.rs`, and `host_abi_test.rs`.
Re-evaluate when introducing multiple concurrent HTTP contexts per instance,
response mutations, deferred completion, or shared timer/request root semantics.


## Aggregate admission and retired pools

Task 03 implementation uses restart-only `gateway.wasm_limits.guest_memory_bytes`
(default 512 MiB) for aggregate logical linear-memory/table-slot allocations.
The limiter reserves before growth and rolls back only the current failed growth.
An external charge guard outlives Store destruction, including initialization
errors; the same budget spans independent configured runtimes and OCI generations.
Shared memories are disabled because Wasmtime does not invoke this limiter for
them. The Store memory/table/instance counts match the single-memory/table
pooling profile. This is not an RSS, GC heap, compiler-heap or host-payload budget.

Configurable preparation slots (default four) cover all source types before decoding/fetch.
Blocking work and undelivered results retain admission after caller cancellation.
OCI artifact ownership/permits continue through compilation independently.
Runtime destruction and OCI replacement serialize retirement with pool returns,
drain idle Stores and reject late repooling. Gauge contributions clear without
reattaching an expired metric epoch. Active old-generation executions stay valid.

Callout deadlines fold the existing absolute route/gRPC bound into every network
wait without restarting it between calls; expiry poisons pending guest tokens
before any late callback. Tick calls have no request deadline. Callout response
header snapshots validate count/byte bounds before owned copies. Neither change
adds a response aggregate, a short SSE lifetime nor a new failure-policy term.

Regression sources: `admission_test.rs`, `vm_manager_test.rs`, `callout_test.rs`
and the existing request/attached SDK lifecycle tests under the Gateway unit tree.

## Configurable limits and idle retention

`gateway.wasm_limits` atomically publishes all process capacities before use.
Unknown local Gateway keys and invalid limits fail startup. Per-instance guest
and host ABI allocation profiles can only tighten their reviewed ceilings. The
Engine pooling totals match max_instances; its table ceiling is now explicitly
20,000 (the old effective default), rather than advertising the formerly looser
100,000 host-only limiter. See the configuration reference for exact ranges.

A healthy return first releases callback payloads and then reserves both global
idle count and exact charged guest bytes. Failure destroys the Store; there is no
queue or global pool registry. The idle guard follows Store and guest destruction
in field order, while checkout releases it immediately. Retirement and late
returns preserve the existing no-repooling rule. Idle defaults are 32 instances
and 128 MiB; either zero disables pooling. This bounds retained idle heaps without
resetting active/timer roots or claiming fairness or total host/RSS accounting.

External unit tests cover strict configuration, failed initialization followed by
valid publication, concurrent idle reservations/rollback, custom real-guest growth
limits, host ABI tightening and cross-entry checkout/retirement conservation.

## Execution-model selection and measurement

The 2026-10-14/15 investigations retained warm exclusive request leases as the
implemented baseline, without selecting a performance winner. Fresh instances
can reuse compiled code and Wasmtime allocation slots while repeating guest/root
initialization. Warm leases save that initialization but retain guest memory
high watermarks and nondeterministically assigned root state. Host-frame release
does not scrub guest memory or establish a generic reset contract. Choose fresh
execution when fresh guest state is required; never zero a live heap to imitate it.

Both exclusive models hold one instance throughout slow uploads, callouts and
SSE. Warm pooling cannot remove active stream occupancy. Compare equivalent
callback/terminal semantics under the same grants, limits, guest bytes and
allocator before selecting a model. Separate compilation, initialization, pool
miss and warm acquisition; include expensive initialization, bodies, callouts,
long streams, sparse traffic and overlapping generations. Measure successful and
offered load, rejections, CPU, latency quantiles, guest logical/resident bytes,
host live/retained heap, virtual reservations and RSS together. Use repeated
independent runs and workload SLOs; instance counts or a local microbenchmark
alone cannot establish a winner. The [benchmark guide](../../../05-testing/references/wasm-instance-reuse-benchmark.md)
records the executable comparison and its narrower evidence.

A complete resident-memory ledger remains outside the implemented guest budget.
It must include Engine/code, artifact retention, compiler jobs/results, shared
data/configuration, every initializing/active/idle/timer Store, host snapshots,
ABI scratch, request staging, callouts and detached outputs across retiring and
published generations. Count shared allocations once and distinct copies
separately; retain charges until their last owner releases them. Guest allocation
admission and idle caps do not establish compiler/host/RSS bounds or fairness.

Multi-context execution and response callout ABI extensions remain unselected.
A shared Store requires one stable logical serial owner, independent of Tokio
thread identity. A suspended mutable Store borrow cannot serve another context.
Network I/O must occur outside guest entry, with generation/incarnation/context/
phase/token-bound completions and no late re-entry. A future design needs separate
context, queue-count/byte, current-event, callout, output and CPU bounds, fair
ready-context scheduling and cancellation/control progress even at saturation.
A trap or unsafe context cancellation may invalidate every context in that Store;
reinstantiation cannot restore their guest objects. Same-root ticks would change
the current dedicated-timer contract and require separate review.

The synchronous response-body filter cannot wait or act as a hidden continuation.
Any selected asynchronous current-event decision must use the canonical awaited
processor, preserve backpressure, and define Pause/resume, deadline, committed
failure and terminal ordering explicitly. Host SSE semantics reuse canonical
decoder/window owners. Real H1/H2 proof covers current observer/gate behavior,
not response-callout support, shared-VM fairness or production capacity; see the
[cancellation contract](../../plugin-runtime/regression-guards/response-callback-downstream-cancellation.md).
Reopen model selection only for a concrete workload that misses its resource or
latency target, with an independently reviewed design and equivalent measurements.
