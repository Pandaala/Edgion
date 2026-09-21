# D0013 — Duplicate-Host Policy Hot-Reload Integration Coverage

- Status: resolved
- Canonical outcome: [HTTP/1.1 duplicate Host header rejected with 400](../../../../skills/04-review/http1/regression-guards/duplicate-host.md)
- Discovered in: B0066

The source task was closed after the cross-process `DuplicateHostPolicyHotReload`
dynamic scenario landed for ordinary HTTP and redirect listeners, covering the
safe default, the explicit `false` escape hatch, the restored `true` generation
and the deletion fallback. The linked semantic rule owns the current contract,
evidence and test results.
