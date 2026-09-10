---
name: local-connection-token-bucket
description: Local connection admission math, integer bounds, generation migration, ownership and measured contention.
status: shipped
---

# Local connection token bucket

## What we run today

`edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` maintains
a process-local budget per `<namespace>/<name>/#<index>` and listener port.
`LocalGateBinding` binds the entry at runtime construction. A port-map hit takes
an entry-local read lock, clones its bucket Arc, releases the map lock and enters
the bucket mutex. Locks use the workspace's existing `parking_lot` dependency,
as required by `.clippy.toml`. Port misses create one unactivated bucket; its first admission
initializes the active generation and full budget under the same mutex.

One token permits one connection-stage invocation. Balance is in milli-tokens
(1000 per token), capacity is `rate * 1000`, and refill is continuous elapsed
millisecond credit rounded down to milli-tokens. No refill timer is used.

For positive `u64` rate `r`, interval `i` and effective elapsed `d`:

```text
C = u128(r) * 1000
if d == 0: do not refill
else if d >= i: B = C; last = now
else:
    p = u128(d) * u128(r)
    added = (p / i) * 1000 + ((p % i) * 1000) / i
    if added > 0: B = min(C, B + added); last = now
```

A higher caller generation first settles old-rate refill, then credits a positive
capacity difference or clamps to the lower capacity. Changed parameters reset the
baseline to effective now; equal parameters only advance the generation. Finally,
consume 1000 if available, otherwise deny. These steps form one locked transition.
Old callers consume the current activated local budget and cannot downgrade it.

## Contract

- Expected O(1) map lookup and constant-time arithmetic. One read lock plus one
  bucket mutex on a hit. No production allocation in gate hits or transitions;
  first entry/port construction allocates. No gate lock spans Redis or logging.
- One persistent registry, partitioned by entry. No eviction or duplicated
  per-worker budgets. Port cardinality is bounded by `u16`; scope count can grow
  with operator churn. This budget registry is not an LRU/TinyUFO cache.
- Production uses monotonic `Instant` sampled inside the state mutex. Test clocks
  enter through a statically dispatched closure. Clamp reversed samples to the
  stored baseline; initialization is an `Option`, not a zero timestamp sentinel.
- Millisecond rounding retains the old rule: an uncredited sub-milli-token
  interval remains pending, but a nonzero refill discards its fractional remainder.
  A real parameter change also discards at most an uncredited milli-token fraction.
- All scaled balances are `u128`. Two `u64` factors fit `u128`; for `d < i`,
  `p / i < r` and `p % i < i`. Scaling either term by 1000, summing and adding
  the current capacity-bounded balance remain far below `u128::MAX`. Full-period
  refill takes the cap branch. No narrowing cast or three-factor product exists.
- Generation allocation uses checked `u64` increment during construction and
  fails new initialization closed on exhaustion. Generations follow production
  serial conf-sync order. Activation is per-port first use; skipped/unexecuted
  generations have no migration effect. The mutex also covers initial activation.

## Why this algorithm

Edgion requires the same local budget to survive configuration changes, including
rate/interval changes, with no lost concurrent consumption. A small mutex-owned
integer state makes the complete migration provable without another atomic
publication mechanism or platform-specific wide CAS. Construction-bound entries
remove unrelated resources from the old global hit-path lock.

The design does not attempt cross-port simultaneous activation, a local-plus-Redis
transaction, worker quota splitting, or an admission scheduler. Redis checks keep
the invocation's pinned runtime settings even after a newer local activation.

## Prior art comparison

Envoy source is pinned to `041439d16f2e8694b1b0feb4bf6d6d6ec86e4167`.

