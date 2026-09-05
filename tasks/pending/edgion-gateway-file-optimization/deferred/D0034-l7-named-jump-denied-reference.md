# D0034 — Fail Closed on Denied L7 Named Jumps

## Meta
- Status: done (2026-09-05)
- Severity: high
- Discovered in: B0066, Canary audit
- Category: correctness-security
- Canonical task: completed and removed from `tasks/todo/` on 2026-09-05
- Decision record: [`skills/04-review/routing/denied-named-jump-fails-closed.md`](../../../../skills/04-review/routing/denied-named-jump-fails-closed.md)

## Evidence
DynamicInternalUpstream validates its configured target through a raw Service kind/name/namespace scan that ignores `ref_denied`, so it can publish a denied internal jump and record Success. The HTTP upstream selector then treats every named-finder error, including `RefDenied`, as an ordinary named miss and falls through to weighted selection. A legal sibling can be selected after the plugin has reported success, or a denied weighted target can fail only later.

## Why deferred
Correctness spans the DynamicInternalUpstream plugin's `onInvalid` boundary and the shared L7 upstream selector's typed error propagation. B0066 owns only Canary and must not partially change shared HTTP routing. The existing L4 denied-reference task covers TCP, UDP, and TLS selection, not HTTP named jumps.

## Integration-test impact
Existing integration tests remain frozen, and the fix shipped with colocated plugin and upstream-selector tests covering denied, missing, namespace, sibling, and weight-zero behavior. The original "no separate future integration issue is required" judgement was revised on close: unit tests cannot exercise a real ReferenceGrant fixture or assert the access-log error code, so end-to-end evidence is tracked in [`tasks/todo/add-denied-named-jump-integration-coverage.md`](../../../todo/add-denied-named-jump-integration-coverage.md).
