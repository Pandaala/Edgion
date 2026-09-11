---
name: request-response-context-continuity
description: Review DSL context read ownership, allocation bounds, request-to-response continuity, and request isolation.
knowledge_type: regression-guards
---

# DSL context read ownership and request-to-response continuity

## Conclusion and scope

DSL `ctx.set` and `ctx.get` share string variables within one proxied request,
including across request and upstream-response plugin stages. A subsequent
request must start without the previous request's variables. The contract is
documented in the [DSL context API](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md#context-storage-ctx).

DSL `ctx.get` borrows the canonical request store only while checking the value
and constructing its independent `Value::Str(Arc<str>)`. Later context overwrite,
removal, or destruction must not alter that previously returned DSL value. A
missing key returns `nil`; an empty string stays a string; `maxStringLen` is
checked in UTF-8 bytes before allocating the VM string.

The integration case `dsl_request_response_context` in
`edgion-tests/src/client/suites/edgion_plugins/dsl/context.rs` exercises this
continuity contract through the shared Controller/Gateway/test_server topology.
That earlier task changed test coverage only. The subsequent context-read
optimization changes the DSL read path without changing storage or script semantics.

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
  `test_ctx_get_reads_pre_existing_value_in_upstream_response` supplies a
  pre-populated context through the mock, so it does not establish real
  lifecycle continuity either.
- The new `32_EdgionPlugins_dsl-request-response-context.yaml` writes fixed,
  non-sensitive markers in `requestPlugins` and reads them only in
  `upstreamResponseFilterPlugins`. The response reader emits an explicit
  `missing` sentinel for `nil`, so skipping the reader cannot pass a negative
  assertion. The write-control header is removed before proxying.
- One test case sends alpha, no-write, beta, no-write sequentially, asserting
  HTTP 200 and exact response markers after each request. Bodies are drained
  incrementally with a 64 KiB test cap, without aggregation.

### Borrowed read and independent VM ownership

- `PluginSession::ctx()` already exposes `&EdgionHttpContext`;
  `PingoraSessionAdapter::ctx` returns the same request context used by its
  `set_ctx_var`, `remove_ctx_var`, and owned `get_ctx_var` methods.
- Previously, DSL `CtxGet` used the owned session accessor, copying the stored
  string with `to_string()` before the VM checked its length and copied it into
  `Arc<str>`. The intermediate copy was unnecessary, including on over-limit reads.
- `lang/builtins.rs::CtxGet` now uses
  `checked_value_opt_str_ref(session.ctx().get_ctx_var(&key), limit)`. This helper
  reuses `check_string_len` and then creates the independent `Arc<str>` directly.
  The borrow ends before any later session mutation or suspension.
- Ordinary `ctx_value_tests` exercise the real context store and conversion:
  one allocation for a present nonempty value, zero for missing or over-limit
  values, exact UTF-8 byte boundaries, empty strings, and retained values after
  context overwrite/removal/drop. Mock builtin tests cover dispatch and errors;
  they do not pretend mock writes update a live backing store.
- This follows [design rules J/U/X](../../../00-design-rules.md) and the root
  `AGENTS.md` rule that data-plane performance outranks mock convenience. The
  existing `ctx()` reference return is mockable without a new trait method,
  another context store, or an owned production fallback.

## Alternatives and limits

The earlier integration task closed a coverage gap, not a demonstrated production
data-loss or cross-request leak. Existing scenarios are preserved. The client is reused and each response
is drained, but the test does not assert a physical connection identity or claim
concurrent HTTP/2 coverage. It does not test hot reload, access-log projection,
or response-body processing; no access-log fields or metrics are added.

The read optimization eliminates the intermediate `String`, not the final
copy into independent VM storage. Allocation assertions cover the synchronous
store lookup/conversion boundary, not total request allocations or measured
throughput. The owned `PluginSession::get_ctx_var` remains for its other
consumers; its signature, every session implementation, and `edgion-tests/**`
are unchanged. No string-type migration is needed; the
[SSO migration decision](../intentional-design/sso-migration-not-worth-it.md)
does not defer this narrower optimization. Access-log declarations, blocklists,
redaction, and plugin-stage lifecycle remain unchanged.

## Verification and re-evaluation

Historical continuity-task verification on 2026-09-11, before the borrowed-read
change (not fresh verification of that optimization):

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

Borrowed-read verification on 2026-09-11:

- `cargo test -p edgion-gateway --lib plugins::http::dsl::` passed all 398 tests,
  including four ordinary ownership/allocation tests and seven context builtin
  wiring/error tests. The measured conversion allocates once for nonempty
  accepted values and zero times for missing or rejected values.
- `cargo fmt --all`, `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets`, and `git diff --check` passed.
  Clippy emitted existing warnings; no primary diagnostic location was on an
  added or modified line. This is not a warning-free workspace claim.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source guards
  passed. Agent-doc validation initially found an unrelated missing task target;
  the existing header-modifier knowledge page's deferred preparation work was
  restored as `tasks/todo/prepare-gateway-api-header-modifier-runtime.md`, without
  changing header behavior or claiming that follow-up resolved.
- Independent read-only code and knowledge review returned LGTM, including the
  restored follow-up record. No full-workspace test or new integration execution
  is claimed for this optimization; existing `edgion-tests/**` files are unchanged.

Re-evaluate if context ownership, plugin-stage adapters, DSL context builtins,
or scenario registration changes. Also re-evaluate if an adapter's `ctx()` stops
exposing the store used by context writes, or the conversion regains an owned
intermediate, changes its limit/error policy, or retains a host borrow in VM values.

## Resolution references

- `add-dsl-request-to-response-context-integration-coverage.md` (created
  2026-09-10, P2 test task): confirmed missing real request-to-response coverage;
  corrected with a new integration scenario and verified on 2026-09-11. No
  production context defect was established or changed.
- `avoid-dsl-ctx-get-string-copy.md` (created 2026-09-10, P2 performance task):
  confirmed unnecessary intermediate string copy; corrected through the existing
  borrowed context accessor and verified on 2026-09-11.
