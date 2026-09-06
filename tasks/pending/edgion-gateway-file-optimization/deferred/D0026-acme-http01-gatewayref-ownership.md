# D0026 — Enforce EdgionAcme HTTP-01 GatewayRef Ownership

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0060, ACME services audit
- Category: correctness-security
- Canonical record: this file. The former `tasks/todo/enforce-edgion-acme-http01-gatewayref-ownership/` path is absent; do not recreate a duplicate coverage todo from D0027.

## Evidence
The raw `spec.challenge.gatewayRef` is serialized with EdgionAcme and reaches the Gateway conf handler, but the Controller does not validate or resolve it into a serving attachment or register Gateway dependency requeue. The Gateway challenge store indexes every synced challenge globally by token, and pre-route HTTP-01 interception checks Host/token before route or Gateway selection without enforcing target ownership. A matching challenge can therefore be served on a non-selected co-located Gateway/listener.

## Why deferred
The correction spans Gateway API-style reference validation and status, cross-namespace ReferenceGrant dependencies, Gateway/listener dependency requeue, a deduplicated resolved attachment set with section-and-port semantics, atomic challenge indexing, and receiving-listener enforcement. It must not be coupled to B0060's one-line expiry-boundary correction.

## Integration-test impact
Existing integration tests remain frozen. D0027 is closed as not a standalone defect; after this serving-attachment design is implemented, this finding owns the new dynamic scenario: two Gateway/listener ports share one challenge Host/token, selected A serves and non-selected B falls through, a hot retarget from A to B reverses the result, and challenge removal or expiry makes both fall through. Reuse one existing topology and readiness/`wait_phase` polling; do not use fixed sleeps or edit existing integration assertions.