| Mechanism | Upstream source and behavior | Adopt or diverge |
|-----------|------------------------------|------------------|
| Construction-bound limiter | [Network local filter](https://github.com/envoyproxy/envoy/blob/041439d16f2e8694b1b0feb4bf6d6d6ec86e4167/source/extensions/filters/network/local_ratelimit/local_ratelimit.cc) holds the resolved limiter in shared config. | Adopt at entry scope; Edgion discovers the port at invocation. |
| Atomic virtual-time bucket | [AtomicTokenBucketImpl](https://github.com/envoyproxy/envoy/blob/041439d16f2e8694b1b0feb4bf6d6d6ec86e4167/source/common/common/token_bucket_impl.h) uses one CAS state with constant capacity/rate. | Adopt indivisible admission, diverge to a mutex for coherent mutable generation parameters and exact scaled integer arithmetic. |
| Sharing and lifetime | The network local filter keys shared buckets by share key plus full token-bucket config and removes unused weak entries. | Diverge: Edgion's identity excludes parameters and retains state for process lifetime. Replacing the bucket on edits would replenish budget. |
| Local/global composition | [Network global filter](https://github.com/envoyproxy/envoy/blob/041439d16f2e8694b1b0feb4bf6d6d6ec86e4167/source/extensions/filters/network/ratelimit/ratelimit.cc) performs a remote decision and pauses only that connection's filter iteration. | Keep local-before-Redis and no refund; preserve Edgion's separately accepted Pingora scheduling behavior. |

## Known limitations

The same bucket remains serialized, and ports sharing one entry still update a
shared read-lock reader count on lookup. Throughput can therefore regress for
some port distributions even while unrelated entries scale better. No fairness
or lock-free progress guarantee is claimed.

The separate [numeric-domain task](../../tasks/todo/define-redis-rate-limiter-numeric-domain/define-redis-rate-limiter-numeric-domain.md)
still owns accepted configuration limits, duration conversion, Lua exactness and
Redis integer bounds. Local arithmetic safety does not close that task.

### Measurement

The ignored in-crate `local_gate_contention_measurement` fixture has identical
workloads before/after, changing only construction/admission adapters. Each case
uses 1/2/4/8 workers and three rounds. Warm cases perform 100,000 operations per
worker; cold port-miss cases perform 1,000. Capacity is 1,000,000,000 and interval
is `u64::MAX` milliseconds so measured calls all admit without meaningful refill.
Runtime construction is outside timing; cold port allocation is inside timing.

Baseline production revision: `b297af6ea734935366205a5928c00b6407a821d7`.
Measured on macOS 15.5, Apple M4 (10 logical CPUs), with
`rustc 1.96.1 (31fca3adb 2026-06-26)` using
`rustc --edition=2021 --test -O` for the baseline module and an isolated Cargo
release harness importing the actual corrected module with `parking_lot 0.12.5`.
Both wrappers use the same fixture; the baseline has no external dependency.
The ordinary in-crate entry point is:

```bash
cargo test -p edgion-gateway --lib --release local_gate_contention_measurement --locked -- --ignored --nocapture --test-threads=1
```

The standalone wrapper avoids rebuilding unrelated release dependencies; this
measurement excludes the plugin async/Redis paths and is not an end-to-end load
test. The test runner is serial; fixture workers are concurrent. Final isolated
comparison medians, in million operations/second (baseline -> corrected):

| Workload | 1 worker | 2 workers | 4 workers | 8 workers |
|----------|----------|-----------|-----------|-----------|
| One bucket | 21.04 -> 22.08 | 8.90 -> 13.75 | 6.19 -> 10.53 | 4.10 -> 13.28 |
| Different entries | 21.82 -> 12.64 | 11.03 -> 14.34 | 6.15 -> 23.17 | 4.28 -> 41.23 |
| Same entry, different ports | 20.72 -> 13.88 | 11.40 -> 12.44 | 6.79 -> 10.39 | 4.66 -> 7.78 |
| First-use port misses | 6.91 -> 4.50 | 3.86 -> 4.04 | 2.55 -> 4.44 | 1.53 -> 4.99 |

Every timed call admitted. These are short microbenchmarks, not confidence
intervals or a throughput guarantee. The final run improves concurrent workloads
but regresses several single-worker cases. An intermediate implementation using
standard-library locks instead showed an eight-worker multi-port regression;
those numbers do not describe the final implementation. No profiler or controlled
single-factor experiment attributes these differences to a specific lock operation.
The approved single-map design is retained; port sharding requires a separately
reviewed representation change. This correction does not claim uniform speedup.

## Replacement criteria

Consider port-map sharding only when the deployment's same-entry multi-port
distribution reproduces a meaningful bottleneck. Compare memory retained per
scope, first-use latency, and warm contention before changing the shape. Consider
CAS only with a complete proof for generation migration, initialization, time
reversal and integer bounds plus measured benefit over the current critical section.
Publication-time activation requires revisiting the store/attachment contract.

## References

- [Local admission regression guard](../04-review/rate-limit/regression-guards/local-connection-admission-transitions.md): D0100 evidence, scope and verification.
- [Entry identity and retained state](../04-review/rate-limit/regression-guards/connection-rate-limit-entry-scoped-buckets.md).
- [Serial listener admission tradeoff](../04-review/listeners/intentional-design/bounded-serial-connection-admission.md): D0099, independently accepted.
