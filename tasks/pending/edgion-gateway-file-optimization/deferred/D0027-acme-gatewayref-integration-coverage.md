# D0027 — Add ACME GatewayRef Isolation Integration Coverage

## Meta
- Status: blocked
- Severity: high
- Discovered in: B0060, ACME services audit
- Category: testing
- Canonical task: [`tasks/todo/add-acme-gatewayref-isolation-integration-coverage.md`](../../../todo/add-acme-gatewayref-isolation-integration-coverage.md)
- Dependency: D0026 / EdgionAcme HTTP-01 gatewayRef ownership design and implementation

## Evidence
The existing ACME integration suite exercises Pebble account and DNS-01 flows but has no HTTP-01 Gateway data-plane scenario. Source-local tests cannot prove Controller-to-conf-sync-to-listener ownership across two Gateway identities.

## Why deferred
The scenario must follow the resolved serving-attachment API selected by D0026. Adding it before that design would encode guessed behavior, while modifying existing integration files during this campaign would violate the frozen-test contract.

## Candidate coverage
Add a new dynamic scenario with two Gateway/listener ports sharing one challenge Host/token: selected A serves, non-selected B falls through; hot retarget A to B reverses the result; challenge removal or expiry makes both fall through. Use readiness and state polling rather than fixed sleeps.
