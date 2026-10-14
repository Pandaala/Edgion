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
The process limit of 128 actual instances includes idle pools and dedicated
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

Task 03 implementation adds restart-only `gateway.wasm_guest_memory_bytes`
(default 512 MiB) for aggregate logical linear-memory/table-slot allocations.
The limiter reserves before growth and rolls back only the current failed growth.
An external charge guard outlives Store destruction, including initialization
errors; the same budget spans independent configured runtimes and OCI generations.
Shared memories are disabled because Wasmtime does not invoke this limiter for
them. The Store memory/table/instance counts match the single-memory/table
pooling profile. This is not an RSS, GC heap, compiler-heap or host-payload budget.

Four immediate preparation slots cover all source types before decoding/fetch.
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
