# Remove TLSRoute Stage-1 Fallback Coherently

## Meta

- Status: pending
- Type: refactor
- Priority: P2

## Evidence

`edgion-gateway/src/routes/tls/mod.rs` dispatches Stage-1 plugins when the
TLSRoute Stage-2 runtime is empty. Stream-plugin development architecture labels
this as removable pre-release compatibility debt. Frozen TLSRoute/StreamPlugins
fixtures `01_EdgionStreamPlugins_deny.yaml` and
`02_EdgionStreamPlugins_allow_localhost.yaml` still use only `spec.plugins` and
depend on that fallback. Deleting the branch in isolation silently changes deny
into allow, so this is a coordinated migration rather than a local cleanup.

## Acceptance Criteria

- Define the TLSRoute-only `tlsRoutePlugins` contract, including Stage-1-only
  configuration handling, with no silent fail-open migration.
- Remove the fallback and its special GeoIP filtering when the contract and
  fixtures are migrated; replace rather than silently discard required coverage.
- Update owning architecture, observability guidance and user-facing bilingual
  documentation consistently; do not retain an unrequested compatibility shim.
- After separate integration authorization, migrate existing fixtures and verify
  Terminate/Passthrough deny/allow, missing-resource fail-close, empty/disabled/
  Bypass behavior and dynamic updates in the canonical shared topology.
- Keep existing integration files unchanged unless their exact paths are explicitly authorized.
