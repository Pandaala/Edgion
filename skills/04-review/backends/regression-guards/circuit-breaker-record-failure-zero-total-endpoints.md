---
name: circuit-breaker-record-failure-zero-total-endpoints
description: Current identity-bound Service outlier ownership and preserved regression boundary.
status: fixed
finding_id: g7-006 Issue B
closed: 2026-05-29
knowledge_type: regression-guards
---

# Percent caps use current coordinator membership

Current Service failure completion accepts an actual backend and captured policy
token. The coordinator verifies endpoint identity and membership under the
endpoint completion lock, then supplies `current.entries.len()` to the single
identity arbitration branch. An empty/absent membership rejects the completion
before identity-state registration; callers cannot supply a stale or zero count.
Concurrent admitted candidates serialize the cap check and final open under the
service ejection mutex. Positive percentages retain the minimum-one allowance.

The historical g7-006 Issue B fix used `svc.endpoints.len().max(1)` when a
hash-wrapper caller supplied zero. That wrapper, caller count and fallback were
removed under ARCH-018. They are historical evidence, not current guidance.

The preserved `test_percent_cap_applies_when_total_endpoints_is_zero` and
`test_percent_cap_applies_concurrently_when_total_endpoints_is_zero` names now
pin the successor boundary: old completions at empty membership cannot recreate
state, then republished four-endpoint membership enforces a two-endpoint 50% cap.
Normal sequential and concurrent percent tests exercise the same owner.

Re-evaluate if coordinator validation stops preceding state registration, if a
caller count reappears, or if final candidate commit escapes arbitration.
Source: outlier_detection/store.rs and discovery/coordinator.rs.
Provenance: g7-006 Issue B (2026-05-29); ARCH-018.
