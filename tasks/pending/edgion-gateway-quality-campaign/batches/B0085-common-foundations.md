# B0085 — Common foundation and observability review

## Coverage and conclusions

Two independent reviewers read all 13 remaining source files and 13 associated
unit-test files, including nested modules, against original project knowledge.

- lib/observe/secret facades: exports and ownership boundaries are coherent.
- startup and tests: nested panic redaction and destructor re-entry isolation hold.
- test_mode: startup-only writes require no compound-state synchronization.
- time and tests: checked arithmetic and caller rejection semantics hold; document
  pre-epoch RFC3339 rejection accurately.
- tls_validation and three test files: complete PEM parsing, canonical CA bounds,
  key pairing, identity matching and deterministic status remain coherent.
- cardinality guard and tests: preserve documented soft cap and observe semantics;
  remove request-reachable tracing on first breach, retaining both alarm metrics.
- federation helpers and two test files: bounded labels confirmed in consumers;
  replace no-op-only coverage with local recorder sample/value/label assertions.
- logs and tests: protected targets remain closed; isolate the global subscriber
  in a child process, retain TempDir ownership, flush after joined producers and
  assert both emitted messages instead of sleeping.
- metrics_api and tests: document serial install idempotence without claiming
  competing initial calls both succeed; runtime behavior unchanged.
- system_metrics and tests: bounded reasons and independent rotation/identity
  ownership preserve state lifecycle.
- secret_store and two test files: atomic replacement, empty snapshots and
  diagnostics preserve ownership and redact boundaries; correct stale consumers.

All existing test function identities remain. No production visibility is widened.
No store, metric, configuration field or failure policy is introduced.

## Verification checkpoint

Observe tests: 26 passed. `cargo test -p edgion-common --lib`: 318 passed,
4 pre-existing ignored. Workspace all-target check and Clippy passed; format
applied. Repository script guards passed before this batch; affected docs,
metrics and tracing guards refreshed. Frozen integration paths remain unchanged.

Implementation and policy reviewers independently read the complete final diff.
The OIDC knowledge-page correction passed separate independent source review.
After volume recovery all eight content fingerprints still match. The interrupted
second final review completed with LGTM after original-rule and consumer checks.
Prior test results apply to unchanged content.
No live test result is claimed for the separate B0084 batch.

## Reviewed content fingerprints

Base: `99b85e31b`. SHA-256 identifies the reviewed working files (including
test and knowledge changes); runtime validation above applies to these contents.

```text
225c112543913b857b79ab24dc9066001192ad04e85193bcae614e08fb3050f4  edgion-common/src/observe/cardinality_guard.rs
0bcf4d251fcbab84cdadc4cf0cedd06342d96c56fc9645b468ed76fb4d308793  edgion-common/src/observe/metrics_api.rs
87df9e6a37e1b5b3538a9eb8e81fb4d0bbd0b8ab87df2cfd2fdffa7649f0b6b1  edgion-common/src/secret/secret_store.rs
db55d360b960422bfbee1a007e95b41eb15f851cfdd1957f5f501bc441850418  edgion-common/src/time.rs
0467c139f005eb4f2d964f822880b2f05379058651c27c813bd8ce15ed4e63c6  edgion-common/tests/unit/observe/fed_metrics_test.rs
5b65e4ae14a869c286f509a8928d61980c286974d1c723edccd6c9ef3a765308  edgion-common/tests/unit/observe/fed_metrics_test/rbac_metric_tests.rs
3ae8dc66241e10a4cc061a9037edb0957aacbefed016b6e54a6f30a6aa333926  edgion-common/tests/unit/observe/logs_test.rs
36a7067bdbcb7a1e8535019f9bdc7fd2c3e579de874728dd2d34c037f2ffe5ff  skills/04-review/oidc/intentional-design/session-secret-resolve-per-request.md
```
