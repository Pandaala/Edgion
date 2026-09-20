# Add Duplicate-Host Policy Hot-Reload Integration Coverage

## Meta

- Status: pending
- Type: test
- Priority: P1
- Issue: N/A

## AI guide summary

Add a cross-process integration scenario proving that `rejectDuplicateHost` updates and safe fallback take effect on already-running ordinary HTTP and HTTP-to-HTTPS redirect listeners. This task is deferred until the campaign freeze permits edits under `edgion-tests/**`.

## Requirements

- Start both listener paths with the default rejection policy and verify duplicate Host yields 400.
- Update the referenced EGC to the explicit `false` escape hatch without restarting the Gateway and verify a new request follows each path's normal behavior.
- Restore `true` and verify rejection resumes.
- Delete the EGC or make the reference missing and verify the safe `true` fallback.
- Keep request construction wire-accurate so the duplicate Host fields reach the Gateway unchanged.

## Scope boundary

Do not change production semantics in this task. Reuse the existing dynamic integration framework and preserve all unrelated scenarios.
