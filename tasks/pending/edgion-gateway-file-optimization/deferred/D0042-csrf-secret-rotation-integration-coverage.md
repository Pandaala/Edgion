# D0042 — Add CSRF Secret-rotation Integration Coverage

## Meta
- Status: fixed (2026-09-06)
- Severity: medium
- Discovered in: B0070, CSRF audit
- Category: integration-testing
- Depends on: B0070; independent of D0041
- Resolution: [`skills/04-review/csrf/csrf-secret-rotation-integration-coverage.md`](../../../../skills/04-review/csrf/csrf-secret-rotation-integration-coverage.md)

## Evidence
Existing frozen CSRF integration coverage verified normal double-submit behavior and a statically missing Secret, but did not exercise Secret event propagation through Controller resolution, conf-sync publication, PluginStore generation replacement, deletion, and recovery.

## Resolution
`CsrfSecretRotation` is a standalone dynamic scenario under `EdgionPlugins/Csrf/DynamicTest`. It covers rotation, pre-rotation token 401, new-token success, deletion fail-close 500, and restore recovery through `wait_phase`. The existing static CSRF suite was not rewritten.
