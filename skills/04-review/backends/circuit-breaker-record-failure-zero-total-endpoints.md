---
name: circuit-breaker-record-failure-zero-total-endpoints
description: Use when reviewing circuit-breaker findings of the form "record_failure bypasses the percent cap when total_endpoints == 0" — already fixed.
status: fixed
finding_id: g7-006 Issue B
closed: 2026-05-29
---

# Circuit Breaker record_failure total_endpoints == 0 Bypass — FIXED

## Conclusion

The circuit-breaker `record_failure` `total_endpoints == 0` bypass is fixed: when the caller-supplied total is 0, it now falls back to `svc.endpoints.len().max(1)`, so the percent-cap is always applied with a non-zero denominator.

Re-reports of "record_failure skips the ejection percent cap when total_endpoints is 0" are **not accepted**; verify the `svc.endpoints.len().max(1)` fallback before re-reporting.

## Core Rationale

**1. The bypass**

`record_failure` computed the maximum-ejection percent cap against a caller-supplied `total_endpoints`; when that value was 0, the cap computation divided/bounded against zero and the cap was effectively bypassed.

**2. The fix**

When the caller-supplied total is 0, the code falls back to `svc.endpoints.len().max(1)`, guaranteeing a sane non-zero denominator so the percent cap applies as intended.

**3. Regression coverage**

Unit regression `test_percent_cap_applies_when_total_endpoints_is_zero` pins the fallback behaviour.

## Fix Suggestions Not Accepted

- Re-reporting the cap bypass without checking the zero-total fallback.

## Re-evaluation Triggers

Re-open only if:

- A new caller path supplies a total that can be 0 *and* the service endpoint set is also empty (the `.max(1)` already guards this).

## Reference Cases

- Source: 2026-05-29 circuit-breaker review (g7-006 Issue B).
- Regression: `test_percent_cap_applies_when_total_endpoints_is_zero`.
- Companion finding: `backends/circuit-breaker-record-success-gauge-leak.md` (g7-006 Issue A).
