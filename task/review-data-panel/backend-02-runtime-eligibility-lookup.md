# Evaluate prepared Service endpoint eligibility lookup

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `0a20600cf`.

## Evidence

`backends/selector.rs::runtime_eligible` reads the backend's already-built
`EndpointRuntimeIdentity` and queries `ServiceRuntimeSnapshot.entries` before
checking health and outlier eligibility. The derived Hash key includes the Service
key, scope, socket, incarnation and proof digest. This is endpoint-map hashing
and lookup, not reconstruction of identity or a top-level Service-store lookup.

`degrade_engaged` can inspect a candidate during survivor counting before the
ordinary selector checks it again. RR/ConsistentHash also resolve addresses through
their compiled maps; LeastConn/EWMA use P2C and may revisit rejected candidates.
The existing short-circuits, survivor early exit and selection budgets remain
important. This candidate concerns the runtime eligibility lookup, distinct from
[backend-01](backend-01-selected-slot-lookup.md)'s successful-pick slot reverse scan.
No profile currently establishes that this lookup is a material bottleneck.

## Decision and constraints

Keep the current map, or investigate a generation-owned section-local binding
between selector candidates and runtime states. A request-built binding would
add work proportional to section size and is not an assumed improvement. A cold
binding would need explicit ownership and publication design:

- Membership activation creates fresh endpoint runtime states, while policy-only
  snapshot publication can share the entries map. Discovery publication precedes
  activation; mismatched membership fails closed. A binding must preserve that
  boundary and must not point into an unrelated activation.
- `EndpointRuntimeState` strongly owns its `LbBackend`. A strong runtime-state
  reference stored back into that backend would create an ownership cycle.
  Section/runtime ownership, retirement and retained-generation memory need proof.
- Health remains live. Retired states freeze their previous eligibility. The
  outlier getter can perform cooldown recovery under completion locks and settle
  gauges. Do not cache eligibility booleans, merge repeated observations, or move
  recovery merely to avoid lookup work.
- Preserve exact section/incarnation identity, missing-state rejection, health
  before outlier checking, rejection-counter behavior, degrade bypass, policy
  capture and live global-default semantics. No new data-plane tracing or metrics.

The [current runtime-selection contract](../../skills/04-review/lb/regression-guards/service-runtime-snapshot-selection.md)
already accepts residual cooldown locks and rejection/recovery allocations. This
proposal does not reopen those accepted costs or classify the current lookup as
a correctness defect. Keeping the existing implementation is a valid decision.

## Acceptance for a future implementation

Compare CPU and retained memory for small/large sections, all four policies,
degrade enabled/disabled, healthy and mostly rejected populations, and frequent
membership/policy reloads. Include publication cost, retained old generations and
any replacement indirection/refcount work, not only isolated hash timing.

Verify exact candidate order and budgets, live health changes, same-socket section
isolation, membership activation gaps, late-completion rejection, policy-only
updates, frozen retired state, cooldown/gauge settlement and cancellation cleanup.
Extend meaningful generation and allocation tests for the chosen design; existing
passing tests alone cannot establish equivalence of a new binding scheme.

Fresh unchanged-runtime verification passed 12 selector, eight compiled-selector,
41 discovery coordinator and 36 outlier tests (97 total). These validate the
current baseline, not this unimplemented proposal. No throughput or contention
benchmark ran.
