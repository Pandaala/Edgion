# Add Auth Identity-header Spoofing Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | blocked |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add new end-to-end scenarios proving that forged authentication identity headers never reach the real upstream and that authenticated, anonymous, rejected, and multiple-auth-plugin paths expose only canonical gateway-owned identity. This task is blocked on [`unify-auth-identity-header-sanitization`](unify-auth-identity-header-sanitization/unify-auth-identity-header-sanitization.md); the current Gateway campaign must not modify existing integration tests.

## Requirements

- Add coverage only after the canonical sanitization and ordering contract is implemented.
- Use new standalone scenario files and the existing Controller/Gateway/upstream topology.
- Send forged consumer username, group, anonymous-consumer, and every other canonical identity header from the client.
- Cover authenticated success, anonymous allowance, rejected authentication, invalid resolved header configuration, and at least one multiple-auth-plugin ordering case.
- Assert the real upstream observes only gateway-owned values or header absence defined by the contract.
- Use existing readiness and state polling; do not use fixed sleeps.
- Never use real credentials or log sensitive tokens in fixtures or diagnostics.

## Acceptance criteria

- No forged identity value reaches the upstream in any covered auth state.
- Authenticated and anonymous outputs match the canonical ownership contract exactly.
- Rejection and invalid configuration cannot leak stale identity from the client or another plugin.
- Multiple-plugin ordering is deterministic across repeated runs.
- New coverage passes without weakening or modifying existing integration scenarios.
