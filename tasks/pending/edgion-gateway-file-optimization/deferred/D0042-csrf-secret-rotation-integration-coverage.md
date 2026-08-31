# D0042 — Add CSRF Secret-rotation Integration Coverage

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0070, CSRF audit
- Category: integration-testing
- Depends on: B0070; independent of D0041
- Canonical task: [`tasks/todo/add-csrf-secret-rotation-integration-coverage.md`](../../../todo/add-csrf-secret-rotation-integration-coverage.md)

## Evidence
Existing frozen CSRF integration coverage verifies normal double-submit behavior and a statically missing Secret, but does not exercise Secret event propagation through Controller resolution, conf-sync publication, PluginStore generation replacement, deletion, and recovery.

## Why deferred
The scenario requires new integration files and minimal suite wiring after the current campaign's frozen-test boundary. It must remain a standalone dynamic scenario and must not rewrite the existing CSRF scenario to manufacture rotation coverage.

## Integration-test impact
Required. Add a future standalone scenario proving an old-key token is rejected after rotation, a new token succeeds, Secret deletion fails closed with 500, and Secret restoration recovers. Use readiness/phase polling and no fixed sleeps.
