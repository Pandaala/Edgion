---
name: csrf-secret-rotation-integration-coverage
description: Use when reviewing CSRF Secret-rotation findings that claim missing ref/requeue logic or missing end-to-end coverage of signing-key generation replacement, deletion fail-close, and recovery.
status: fixed
finding_id: D0042
closed: 2026-09-06
---

# CSRF Secret Rotation Has Shared Ref/Requeue Logic Plus Dedicated Integration Coverage — FIXED

## Conclusion

CSRF already uses the shared Secret `ref` and cascading-requeue pipeline. D0042 was a
test-gap only: a standalone dynamic scenario now proves signing-key generation
replacement, pre-rotation token rejection, deletion fail-close, and recovery.

Fix suggestions of the form "add a CSRF-local Secret watcher", "rewrite the existing
static CSRF suite to manufacture rotation", "use a fixed sleep instead of phase
polling", or "log token or key values in fixtures or failures" are **not accepted**.

## Core Rationale

**1. Production Secret lifecycle is already shared.**

`resolve_csrf` calls `fetch_secret`, which registers `SecretRefManager` before lookup.
`SecretHandler` requeues dependent `EdgionPlugins` on update and delete. Gateway
construction pins `resolved_key`; an empty key seals fail-closed with 500.

**2. The missing proof was CSRF-specific token generations.**

Static CSRF coverage only exercises a never-created Secret. HMAC and AI-backend
rotation scenarios prove the same pipe for other credentials. CSRF tokens are
server-signed with the current generation, so only a CSRF scenario can show that a
pre-rotation token becomes 401 after the new generation is live.

**3. The scenario stays standalone.**

`CsrfSecretRotation` lives under `EdgionPlugins/Csrf/DynamicTest` with unique
resource names. Existing `Csrf` static cases are unchanged except for registry
wiring required to list the new scenario.

## Fix Suggestions Not Accepted

- CSRF-local Secret watch or a second requeue path — the shared pipeline is the canon
- Rewriting `edgion-tests/src/client/suites/edgion_plugins/csrf/csrf.rs` to add rotation
- Fixed sleeps or `wait_resource_sync` instead of `wait_phase`
- Dual-key overlap, duration-string `expires` migration, or D0041 reopening

## Re-evaluation Triggers

Re-open this decision only if:

- CSRF stops registering through `fetch_secret` / `SecretRefManager`
- PluginStore generations keep serving a previous signing key after Secret rotation
- Deleting the referenced Secret no longer seals fail-closed with 500

## Reference Cases

- Closed D0042 and `tasks/todo/add-csrf-secret-rotation-integration-coverage.md`
- Scenario: `edgion-tests/src/client/dynamic/scenarios/csrf_secret_rotation.rs`
- Closest sibling: `RequestAccessPolicyHmacDynamic`
