# D0026 — Enforce EdgionAcme HTTP-01 GatewayRef Ownership

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0060, ACME services audit
- Category: correctness-security
- Canonical task: [`tasks/todo/enforce-edgion-acme-http01-gatewayref-ownership/`](../../../todo/enforce-edgion-acme-http01-gatewayref-ownership/enforce-edgion-acme-http01-gatewayref-ownership.md)

## Evidence
The raw `spec.challenge.gatewayRef` is serialized with EdgionAcme and reaches the Gateway conf handler, but the Controller does not validate or resolve it into a serving attachment or register Gateway dependency requeue. The Gateway challenge store indexes every synced challenge globally by token, and pre-route HTTP-01 interception checks Host/token before route or Gateway selection without enforcing target ownership. A matching challenge can therefore be served on a non-selected co-located Gateway/listener.

## Why deferred
The correction spans Gateway API-style reference validation and status, cross-namespace ReferenceGrant dependencies, Gateway/listener dependency requeue, a deduplicated resolved attachment set with section-and-port semantics, atomic challenge indexing, and receiving-listener enforcement. It must not be coupled to B0060's one-line expiry-boundary correction.

## Integration-test impact
Existing integration tests remain frozen. D0027 records the genuinely required new multi-Gateway HTTP-01 isolation scenario and is blocked on the technical serving-attachment design.
