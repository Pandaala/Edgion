---
id: ARCH-016
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-266
independent_verification: confirmed
---

# Compile BackendSelector as immutable generation-owned state

## Current ownership and concrete change scenario

BackendSelector in lb/backend_selector/selector.rs wraps SelectorState in an
ArcSwapOption and exposes new, is_initialized and idempotent init. Selection loads
the swap on every backendRef selection. All three production builders initialize
the selector before publishing their owning generation: HttpRouteRuleUnit,
GrpcRouteRuleUnit and CompiledL4Backends::from_references. No production request
initializes or replaces it. Empty HTTP/gRPC references intentionally retain the
uninitialized sentinel and its existing no-backend error behavior.

The mutable publication API therefore describes a lifecycle the owning runtime
does not use. Adding a backendRef policy or initialization failure state requires
reasoning about both cold generation compilation and an apparent later write
path, while the actual runtime only needs one captured immutable selection plan.
This is structural debt, not an observed race. The historic lazy-initialization
race was already removed from the call path.

## Bounded improvement and alternatives

Construct private immutable SelectorState when compiling its owning generation.
Represent empty and invalid construction states explicitly, retaining current
error outcomes. Remove the two-stage initialization API and swap after auditing
all callers; update every caller and test without compatibility aliases.
WeightedRoundRobin's atomic cursor remains real request-time mutable state and
must be retained. This recommendation concerns backendRef selection, not live
endpoint health or the endpoint algorithm implementations.

Keeping the swap would preserve an unused reinitialization seam and request-time
snapshot load. Moving all selectors into one general runtime registry would add
ownership and synchronization rather than simplify the captured generation.
The projected benefit is a smaller state model and fewer publication seams; no
throughput measurement or request-copy reduction is claimed.

## Required invariants and migration risks

Preserve default weights, partial-weight rejection, zero/negative filtering,
raw-slot mapping, HTTP AI/generic partitioning, denied weighted shares,
single-backend fast paths, existing selection errors, WRR normalization and shared
cursor ownership across rule match units. Captured old generations remain usable;
new generations receive their own intended cursor state. Endpoint eligibility,
retry, persistence and algorithm-specific behavior stay with their current owners.
Do not introduce owned request copies or retain the old lifecycle for mocks.

Future checks should cover constructors and error states, weighted distributions,
raw-slot indices, empty routes and reload/cursor ownership with existing tests.
Benchmark only if a later implementation claims throughput improvement.

## Original authority and review limits

The analyst fully read all 17 LB primaries and traced HTTP/gRPC/L4 builders, native
selection and retry/terminal callers, existing selector and algorithm tests,
original backend architecture and LB catalog. The historical
concurrency/review-evidence/lb-false-positives.md rejects a lazy-init race; its
old initialization premise does not establish a need for today's unused swap.
lb/regression-guards/wrr-new-result-api.md requires constructor errors to survive.
cpu-memory/regression-guards/domain-route-rules-inner-arcswap-removed.md supplies
an analogous immutable-generation decision, not automatic approval here.
Static review only; independent verification is pending.
