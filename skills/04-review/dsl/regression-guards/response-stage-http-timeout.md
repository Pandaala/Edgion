---
name: response-stage-http-timeout
description: Use when reviewing response-stage DSL async HTTP suspension and configured timeout enforcement in integration tests.
knowledge_type: regression-guards
---

# Response-stage HTTP suspension and timeout enforcement

## Conclusion and scope

The DSL response-header hook must drive an asynchronous `http.call` through
suspension and completion. The effective call timeout is the smaller of the
script's requested timeout and the plugin's `httpMaxTimeoutMs`. Transport timeout
returns the existing empty failure map with status zero, allowing the script to
continue and modify the response headers.

The integration case `dsl_response_http_timeout` covers this boundary through
the existing Controller/Gateway/test_server topology. This corrects a test gap;
no production timeout or synchronous-bridge defect was established or changed.

## Rationale and evidence

- [DSL engine bridging](../../../01-architecture/02-gateway/dsl-engine.md#4-async--sync-bridging)
  defines the response hook's `block_in_place` and `Handle::block_on` bridge.
  `edgion-gateway/src/plugins/http/dsl/plugin.rs` implements this hook.
- `DslConfig.http_max_timeout_ms` is resolved into immutable
  `DslRuntime.max_timeout_ms` at plugin construction. `call_builtin_http` in
  `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` clamps the script timeout
  before setting the outbound request timeout and awaiting `send`.
- [HTTP call semantics](../../../03-coding/dsl/http-call.md) specify the
  status-zero failure map and bounded outbound exchange. The main proxied
  response remains streaming.
- The existing `24_EdgionPlugins_dsl-http-call.yaml` runs in `requestPlugins`
  against `/health` with requested timeout 2000 ms and cap 3000 ms. It exercises
  neither response-stage suspension nor a requested timeout above the cap.
- The new `33_EdgionPlugins_dsl-response-http-timeout.yaml` uses two
  `upstreamResponseFilterPlugins` against test_server's `/delay/1`. A control
  with requested timeout and cap both 3000 ms must return HTTP 200 and the exact
  delayed body. A subsequent call requests 3000 ms with cap 10 ms and must
  return status zero, an empty body, and empty headers. The two plugins emit
  distinct fixed success/failure markers.
- `edgion-tests/src/client/suites/edgion_plugins/dsl/response_http_timeout.rs`
  requires both expected markers, proxied HTTP 200, and the exact complete `OK`
  body from `/health`, comparing chunks without aggregation. The case has a
  10-second client request timeout to bound a broken bridge.
- Local and K8s fixtures share the same route, scripts, and assertions; only
  the outbound destination changes from loopback to the existing test service.

## Alternatives and limits

The slow success control prevents an unavailable destination from satisfying
the test merely by returning status zero, and exercises real async suspension.
If the short cap is ignored, the subsequent call succeeds after one second and
emits the wrong marker. Skipping the response scripts leaves required markers
absent. A bridge failure cannot be hidden by the DSL error policy because the
expected markers are written only after the calls return.

This is a semantic timeout boundary test, not a scheduler precision benchmark:
it does not assert an exact 10 ms duration or exclude an unrelated transport
failure arising between the successful control and the capped call. It does
not prove every timeout value, hot reload, or concurrent HTTP/2 behavior.
Existing integration scenarios and production egress/client/body policies are
unchanged. No user documentation change is needed because the public contract
is unchanged.

## Verification and re-evaluation

Verified on 2026-09-11:

- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy reported existing
  warnings. `git diff --check` passed.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source guards
  passed. New local/K8s YAML parsed successfully and matched after replacing
  the loopback destination with the test Service hostname.
- `bash edgion-tests/integration/scripts/utils/prepare.sh` passed and built the
  product/test binaries and required ignored-test executables.
- The standard `run_integration.sh --no-prepare -r EdgionPlugins -i Dsl`
  passed its English-only, agent-doc, types-purity, and Bash preflights, then
  stopped because macOS lacked loopback aliases 127.0.0.2 through 127.0.0.7.
  That standard run did not execute tests and is not recorded as passing.
- The existing `start_all_with_conf.sh --suites
  base,HTTPRoute/Basic,EdgionPlugins/base,LinkSys/Redis,EdgionPlugins/Dsl` and
  `test_client -g -r EdgionPlugins -i Dsl --json` passed all 55 cases with zero
  infrastructure errors. The new case took approximately 1.03 seconds.
  The standard Redis Docker fixture and mTLS configuration-sync topology were
  used; resource synchronization passed. Startup, tests, and cleanup ran in
  one shell session. Product sources and existing fixtures were not altered.
- A temporary work-directory copy changed only the new capped plugin's
  `httpMaxTimeoutMs` from 10 to 3000. After publishing that copy through the
  Controller and observing the nonzero branch, the same test client returned
  nonzero: 54 passed and only `dsl_response_http_timeout` failed, explicitly
  reporting expected `timeout` versus actual `unexpected`. Infrastructure
  errors remained zero. The original fixture was reapplied, and both markers
  plus the complete `OK` body were checked again before service cleanup.
- Successful positive/negative artifacts are under
  `tmp/integration_testing/testing_20260911_210818/` (gitignored):
  `dsl-timeout-report.json`, `dsl-timeout-negative-report.json`, their test
  logs, `dsl-timeout-startup.log`, and `logs/resource_diff.log`.
  An earlier run under `testing_20260911_210725/` also passed all 55 cases,
  but its auxiliary negative-control HTTP poll disconnected before assertions;
  the complete retry above explicitly disabled proxy pickup for that poll.
- Independent read-only review approved the implementation and knowledge
  scope. K8s fixtures were checked statically, not executed on a cluster.
  No full unit-test or integration sweep is claimed. Manual targeted execution
  did not run the standard runner's final metrics/residue gates.

### Current-worktree revalidation (2026-09-11)

The implementation and task deletion were already present in `61c0455e` when
this follow-up resumed. The earlier artifacts listed above were absent from
this worktree; those historical results were not treated as fresh verification.

- Fresh format, workspace check/Clippy, agent-doc, SSA-force, metrics-inventory,
  Pingora-source, and diff checks passed. Clippy retained existing warnings.
  The agent-doc guard initially found an unrelated missing task link in
  `header-modifier-validation.md`; the link now preserves the historical
  identifier and explicitly does not claim the deferred work was completed.
- `prepare.sh` rebuilt all integration binaries and auxiliary executables.
  An initial run with stale binaries failed capability inventory validation;
  after rebuilding, the standard runner passed its preflights but stopped at
  the missing macOS loopback aliases. Neither attempt is a passing integration run.
- The existing shared-topology startup and DSL client commands listed above
  passed 55/55 cases with zero infrastructure errors. The new case took
  1.016 seconds. A temporary fixture with cap 3000 instead of 10 produced
  exactly one expected failure: `dsl_response_http_timeout` reported expected
  `timeout` versus actual `unexpected`; the other 54 cases passed. The original
  fixture was restored and both markers and the complete `OK` body rechecked.
- Fresh artifacts are in `tmp/integration_testing/testing_20260911_225936/`:
  `dsl-timeout-report.json`, `dsl-timeout-negative-report.json`, the matching
  test logs, `dsl-timeout-startup.log`, and `logs/resource_diff.log`. An earlier
  startup in `testing_20260911_225904/` lost its test_server before readiness;
  no test ran there, and the available log did not establish the exit cause.
- Independent read-only review approved the implementation and knowledge.
  Local/K8s fixture parity was checked statically. No cluster execution, full
  test sweep, or standard runner final metrics/residue checks are claimed.

Re-evaluate if the response hook, async VM dispatch, timeout resolution, failure
map, slow endpoint, or scenario registration changes.

## Resolution references

- D0139 / `add-dsl-response-stage-http-call-timeout-integration-coverage.md` (created
  2026-09-10, P1 test task): confirmed test gap; the user authorized adding
  coverage. Corrected with a new integration scenario and verified on
  2026-09-11; no production behavior change was needed.
