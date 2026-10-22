# Wasm guest allocator preparation

Status: deferred preparation and ownership decision. No runtime change.
Source baseline: 7e463a22f; locked Wasmtime version: 49.0.2.

## Source evidence

`edgion-gateway/src/plugins/http/wasm/host_abi.rs::write_to_guest` looks up
`proxy_on_memory_allocate` and checks its `(i32) -> i32` signature on every
successful host-to-guest return through this helper. Header maps and values,
buffer ranges, properties, legacy configuration and shared-data reads use it.
`malloc` is considered only when the primary export is absent: a non-function
or incorrectly typed primary export does not fall back to a valid `malloc`.
Missing/invalid allocators and allocator traps return INTERNAL_FAILURE.

The allocator is guest execution, not a host allocation primitive. It can
reenter imports, mutate source storage and grow guest memory. The helper obtains
memory after that call, then writes the payload, returned pointer and returned
size in order. Invalid memory returns INVALID_MEMORY_ACCESS; earlier writes may
already have succeeded. Successful empty/zero-size reads still call the allocator.

`callbacks.rs::GuestCallbacks` prepares other typed exports once, but its owner
is `PooledVm`, outside HostState. In `vm_manager.rs::new_vm`, callback preparation
follows instantiation, optional initialization/start exports and root lifecycle
callbacks. A Wasm start function can execute during instantiation. Imports may
therefore run before the existing callback table is available.

A HostState cache cannot simply lend its typed handle across a mutable Caller
guest call. The locked Wasmtime `TypedFunc` contains a `FuncType` and its Clone
implementation clones that type while copying the Func handle. Any proposed
ownership arrangement must account for per-call handle cloning and retained type
state; caching is not evidence of allocation-free access. Repeated lookup and
signature preparation are source-evident, but their allocation cost and share
of callback latency have not been measured.

## Decision alternatives

- Retain the current implementation unless measurement justifies preparation.
- Design instance/Store-owned preparation with explicit early-initialization
  handling and reentry-safe access. Preserve lazy failure timing: guests that
  never use these accessors must not acquire a new startup rejection.
- Consider an upstream-supported access pattern only after checking its actual
  ownership and lifetime guarantees against the locked dependency.

Do not introduce a global allocator-handle cache, unchecked typed calls, a new
configuration switch or a second guest execution owner as an incidental cleanup.
Account for initializing, active, idle and timer Stores, generation retirement
and destruction. A handle must never cross instance/Store identity. Quantify any
new retained state without relaxing existing admission or claiming that guest
memory limits cover host heap.

Measure export lookup, signature preparation, typed-handle cloning, guest
allocation and memory copies separately, then compare complete callback latency.
Include initialization and request/response accessor mixes with zero, small and
large returns. Preserve equivalent fuel, epoch, memory and concurrency limits.
No throughput improvement or bottleneck has been established.

## Acceptance for a selected implementation

Preserve primary-export precedence, lazy errors, allocator traps and execution
budgets, zero-size invocation, post-allocation memory lookup and partial-write
ordering. Keep independent pre-allocation snapshots: no HostState source borrow
may survive guest reentry. This task does not authorize changes to payload
snapshot ownership, body handling, shared-data policy or pool discard ordering.

Real-guest regressions must cover absent primary with valid fallback, invalid
primary export/signature despite valid fallback, allocator traps, memory growth,
reentrant GET/SET, initialization imports, zero-length reads and invalid return
pointers including partial writes. Verify instance isolation, warm reuse,
retirement, cancellation and early initialization failure for any cached owner.
Existing host ABI and VM lifecycle tests are starting coverage, not proof that
an unimplemented cache is safe.

Any selected implementation needs a frozen scope, IMPL plan, independent reviews
and project checks. No prototype, runtime change or performance claim is included.

Relevant original knowledge:
- skills/04-review/wasm/regression-guards/buffer-range-snapshot-before-guest-allocation.md
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/cpu-memory/regression-guards/wasm-abi-retention-budget.md
