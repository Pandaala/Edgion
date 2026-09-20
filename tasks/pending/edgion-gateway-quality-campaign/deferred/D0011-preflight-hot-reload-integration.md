# D0011 — Preflight Policy Hot-Reload Integration Coverage

## Meta

- Status: ready-after-campaign-freeze
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

## Canonical task

- `tasks/todo/add-preflight-policy-hot-reload-integration-coverage.md`
