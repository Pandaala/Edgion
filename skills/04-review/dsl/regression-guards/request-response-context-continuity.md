---
name: request-response-context-continuity
description: Review DSL context continuity across request and response phases and isolation between requests.
knowledge_type: regression-guards
---

# DSL request-to-response context continuity

## Conclusion and scope

DSL `ctx.set` and `ctx.get` share string variables within one proxied request,
including across request and upstream-response plugin stages. A subsequent
request must start without the previous request's variables. The contract is
documented in the [DSL context API](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md#context-storage-ctx).

The integration case `dsl_request_response_context` in
`edgion-tests/src/client/suites/edgion_plugins/dsl/context.rs` exercises this
contract through the shared Controller/Gateway/test_server topology. It changes
test coverage only, not production storage or DSL semantics.

## Rationale and evidence

- `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs` creates an
  `EdgionHttpContext` for each request. Its optional `PluginAuxState` owns
  `ctx_map: HashMap<String, String>`, accessed through `set_ctx_var`/`get_ctx_var`.
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` passes the context to
  the request adapter and the response adapter; DSL builtins delegate to these
  adapters rather than retaining variables on a shared plugin instance.
- The existing `07_EdgionPlugins_dsl-ctx-chain.yaml` reads context only inside
  `requestPlugins`. Setting a response header there does not test response-stage
  context reads. The unit test
  `test_ctx_get_reads_pre_existing_value_in_upstream_response` supplies a mock
  value directly, so it does not establish real lifecycle continuity either.
- The new `32_EdgionPlugins_dsl-request-response-context.yaml` writes fixed,
  non-sensitive markers in `requestPlugins` and reads them only in
  `upstreamResponseFilterPlugins`. The response reader emits an explicit
  `missing` sentinel for `nil`, so skipping the reader cannot pass a negative
  assertion. The write-control header is removed before proxying.
- One test case sends alpha, no-write, beta, no-write sequentially, asserting
  HTTP 200 and exact response markers after each request. Bodies are drained
  incrementally with a 64 KiB test cap, without aggregation.

## Alternatives and limits

This closes a coverage gap, not a demonstrated production data-loss or cross-request
leak. Existing scenarios are preserved. The client is reused and each response
is drained, but the test does not assert a physical connection identity or claim
concurrent HTTP/2 coverage. It does not test hot reload, access-log projection,
or response-body processing; no access-log fields or metrics are added.

## Verification and re-evaluation

Verified on 2026-09-11:

- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed; Clippy emitted existing
  warnings. `git diff --check` also passed.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source guards
  passed. Two unrelated stale documentation links had to be repaired for the
  agent-doc guard.
- `bash edgion-tests/integration/scripts/utils/prepare.sh` passed, rebuilding
  the product and test binaries before runtime execution.
- The standard `run_integration.sh --no-prepare -r EdgionPlugins -i Dsl`
  entry stopped at the macOS loopback-alias preflight (127.0.0.2 through
  127.0.0.7 were absent). It did not complete and is not recorded as passing.
- The existing `start_all_with_conf.sh --suites
  base,HTTPRoute/Basic,EdgionPlugins/base,LinkSys/Redis,EdgionPlugins/Dsl` plus
  `test_client -g -r EdgionPlugins -i Dsl --json` passed all 54 cases,
  including `dsl_request_response_context`, with zero infrastructure errors.
  The standard Redis Docker fixture was used. Controller/Gateway resource
  synchronization passed. Startup, client execution, and cleanup ran inside
  one shell session because the execution environment reclaimed background
  services after an earlier standalone startup command exited; that earlier
  client attempt failed to connect and is not a passing run.
- Successful local artifacts are under
  `tmp/integration_testing/testing_20260911_203351/`:
  `dsl-context-report.json`, `dsl-context-tests.log`,
  `dsl-context-startup.log`, and `logs/resource_diff.log` (gitignored).
- Independent read-only review approved the test and knowledge scope. No full
  integration sweep or full unit-test run is claimed. The manual targeted
  execution did not run the main runner's final metrics/residue gates.

Re-evaluate if context ownership, plugin-stage adapters, DSL context builtins,
or scenario registration changes.

## Resolution references

- `add-dsl-request-to-response-context-integration-coverage.md` (created
  2026-09-10, P2 test task): confirmed missing real request-to-response coverage;
  corrected with a new integration scenario and verified on 2026-09-11. No
  production context defect was established or changed.
