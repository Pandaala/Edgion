# Local connection-gate lookup ownership

Status: deferred ownership and contention decision for the user.
Source baseline: `9f7e56d8d`. No runtime or dependency change.

## Current evidence

`edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` binds
LocalGateBinding to a persistent entry during construction. Admission does not
construct an owned scope key or acquire the global registry mutex. EntryGates
looks up the listener port under an entry-local read lock, clones the bucket Arc,
and releases the map guard before entering LocalRateGate's state mutex. Misses
recheck under the write lock and publish one unactivated bucket.

Warm gate hits already avoid heap allocation. The Arc clone shares a bucket; it
does not copy its state. It lets a caller wait for one bucket without retaining
the port-map guard. Replacing that clone with a borrow through the read guard
would extend the guard across the bucket wait and could delay creation of another
port under the same entry. The existing
`blocked_bucket_does_not_hold_port_map_or_global_registry` unit test exercises
independent same-entry port and unrelated-entry progress while a bucket is held.

The registry intentionally retains budgets across runtime removal and recreation.
Its per-entry port dimension is bounded by u16, but entry churn is not globally
memory-bounded. This is not a disposable cache, and a runtime-local replacement
bucket would reset admission state. A full dense port table would also introduce
a different retained-memory cost for every entry, including sparse ones.

## Alternatives for a separate decision

- Keep the current sparse map and short-lived Arc ownership.
- Evaluate port-map sharding if current workloads demonstrate entry-local lock
  contention. Sharding still needs safe bucket ownership after releasing a shard
  guard; it does not itself eliminate reference-count work.
- Evaluate stable per-port lookup ownership only with an explicit lifetime,
  publication and memory design that releases lookup guards before bucket waits.
  No representation or new cache is selected here.

The original algorithm page records historical contention measurements with
mixed single-worker and multi-worker results. Those measurements are not a fresh
baseline and do not isolate Arc operations as a bottleneck. Removing a visible
clone alone does not establish a performance win.

## Acceptance for an approved implementation

Preserve entry-scope plus port identity, independent of Redis keyPrefix. Preserve
one retained budget per identity, including delete/recreate and disabled periods.
Keep first-use activation per port, inert unused generations, checked generation
reservation and fail-closed exhaustion. Old callers consume the latest activated
local parameters without restoring old settings; their Redis settings stay pinned.

Keep initialization, old-rate refill, generation adoption, capacity adjustment and
consumption one coherent transition. Preserve integer bounds, rounding and reversed
clock handling. Sample the production monotonic clock inside that transition.
No gate lock may span Redis, logging or metrics. Local admission remains before
Redis, and Redis denial does not refund local tokens. Layered attachments retain
their cumulative behavior; this task does not change listener scheduling.

Measure a current baseline and candidate under the same optimized build, machine
and policy inputs: one hot bucket, multiple ports in one entry, independent
entries, cold first-use misses, retained runtimes and configuration churn. Vary
worker count and separate admitted and exhausted paths. Report lookup/refcount
and lock evidence, warm throughput, first-use latency and retained bytes per
entry/port, including sparse configurations. Keep the current design if the
candidate lacks a meaningful benefit or merely shifts contention or memory cost.

Re-run the existing deterministic concurrency and generation tests, preserving
blocked-bucket isolation. Any new representation needs targeted publication and
first-use race coverage. Microbenchmarks do not replace hot-reload traffic tests
and do not establish a cross-process policy transaction. Preserve the canonical
registry and observability owners; add no per-connection tracing or diagnostic
payloads to justify the optimization.

## Original knowledge and verification limits

- [Local admission transitions](../../skills/04-review/rate-limit/regression-guards/local-connection-admission-transitions.md)
- [Entry identity and retained budgets](../../skills/04-review/rate-limit/regression-guards/connection-rate-limit-entry-scoped-buckets.md)
- [Token-bucket algorithm and historical measurements](../../skills/12-algorithms/local-connection-token-bucket.md)

Fresh connection-rate-limit unit verification passed 32 tests; the existing
contention measurement remains ignored. Formatting and workspace all-target
check/Clippy passed, with existing unrelated warnings. This validates the current
implementation, not any proposed replacement. No contention benchmark, live Redis
exchange or hot-reload traffic test ran for this audit.
