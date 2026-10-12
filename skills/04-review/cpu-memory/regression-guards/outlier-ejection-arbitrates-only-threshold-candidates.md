---
knowledge_type: regression-guards
---

# Outlier ejection arbitrates only threshold candidates

## Decision

Failure accounting is endpoint-local. A failure that is below every configured
threshold, or targets an endpoint that is already open, must return before the
per-service ejection mutex and endpoint scan. When `max_ejection_percent` is
disabled, opening also stays endpoint-local and does not take the service mutex.

For a threshold candidate, the endpoint state returns an opaque generation-bound
token. The wrapper then serializes the percent check and final open operation.
The final operation revalidates the generation, open window, and counters, so a
concurrent success cannot be overwritten by a stale candidate.

The same two-phase state-machine API is used by ordinary Service endpoints and
AI credential outlier state. This preserves the max-ejection invariant while
keeping the common failure path free of the service-wide O(N) critical section.

## Regression coverage

- Service-level identity/token coverage counts percent arbitrations in the actual
  production identity branch and requires zero before
  the threshold, one at the threshold, and no additional arbitration after the
  endpoint opens.
- State-machine coverage records a candidate, applies a success, and requires
  the stale candidate to leave the endpoint closed.
- Existing concurrent percent-cap and same-endpoint tests continue to require
  bounded, once-only ejection.

## Resolution references

- `gw-10` — 2026-08-09.

## Ownership correction

ARCH-018 removed the unused socket-hash execution wrapper whose instrumentation
previously made the named store regression green without exercising production
arbitration. The counter is test-only and now sits under the actual identity
branch's service lock. Store fixtures publish real isolated coordinator
membership and capture real policy tokens; they implement no second algorithm.
Historical zero-count tests now verify empty-membership rejection and the
coordinator-owned count. The shared passive kernel and AI consumer stay unchanged.
No throughput or allocation improvement is claimed without measurement.
