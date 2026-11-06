# infra3-02: Round-robin restarts at the lowest-address endpoint on every selector rebuild, synchronizing all Gateway replicas

- **Category**: best-practice
- **Severity**: P2
- **Scope**: local
- **Location**: `edgion-gateway/src/lb/selection/round_robin.rs:89-106,115-121`,
  `edgion-gateway/src/lb/compiled_selector.rs:83-92,261,284-288`,
  `edgion-gateway/src/backends/discovery/coordinator.rs:1900`,
  `edgion-gateway/src/backends/selector.rs:106`

## Summary
Round robin is the default LB policy (`selector.rs:106`). Its selector sorts endpoints by socket address and
starts its counter at 0. A new selector is built every time the Service's endpoint fingerprint changes, for
example on a pod add or remove, a readiness flip, or an IP change. After every rebuild, every Gateway replica
therefore sends its next requests to the same lowest-address endpoints, in the same order. During rollouts,
autoscaling and readiness flapping, which are the periods of highest churn and tightest capacity, the low end
of the address space gets a multiple of its fair share.

## Current behavior (evidence)
- `round_robin.rs:89-90`: `let mut sorted = backends.to_vec(); sorted.sort_by(|a, b| a.addr.cmp(&b.addr));`.
  The order is deterministic and identical on every replica.
- `round_robin.rs:104`: `counter: AtomicUsize::new(0)`. `round_robin.rs:115-117`: the pick is
  `weighted[counter % len]`, so the first pick after a build is index 0, then 1, and so on.
- `compiled_selector.rs:83-92` (`RrCore::build`) creates a new `RoundRobinSelector` for each section build.
  `CompiledSelectors::build` (`:284-288`) is called from `coordinator.rs:1900` whenever a section is
  re-materialized. Per `gw-infra-03`, that happens on every content-fingerprint change of the Service. No
  counter carries over from the previous generation.
- The `ChCore` keyless fallback (`compiled_selector.rs:120-125`) embeds another `RrCore` with the same
  behavior.
- When the selected endpoint fails the health filter, `select` walks to the *next* index
  (`round_robin.rs:118-128`). The address-adjacent neighbor of an ejected endpoint therefore absorbs its full
  share as well.
- Worked example: 20 Gateway replicas at about 5 rps each, a 50-pod Service in a rolling update that changes
  endpoints about every 2 s. Each replica issues about 10 requests per selector generation, so in every
  generation only the 10 lowest-address pods receive traffic, from all 20 replicas. For the duration of the
  rollout, 20% of the pods carry about 100% of the load, roughly 5x their fair share.

## Industry comparison
- Envoy `RoundRobinLoadBalancer` (EDF base) seeds per-priority/locality round-robin indexes from a random
  `seed_`. On host-set refresh it keeps the existing index (`rr_indexes_.insert({source, seed_})` does not
  overwrite), so replicas are desynchronized and fairness survives membership churn.
- NGINX `upstream` round robin keeps the position across reloads within a worker. Each worker starts
  independently, so instances are not aligned.
- Pingora's own `RoundRobin` also starts at 0. Edgion's P2C-based selectors (`selection/p2c.rs`) already use
  randomness, so only the RR path has this problem.

## Impact
Load skew and tail latency on the default policy during exactly the events (deploys, HPA scale-out,
readiness flaps) that already reduce capacity. The skew grows with the number of replicas and with churn,
and it compounds the neighbor-spill effect when endpoints are ejected.

## Suggested direction
1. Seed `RoundRobinSelector::counter` with a random value at build time, which is cheap and removes the
   cross-replica alignment, and/or
2. Carry the previous generation's counter into the rebuilt selector for the same Service section. The
   coordinator already keeps per-endpoint runtime counters across generations, so it can pass the old
   counter value in as well.
3. Optionally shuffle the endpoint order once per build with a per-process seed instead of sorting by
   address. That keeps the order deterministic within a replica but different across replicas.
4. For the unhealthy-skip walk, advance from a randomized offset instead of `idx + 1`, so one ejected endpoint
   does not double the load on its neighbor.

## Notes
Checked `skills/04-review/lb/` (SKILL index, `rr-consistent-hash-per-backend-weight-cap.md`,
`weighted-round-robin-normalize-weights.md`, `rr-ch-core-shared-resolve-helper.md`,
`service-runtime-snapshot-selection.md`). None of them decide the RR start position or a rebuild-reset policy.
`gw-infra-03` covers the rebuild cost and `gw-infra-02` the health/outlier state reset. This finding is the
separate fairness consequence of the counter reset combined with address-sorted ordering.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `lb/selection/round_robin.rs:89-90` sorts by `addr`. `:104` starts with `counter: AtomicUsize::new(0)`. `:115-121` picks `weighted[start % wlen]`, and the unhealthy fallback walks `idx + 1` (`:124-130`).
- Each rebuild constructs a fresh selector, so every replica restarts at the same lowest-address endpoint. No `skills/04-review/lb` page decides start position or reset policy.
- The worked example (rebuild every 2 s) is an upper-bound scenario. The mechanism and the fix (random seed or counter carry-over) are sound and cheap.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `lb/selection/`, `lb/compiled_selector.rs`, `backends/discovery/coordinator.rs` and `backends/selector.rs` are unchanged between feature-08-30 and HEAD; `skills/04-review/lb/` has no changes.
- `round_robin.rs:89-90` still sorts by `addr` and `:104` still starts at `AtomicUsize::new(0)`; each rebuild via `CompiledSelectors::build` (`compiled_selector.rs:284`) creates a fresh counter.
