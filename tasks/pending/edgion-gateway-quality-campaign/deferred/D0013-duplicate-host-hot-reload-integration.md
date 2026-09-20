# D0013 — Duplicate-Host Policy Hot-Reload Integration Coverage

- Status: ready-after-campaign-freeze
- Canonical task: `tasks/todo/add-duplicate-host-policy-hot-reload-integration-coverage.md`
- Discovered in: B0066

## Evidence

Existing frozen integration coverage proves static duplicate-Host rejection but does not update or delete the referenced EGC while ordinary HTTP and redirect listeners remain active. B0066 can add crate-local generation-pinning tests, but registering the cross-process scenario requires edits under frozen `edgion-tests/**`.

## Required coverage

Exercise the default, a live update to the explicit `false` escape hatch, update back to `true`, and deletion or missing-reference safe fallback on both listener paths without restarting the Gateway.
