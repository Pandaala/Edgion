# D0034 — Fail Closed on Denied L7 Named Jumps

## Meta
- Status: needs-implementation
- Severity: high
- Discovered in: B0066, Canary audit
- Category: correctness-security
- Canonical task: [`tasks/todo/fail-closed-on-denied-l7-named-jumps.md`](../../../todo/fail-closed-on-denied-l7-named-jumps.md)

## Evidence
DynamicInternalUpstream validates its configured target through a raw Service kind/name/namespace scan that ignores `ref_denied`, so it can publish a denied internal jump and record Success. The HTTP upstream selector then treats every named-finder error, including `RefDenied`, as an ordinary named miss and falls through to weighted selection. A legal sibling can be selected after the plugin has reported success, or a denied weighted target can fail only later.

## Why deferred
Correctness spans the DynamicInternalUpstream plugin's `onInvalid` boundary and the shared L7 upstream selector's typed error propagation. B0066 owns only Canary and must not partially change shared HTTP routing. The existing L4 denied-reference task covers TCP, UDP, and TLS selection, not HTTP named jumps.

## Integration-test impact
Existing integration tests remain frozen. Colocated plugin and upstream-selector tests can prove denied, missing, namespace, sibling, and weight-zero behavior; no separate future integration issue is required.
