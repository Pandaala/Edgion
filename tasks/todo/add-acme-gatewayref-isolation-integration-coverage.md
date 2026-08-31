# Add ACME GatewayRef Isolation Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | blocked |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add a new end-to-end HTTP-01 scenario proving that EdgionAcme `gatewayRef` ownership is enforced across Controller validation, conf-sync, and two Gateway/listener identities. This coverage is blocked on the approved serving-attachment design and implementation in [`enforce-edgion-acme-http01-gatewayref-ownership`](enforce-edgion-acme-http01-gatewayref-ownership/enforce-edgion-acme-http01-gatewayref-ownership.md); the current Gateway quality campaign must not modify existing integration tests.

## Requirements

- Add coverage only after the technical task defines and implements the resolved serving attachment and listener-selection behavior.
- Reuse the existing Gateway and Controller integration topology; do not add a new deployment shape merely for this scenario.
- Create two Gateway/listener identities on distinct listener ports that can receive the same challenge Host and token.
- Publish one deterministic HTTP-01 challenge targeting Gateway A and prove A serves the key authorization while Gateway B falls through normally.
- Hot-update the target from A to B and prove the serving/fallthrough results reverse without relying on fixed sleeps.
- Remove or expire the challenge and prove both listeners fall through.
- Use existing `wait_phase` or state-polling patterns for readiness, conf-sync, target changes, and removal.
- Never log or embed real Secret material. Keep test token/key-authorization data synthetic and bounded.

## Scope

### In scope

- New ACME HTTP-01 gatewayRef isolation integration scenario files and fixtures.
- Minimal harness registration only if the existing discovery mechanism cannot pick up new scenario files automatically.
- Positive target, negative non-target, hot-retarget, and removal/expiry assertions.

### Out of scope

- Implementing or redesigning gatewayRef ownership semantics.
- Editing existing integration assertions to manufacture coverage.
- Fixed sleeps, new cluster topology, ACME account/DNS-01 changes, or external CA dependencies.

## Acceptance criteria

- Selected Gateway A serves and non-selected B falls through for the same Host/token.
- After a dynamically observed A-to-B retarget, B serves and A falls through.
- After challenge removal or expiry, neither listener serves the challenge.
- All transitions use readiness/state polling and are stable under repeated runs.
- The new coverage passes without weakening or modifying existing integration scenarios.

## Dependency

Blocked on [`Enforce EdgionAcme HTTP-01 GatewayRef Ownership`](enforce-edgion-acme-http01-gatewayref-ownership/enforce-edgion-acme-http01-gatewayref-ownership.md).

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Create a separate future integration issue instead of changing tests in B0060. | Existing ACME integration coverage is account and DNS-01 oriented and cannot validate multi-Gateway HTTP-01 serving ownership. |
