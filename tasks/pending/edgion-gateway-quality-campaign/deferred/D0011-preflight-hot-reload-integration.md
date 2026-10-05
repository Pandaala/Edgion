# D0011 — Preflight Policy Hot-Reload Integration Coverage

## Meta

- Status: coverage-added
- Severity: low
- Discovered in: B0065, `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`
- Category: integration coverage

## Evidence

The B0065 correction can prove immutable snapshot publication, request pinning,
and preflight classification with crate-local tests. A cross-process scenario
that updates `EdgionGatewayConfig.spec.preflightPolicy` on an already running
listener would require modifying existing frozen integration registration.

## Why deferred

Existing `edgion-tests/**` files are frozen for this campaign. Adding the
scenario safely requires updating the integration inventory and runtime fixture
outside the campaign mutation lane.

## Outcome

- The `PreflightPolicyHotReload` dynamic scenario now covers the live update on
  one HTTP listener address and persistent connection. The user requested that
  tests not be run in this task, so runtime execution is not claimed.
- `skills/04-review/cors/regression-guards/preflight-policy-egc-hot-reload.md`
