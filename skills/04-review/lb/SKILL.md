---
name: review-lb
description: Review knowledge for lb, grouped by decision type.
---

# lb review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [lb-F010 — single `build_lock` covering all four policy slots is intentional — DESIGN-TRADEOFF](intentional-design/build-lock-single-mutex-intentional.md)
- [lb-F011 — CRC32 session-affinity token is a stickiness hint, not a security boundary — DOCUMENTED](intentional-design/crc32-session-affinity-not-a-security-boundary.md)
- [P2C choice count stays fixed at two — DESIGN-TRADEOFF](intentional-design/p2c-choice-count-fixed-at-two.md)

## Regression guards

- [lb-F004 — consistent-hash walk charges budget per distinct backend, not per ring slot](regression-guards/consistent-hash-duplicate-node-budget.md)
- [ConsistentHashRing::select Per-Request HashSet Allocation](regression-guards/consistent-hash-ring-seenset-no-heap.md)
- [LB Backend Wrapper Name Boundary — FIXED](regression-guards/lb-backend-wrapper-name-boundary.md)
- [RrCore::select / ChCore::select Duplicated Resolution + ChCore Double addr_map](regression-guards/rr-ch-core-shared-resolve-helper.md)
- [lb-F013 — RR / ConsistentHash selectors cap per-backend weight expansion](regression-guards/rr-consistent-hash-per-backend-weight-cap.md)
- [backends-F012 — select_backend_by_policy dead wrapper](regression-guards/select-backend-by-policy-dead-wrapper.md)
- [Session-persistence token must hash the LB cell address, not the peer ip/port — FIXED](regression-guards/sp-token-affinity-map-address-canonicalization.md)
- [Session persistence across weighted BackendRefs — resolve-before-selection — FIXED](regression-guards/sp-weighted-backends-resolve-before-selection.md)
- [WeightedRoundRobin::new O(sum-of-weights) Sequence Allocation / Unclamped Weight](regression-guards/weighted-round-robin-normalize-weights.md)
- [lb-FB01 — `WeightedRoundRobin::new` returns `Result`, not panic — FIXED](regression-guards/wrr-new-result-api.md)

## Superseded decisions

- [PolicyStore::remove_by_route Unconditional Clone in retain Closure](superseded/policy-store-remove-by-route-no-clone.md)
- [LbBackend::update_ewma u64 Overflow](superseded/update-ewma-u128-intermediate.md)
