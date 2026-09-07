---
name: circuit-breaker-record-success-gauge-leak
description: Use when reviewing circuit-breaker findings of the form "record_success removes an ejected endpoint but leaks the ejection gauge" — already fixed.
status: fixed
finding_id: g7-006 Issue A
closed: 2026-05-29
knowledge_type: regression-guards
---

# Circuit Breaker record_success Gauge Leak — FIXED

## Conclusion

The circuit-breaker `record_success` gauge leak is fixed: removing an entry whose `ejected_at.is_some()` now calls `record_cb_recovery`, so the ejection gauge is decremented when the endpoint recovers instead of leaking.

Re-reports of "record_success removes an ejected endpoint without decrementing the ejection gauge" are **not accepted**; verify the `record_cb_recovery` call on the `ejected_at.is_some()` removal path before re-reporting.

## Core Rationale

**1. The leak**

`record_success` previously removed an endpoint entry that had been ejected (`ejected_at.is_some()`) without emitting the corresponding recovery metric, so the circuit-breaker ejection gauge was incremented on ejection but never decremented on recovery — a monotonically leaking gauge.

**2. The fix**

The removal path now calls `record_cb_recovery` when the removed entry carries `ejected_at.is_some()`, balancing the gauge.

**3. Regression coverage**

Unit regression `test_record_success_on_ejected_does_not_leak_gauge` pins the balanced behaviour.

## Fix Suggestions Not Accepted

- Re-reporting the gauge as leaking without checking the `record_cb_recovery` call.

## Re-evaluation Triggers

Re-open only if:

- A new ejection/recovery state transition is added that bypasses the `record_cb_recovery` accounting.

## Reference Cases

- Source: 2026-05-29 circuit-breaker review (g7-006 Issue A).
- Regression: `test_record_success_on_ejected_does_not_leak_gauge`.
- Companion finding: `backends/circuit-breaker-record-failure-zero-total-endpoints.md` (g7-006 Issue B).

## Historical resolution evidence: g7-006 Issue B

`record_failure` `total_endpoints == 0` bypass fixed: falls back to `svc.endpoints.len().max(1)` when caller total is 0; unit regression `test_percent_cap_applies_when_total_endpoints_is_zero`

## Resolution references

- `g7-006 Issue B` — —.
