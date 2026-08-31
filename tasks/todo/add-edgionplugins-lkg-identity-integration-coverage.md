# Add EdgionPlugins LKG Identity Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | blocked |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add a new end-to-end scenario proving that EdgionPlugins last-known-good retention follows resource identity consistently through source events, compression, conf-sync, reconnect, and Gateway publication. This coverage is blocked on [`retain-edgionplugins-last-known-good-resource-identity`](retain-edgionplugins-last-known-good-resource-identity/retain-edgionplugins-last-known-good-resource-identity.md); the current Gateway quality campaign must not modify existing integration tests.

## Requirements

- Add coverage only after the technical task defines and implements the cross-source identity contract.
- Reuse one existing Controller/Gateway integration topology and shared readiness helpers.
- Publish a valid DSL or WAF plugin generation and prove its behavior is active.
- Apply an invalid update to the same logical resource and prove the valid generation remains active.
- Delete and recreate the same namespace/name with a new identity and invalid configuration; prove the deleted generation is not served and the replacement fails closed.
- Force the supported reconnect or relist path and prove full-snapshot behavior matches the equivalent partial history.
- Publish a valid replacement and prove normal recovery.
- Use `wait_phase` or state polling for every transition; do not use fixed sleeps.

## Scope

### In scope

- A new standalone dynamic integration scenario and fixtures.
- Minimal harness registration only when automatic discovery cannot find the new scenario.
- Identity-sensitive update, recreate, reconnect, and recovery assertions.

### Out of scope

- Implementing or redesigning identity semantics.
- Editing existing integration assertions to manufacture coverage.
- Adding a new deployment topology, fixed sleeps, or unrelated plugin behavior coverage.

## Acceptance criteria

- Same-identity invalid update retains the last valid plugin behavior.
- New-identity invalid recreation never serves the deleted behavior and fails closed according to the plugin contract.
- Reconnect/full-snapshot results match the corresponding partial event history.
- A valid subsequent generation recovers deterministically.
- All transitions use existing readiness/state polling and remain stable under repeated runs.
- The new scenario passes without weakening or modifying existing integration tests.

## Dependency

Blocked on [`Retain EdgionPlugins Last-known-good Only for the Same Resource Identity`](retain-edgionplugins-last-known-good-resource-identity/retain-edgionplugins-last-known-good-resource-identity.md).

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Create a separate future integration issue instead of changing tests in B0062. | Unit tests cannot validate identity across source events, compression, conf-sync, reconnect, and Gateway publication. |
