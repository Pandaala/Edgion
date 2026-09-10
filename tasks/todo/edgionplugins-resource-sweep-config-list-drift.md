# Fix EdgionPlugins resource sweep configuration-list drift

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-07-27 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

The `-r EdgionPlugins` integration sweep invokes suites whose configuration is
not consistently included in the resource-wide load list. Those suites then
receive HTTP 404 responses and make the sweep fail even though independently
loaded suites pass. Unify the execution and configuration inventories, improve
the partial configuration-load diagnostic, and add an omission guard so the two
hard-coded lists cannot drift again.

## Evidence

The body-buffering verification run on 2026-07-27 executed:

```text
./edgion-tests/integration/scripts/integration/run_integration.sh -r EdgionPlugins
```

`EdgionPlugins_DslBody` passed 6/6 and `EdgionPlugins_BodyPlugins` passed
19/19, while the overall resource sweep reported 10 failures. Most failures
returned HTTP 404 after the runner reported a partial configuration-load
failure. Affected suites included `Csrf`, `RateLimitRedis`, `RequestMirror`,
`Canary`, `IpRestriction`, `ExtProc`, `EdgionConfigDataStatus`,
`ResolvedRefs`, and `Cors`; `DslGrpcError` also failed and needs separate
diagnosis within this task.

## Current verification (2026-09-10)

- Static inspection and the `test_client` unit suite (22/22) confirm fixture
  directory coverage for the current EdgionPlugins resource sweep. The guard
  now distinguishes exact directories and recursive parents from name prefixes.
- Corrected `load_configs`: a loader failure could be masked by successful
  `tee`. The local pipefail boundary now propagates loader/log-sink failure and
  reports the failed suite and log path. Isolated regression checks pass.
- Workspace fmt, check, clippy (existing warnings), agent-doc, SSA-force,
  metrics-inventory, and Pingora source-policy checks passed.
- Full artifact preparation, including the external ExtProc fixture, passed.
- `run_all_integration.sh --no-prepare --full-test` ran all eight jobs in
  `tmp/integration_testing/run_all_20260910_124334`: Etcd, Elasticsearch, Kafka,
  ACME, CLI, and FileWatcher passed. Main stopped at missing macOS loopback
  aliases before executing its suite. OTLP exposed stale startup-count and
  shutdown-wait assumptions; the corrected marked-request delivery test passed
  12/12 on rerun in `tmp/integration_testing/run_all_20260910_125310`.
- Remaining gate: configure host aliases `127.0.0.2` through `127.0.0.7` (local
  sudo is required), then run the full main suite and the EdgionPlugins resource
  sweep. Historical `DslGrpcError` failures are not yet revalidated. Retain this
  task until live verification and independent review are complete.

Reusable evidence is maintained in:
- [Configuration failure propagation](../../skills/04-review/testing/regression-guards/mandatory-integration-gates-fail-closed.md)
- [Suite and fixture coverage](../../skills/04-review/testing/regression-guards/integration-suite-registrations-must-be-checked-per-sweep.md)
- [OTLP shutdown delivery](../../skills/04-review/testing/regression-guards/integration-suite-lifecycle-errors-must-fail.md)

## Requirements

- Keep the resource-wide suite invocation list and configuration-load list in
  sync.
- Diagnose and make the partial configuration-load failure actionable.
- Add a lightweight guard that detects any future inventory mismatch.
- Make the complete `-r EdgionPlugins` sweep pass in the supported gateway
  mode.

## Scope

### In scope

- EdgionPlugins resource-wide integration orchestration.
- Resource-wide configuration inventory and load diagnostics.
- Runner-level inventory consistency tests.

### Out of scope

- Body-buffering runtime behavior.
- Changes to the individual plugin implementations unless a separate defect is
  confirmed and recorded.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-tests/integration/scripts/integration/run_integration.sh` | Resource-wide suite and configuration inventories |
| `edgion-tests/src/client/test_client.rs` | Inventory omission guard |
| Integration configuration fixtures | Missing or failing resource-load inputs |
