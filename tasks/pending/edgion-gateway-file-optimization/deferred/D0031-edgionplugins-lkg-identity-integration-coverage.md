# D0031 — Add EdgionPlugins LKG Identity Integration Coverage

## Meta
- Status: blocked
- Severity: high
- Discovered in: B0062, HTTP plugin framework audit
- Category: testing
- Canonical task: [`tasks/todo/add-edgionplugins-lkg-identity-integration-coverage.md`](../../../todo/add-edgionplugins-lkg-identity-integration-coverage.md)
- Dependency: D0030 / EdgionPlugins cross-source generation identity design and implementation

## Evidence
Colocated tests prove Gateway store semantics for explicit add, update, and full-set calls, but cannot prove resource identity survives the real source, event compression, conf-sync, reconnect, and Gateway publication chain. Existing integration coverage does not establish parity between an invalid same-object update and an invalid same-key recreation across reconnect.

## Why deferred
The scenario must consume the identity contract selected by D0030. Adding it first would encode guessed Kubernetes-only behavior, while modifying current integration files would violate the campaign's frozen-test contract.

## Candidate coverage
Add a new dynamic scenario that publishes a valid plugin, proves an invalid same-object update retains it, proves an invalid same-key recreation replaces it fail-closed, reconnects or relists to prove full-snapshot parity, and then proves valid recovery. Reuse one shared topology and `wait_phase` or state polling; do not use fixed sleeps.
