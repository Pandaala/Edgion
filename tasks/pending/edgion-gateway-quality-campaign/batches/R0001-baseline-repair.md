# R0001 - Restore baseline verification

- Status: analysis-in-progress
- Baseline: 75c4d51ecd195193ba6a91d210736c0be4e4676f
- Authority: user explicitly requested fixing baseline failures before continuing pruning on 2026-10-29.
- Scope: six independently diagnosed root causes, D0002-D0004 and D0006-D0008. Exact code write allow-list pending technical plan and independent approval.
- Analysis roles: /root/repair_controller_analysis and /root/repair_gateway_analysis (read-only).
- Protected: original impl/, untracked specification, unrelated files.
- Required: preserve all meaningful test identities/assertions and production limits, independently review a concrete plan, implement in one serial lane, independently review final diff, run targeted regressions and required workspace/pre-commit checks, then local commit without push.
- No code edits yet. Existing pruning B0001-B0003 remain queued, to be revalidated after this repair commit.

## Controller/shared analysis

`/root/repair_controller_analysis` independently revalidated all three causes and original contracts. Proposed code scope: `edgion-resources/tests/fixtures/policy_precedence_wire.json`, `edgion-controller/tests/unit/conf_mgr/conf_center/etcd/leader_election_test.rs`, `edgion-controller/tests/unit/observe/unknown_fields_registry_test.rs`. No production edits.

- Add unrestricted preparedAncestorAdmission only to six wire objects, preserving source fixtures and exact wire/list/watch/arbitration assertions. ARCH-106 requires this field independently of observed implementation.
- Match only Ok(()) from the mock win channel, preserving shutdown/win, diagnostic drop and starts==1; sender closure is not a win.
- Preserve metric test identity, exact HELP/TYPE metadata, and duplicate-preserving sorted sample Vec compared with exact expected identities/values. Canonical exporter has unspecified sample order; no production sorting, sets, filtered unknown samples or count-only assertion. Historical canonical TestRecorder migration already normalized a neighboring test.

Verification: Controller/Gateway policy_precedence groups, Controller election and unknown-field registry modules, repeated repaired race/order tests, then full workspace/default parallel tests and quick gates. Implementation role `/root/r0001_impl` is preparing a combined plan only; Gateway analysis and independent plan approval remain required.

## Frozen implementation plan (pending independent review)

Implementation role `/root/r0001_impl`; plan reviewer `/root/r0001_plan_review`. Complexity: complex. No edits until approval.

1. Preserve original Gateway/Controller compiled test discovery lists and user artifacts.
2. Repair six fixture wire specs only, election successful-delivery pattern, and exact duplicate-preserving metric sample/metadata assertions.
3. Wrap 99 actual-preparation Wasm tests with existing exact-case subprocess helpers (38 plugin, 51 vm_manager, 3 request, 5 attached, 2 callout); retain existing five isolated capacity tests and pure tests. No admission/production changes.
4. Isolate five remaining direct TLS store fixture writers with the same helper, retaining cleanup and all identity/digest/precedence assertions. Production handler writers already serialize through GatewayStore; no store implementation change.
5. Update only proven stale sizes and three owning knowledge pages: RequestInfo432-24(query ownership)-8(borrowed protocol)=400; PluginExecutionState177 field bytes+7padding=184. Probe succeeded; Aux296, OptionBox8 and context1376<1800 retained.
6. Preserve test identities and meaningful assertions; run focused/default-parallel stress tests, full workspace suite and quick checks; obtain independent final review before local commit. Stop on scope/behavior deviation, new failures or changed identity.

### Exact code and knowledge allow-list

- edgion-resources/tests/fixtures/policy_precedence_wire.json
- edgion-controller/tests/unit/conf_mgr/conf_center/etcd/leader_election_test.rs
- edgion-controller/tests/unit/observe/unknown_fields_registry_test.rs
- edgion-gateway/tests/unit/plugins/http/wasm/plugin_test.rs
- edgion-gateway/tests/unit/plugins/http/wasm/vm_manager_test.rs
- edgion-gateway/tests/unit/plugins/http/wasm/request_test.rs
- edgion-gateway/tests/unit/plugins/http/wasm/attached_test.rs
- edgion-gateway/tests/unit/plugins/http/wasm/callout_test.rs
- edgion-gateway/tests/unit/backends/peer_builder_test.rs
- edgion-gateway/tests/unit/backends/peer_builder_test/pool_tests.rs
- edgion-gateway/tests/unit/ctx_test/plugin_request_state_tests.rs
- edgion-gateway/tests/unit/ctx_test/request_scenario_state_tests.rs
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/tls/regression-guards/backend-tls-pool-isolation.md
- skills/04-review/cpu-memory/intentional-design/http-request-context-layout-budget.md

### Evidence and verification contract

Native layout probe `/private/tmp/edgion-r0001-layout-probe.log` exited0 in3m11s: RequestInfo393fields+7pad=400, PluginExecution177+7=184, PluginAux289+7=296, OptionBox8, Context1372+4=1376 (alignment8). Provenance: 6118e8e2 ip_body_denied, 25e0fefda borrowed protocol, 7b1755089 moved raw-query ownership.

104 new child launches add test startup overhead. Existing helper uses exact case selection/environment marker, executes original body only in child, and verifies successful exit and exactly one completion marker. Harness default parallelism remains enabled; serial-only success is insufficient. Preserve intra-test concurrency/cancellation/exhaustion checks and production four-permit limit.

Baseline test identities: `/private/tmp/edgion-r0001-gateway-test-identities-before.txt`, `/private/tmp/edgion-r0001-controller-test-identities-before.txt`. No deletion/merge/rename authorized. Relevant API filter is api:: (not api::tests). Run both policy_precedence groups, Controller election/unknown-fields, Gateway Wasm/peer-builder/ctx; repeat concurrency regressions and full default-parallel workspace tests.

## Independent plan review

`/root/r0001_plan_review`: Approved, no Must Fix. Independently checked original contracts, exact-case child execution, production writer serialization and native layout evidence. Implementation authorized for the frozen15 paths only. Final acceptance additionally requires unchanged test identities/bodies, default-parallel suite success, exact scope, all preparation callers covered and documented child-process cost/native measurement scope. `/root/r0001_impl` began phase2 after approval.

## Implementation checkpoint

All15 source/knowledge paths are frozen after formatting; no production source changed. `/private/tmp/r0001_verify_bodies.py` restores104 wrapper bodies and canonicalizes complete original/restored files with rustfmt: all seven files match exactly, preserving attributes, helpers, literals and assertions. Parsed fixture comparison after removing only the six new fields matches HEAD. Independent final reviewer `/root/r0001_final_review` is checking current diff and original contracts.

Fresh static guards passed: agent docs, SSA force,102-metric inventory, Pingora source policy, Gateway API transform contract and gateway tracing; unit-test layout is still running(session52123). Targeted Controller compile/test session5163 runs under implementer. Full workspace tests/check/clippy remain pending.

## Focused validation

Controller policy3pass, election5pass/5existing ignored, unknownfields19pass. Repaired election and metrics test each passed20 repetitions. Entire2048-test discovery output is byte-identical to baseline. `/private/tmp/edgion-r0001-controller-focused.log`. Final reviewer requested one doc symbol correction (GatewayHandler), implemented and reviewed; no production change. Fresh formatting and all seven static guards passed, including1113 mounted test files across9crates. Full cargo test --all now runs as session91777; workspace check/clippy pending.

## Independent final review

`/root/r0001_final_review`: LGTM, no remaining Must Fix. Independently checked all15 files, original contracts,104 body-preservation comparisons, exact scope, subprocess child completion, production TLS writer serialization, six fixture fields and native layout evidence. Corrected doc symbol verified. This review does not waive pending full-suite, Gateway discovery or workspace check/clippy gates.

## First full-suite result

Full suite session91777 exited101. Controller library passed2042/0failed/6ignored. Gateway ran default-parallel in9.57s:6419passed,1failed,19ignored. All previously diagnosed failure cases passed; sole remaining failure is untouched response_cache_first_actual_tail_binding_and_consumed_peer_fill_then_hit_and_input_miss (expected fill1, observed0). Fresh read-only diagnostic `/root/r0001_remaining_diagnostic` is investigating attribution and minimal repair scope; no new edit authorized yet. Complete Gateway6439-test discovery is byte-identical to baseline. Log `/private/tmp/edgion-r0001-full-tests.log`; workspace check session7546 running independently. No claim of complete baseline success.

## Response-cache plan addendum (review pending)

Fresh independent diagnosis proves existing test interference: original binary7/8 and current18/20 mixed upstream-peer runs fail; isolated12/12 passes. Concurrent traffic-policy generation changes between tail binding, consumed peer, fill and next lookup correctly invalidate cache evidence. A second fixture fails2/8; a third has the identical unisolated lifecycle. No production failure semantics may change.

Proposed extra file: `edgion-gateway/tests/unit/routes/http/proxy_http/pg_request_filter_test/response_cache_tests.rs`. Three canonical async subprocess wrappers preserve original function bodies, identities, TLS locks and all assertions. Explicit late-reload/discard tests stay unchanged. `/root/r0001_impl` prepares addendum; `/root/r0001_plan_review` independently reviews before edit. Expected total107wrappers/16files. Verify mixed group at least8times, full response-cache module, body/discovery equivalence and complete suite.

Workspace check passed58.95s and Clippy passed44.94s with exactly existing advisory warnings in unchanged files. Logs `/private/tmp/edgion-r0001-{check,clippy}.log`; rerun affected gates after addendum edits.

Response-cache addendum independently Approved by `/root/r0001_plan_review`, no Must Fix. Reviewer verified original/current logs, all three bodies, shared expected/actual stamp checks and key generation. Third GraphQL case is code-proven equivalent vulnerability, not claimed reproduced failure. Exact allow-list now adds only the named response_cache_tests.rs file (16 total); implementation authorized after approval.107 wrapper bodies and explicit reload/discard preservation require final verification.

## Addendum validation

Independent final reviewer re-reviewed and returned LGTM with no Must Fix. Entire8-file/107-test body restoration matches HEAD. Response-cache module11/11passed; eight default-parallel mixed upstream-peer/cache runs each109/109passed. Gateway6439-test discovery remains byte-identical. Logs `/private/tmp/edgion-r0001-response-cache-{focused,mixed-after}.log`. All seven static pre-commit guards rerun after final edit passed. Final full-suite run session6638, log `/private/tmp/edgion-r0001-full-tests-final.log`, is in progress.

## Second full-suite result and remaining-target sweep

Session6638 exited101 at newly reached integration target listener_tls_tcp_alpn: tls_to_tcp_builder_does_not_advertise_http_alpn failed TLS handshake. Gateway default-parallel library fully passed6420/0failed/19ignored in9.55s; Controller fully passed2042/0failed/6ignored. Fresh diagnostic `/root/r0001_alpn_diagnostic` investigates original contracts before any new edit. Workspace check after cache addendum passed; Clippy session45251 running.

An unchanged-source `cargo test --all --no-fail-fast` sweep(session73683, `/private/tmp/edgion-r0001-remaining-inventory.log`) now discovers later failing targets instead of repeatedly stopping at the first. This changes only failure aggregation, not test assertions/inputs or acceptance; overall failure remains failure. New scope requires independently reviewed plan.

## ALPN fixture addendum and complete remaining-target inventory

No-fail-fast sweep73683 completed exit101; eight targets failed: Controller lib(region failover shared-state test), listener_tls_tcp_alpn, tls_passthrough_clienthello, Resources crd_pattern_canonical/waf_supported_crs, Tests lib(plugin-policy fixture), test_client(default sweep inventory), waf_supported_fixture. Read-only `/root/r0001_sweep_diagnostic` investigates remaining seven while ALPN proceeds independently within this same serial batch.

ALPN diagnosis proves absent resolved_attachment_proof produces unresolved sentinel and actual early listener eligibility rejects the candidate. Independent plan reviewer Approved exact two new paths: `edgion-gateway/tests/listener_tls_tcp_alpn.rs`, `skills/04-review/tls/regression-guards/https-http2-alpn-policy-handoff.md`. Set explicit valid Controller-publication fixture proof, reuse canonical returned attachment proof, assert is_resolved and actual snapshot predicate selection. Preserve actual Pingora handshake and ALPNNone, all admission/cert/deadline/cleanup assertions. Fixed digest models transport input, not Controller digest/admission verification. Total18files/107wrappers. Implementer proceeding after sweep completion and approval.

ALPN focused integration passed and rebuilt executable passed three additional real-handshake repetitions. Existing single test identity unchanged; logs `/private/tmp/edgion-r0001-alpn-{focused,repeats}.log`.18-path source frozen and no build processes remain. Independent final reviewer checking this addendum. Remaining-target diagnosis split: WAF reproducibility/digest fixtures owned by `/root/r0001_waf_fixture_diagnostic`, other five failures by `/root/r0001_sweep_diagnostic`; both read-only.

## Five-root addendum (plan review pending)

Implementation plan independently derived from diagnostic evidence; no edits yet. Add14paths, total32. MetricsLifetime remains a separately planned addendum in this serial repair batch.

1. Rebuild RV floor from actual seed values for six direct failover fixture seeds, no allocator reset or changed assertions.
2. Publish processed Gateway listener and TLSRoute parent statuses with consistent generation, preserving TLS socket/negative/replay/SSRF/log tests.
3. Extract exact reference group/kind grammars into shared constants; schema owners and canonical inventory reference them, with no CRD or runtime grammar changes.
4. Resolve PluginPolicy WAF fixture from actual policy/bundle inputs, independently verify references/profile/digest and preserve all prior validation assertions.
5. Narrow WAF plugin serializer quotes only complete on-mode line for finite plugin-map shape; input/output Value equality and one-match assertion guard accidental changes. Preserve core/tuning/order/digests/guards and existing acceptance checks. Regeneration permits exactly four derived-file changes; acceptance JSON only plugin output filehash.
6. Run targeted modules, socket integration, schema/canonical guards, generator check and both WAF targets; stop on unexpected generated diff or behavior/scope change.

Exact additional files:
- edgion-controller/tests/unit/api/region_route_handlers_test/failover_version_tests.rs
- edgion-gateway/tests/tls_passthrough_clienthello.rs
- skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md
- edgion-resources/src/gwapi_types.rs
- edgion-resources/src/resources/common/outbound_tls.rs
- edgion-resources/src/resources/gateway.rs
- edgion-resources/tests/crd_pattern_canonical.rs
- edgion-tests/src/client/dynamic/scenarios/plugin_policy.rs
- edgion-resources/tests/common/supported_crs.rs
- edgion-resources/tests/waf_supported_crs.rs
- config/waf/crs-4.25.1-supported-v1/generated/EdgionPlugins_crs-supported.yaml
- config/waf/crs-4.25.1-supported-v1/generated/acceptance.json
- edgion-tests/integration/conf/EdgionPlugins/Waf/DynamicTest/updates/to_supported_on/EdgionPlugins_waf-dynamic.yaml
- edgion-tests/integration/conf/EdgionPlugins/Waf/DynamicTest/updates/to_supported_standard/EdgionPlugins_waf-dynamic.yaml

Independent reviewer `/root/r0001_plan_review` is evaluating before mutation. User explicitly allows evidence-backed integration fixture repair, overriding default skill freeze.

Five-root14-path addendum independently Approved, no Must Fix. Implementation authorized; exact reference grammar and unchanged schema, RVfloor semantics, real WAF fixture projection and narrow double-quoted renderer were verified against original contracts.

## MetricsLifetime plan (separate approval pending)

Exact five proposed paths: new `edgion-tests/integration/scripts/integration/test_metrics_lifetime.sh`; `edgion-tests/integration/scripts/utils/isolated_ports.sh`; `edgion-tests/integration/scripts/integration/run_integration.sh`; `edgion-tests/src/client/test_client.rs`; `skills/05-testing/02-cache-residue-detection.md`.

Reuse default isolated stage before shared topology, existing actual MetricsLifetime client scenario/fixtures, process-local TTL2/cap64 profile, registered ports and preflight, owned minimalFScontroller/gateway/backend, canonical mTLS/readiness helpers, explicit test-mode env confirmation, inherited instrumented binaries/LLVM env and bounded owned-PID cleanup. Do not kill existing services or use nested main runner/start_all.

Capability guard must verify fail-closed actual default invocation, name/command mapping and real scenario call; negative mutations remove coverage when mapping/invocation/ID/failure propagation is broken. KNOWN_TARGETED_ONLY remains empty; no coverage waiver. Required real integration and default-path wiring verification cannot be replaced by static string presence. Independent reviewer currently assessing exact implementation/validation contract.

## Metrics plan approval and exact acceptance

`/root/r0001_plan_review` Approved the five MetricsLifetime paths with no Must Fix after refinements: minimal matching EGC/Namespace/Class and original Gateway seeded before startup; authenticated metrics/admin endpoints; canonical artifact_paths resolver preserving instrumented binaries/overrides/LLVM; Gateway shutdown before Controller/backend within5s normal or60s coverage, unexpected exit/forcedkill fails; actual default failclosed invocation/mapping/scenario command with negative mutation checks and empty exemption list. Total approved scope37paths (32previous+5Metrics).

Direct standalone script and actual untargeted default run_integration.sh are required. No -r/-i; --suites is not an isolated-only selector. No --full-test opt-ins required. Partial/blocked default integration cannot be called passing. Script must preflight occupied ports and never terminate existing services. All edits remain serialized under `/root/r0001_impl`; no separate implementation lane.

## Resumed verification

Independent final review approved all 37 paths after the MetricsLifetime script gained project-root working-directory normalization and the capability guard rejected conditional or unused-function scenario wrappers. Latest workspace test build completed successfully. Format, docs, SSA, metrics inventory, Pingora policy, Gateway API transform, unit-test layout, gateway tracing, shell compatibility and Bash arithmetic guards passed.

Fresh direct TLS passthrough verification still fails the initial allowed-generation backend dial with `no match`; the prior fixture repair is therefore not accepted yet. Implementation is frozen while the implementer diagnoses and the independent plan reviewer revalidates the missing fixture contract. Other focused tests and the full no-fail-fast workspace sweep continue. Canonical integration preparation is running. Default integration additionally awaits macOS loopback aliases 127.0.0.2 through 127.0.0.4; OS sudo requires the user's local password, and no password is requested or collected. No repair commit or pruning promotion has occurred.

TLS follow-up plan and final review approved explicit canonical local-bind annotation and public listener-index preconditions; no production visibility changes or security assertion removal. Metrics follow-up plan and final review approved exact inline scenario-enable flag and canonical scenario identity guard with missing/zero flag negatives. Real source and runtime evidence show Gateway always spends five seconds draining auxiliary services after Pingora, so this owned fixture uses a ten-second normal shutdown deadline; coverage remains sixty seconds and forced termination remains failure.

The first complete resumed workspace sweep ended with only TLS passthrough and test_client capability failures; all other targets passed. Both causes are repaired, and a fresh full sweep is rebuilding. The corrected MetricsLifetime standalone script ran from /private/tmp successfully (exit 0), including actual scenario execution and owned-process cleanup. Evidence: /private/tmp/edgion-r0001-metrics-standalone-verified.log. Required default integration remains pending, not waived.

## Final resumed verification and external prerequisite

- `cargo test --all --no-fail-fast`: exit 0; 13,152 passed, zero failed, 67 ignored across 78 successful target summaries. Log: /private/tmp/edgion-r0001-full-tests-verified.log.
- Latest workspace check, Clippy (existing advisory warnings), fmt check, docs, unit layout and prior unchanged static guards passed. Check/Clippy logs: /private/tmp/edgion-r0001-{check,clippy}-verified.log.
- TLS complete real-socket/security/parser matrix passed three direct repeats; capability checks passed 31/31, with binary freshness verified.
- Canonical integration prepare completed exit 0; standalone MetricsLifetime passed actual scenario and cleanup from /private/tmp.
- First default attempt encountered transient occupied ports during concurrent tests; no process was manually terminated. After all tests completed, the serial untargeted default run passed ports and static preflights and failed the documented macOS alias prerequisite. Exact missing aliases: 127.0.0.2 through 127.0.0.7. Log: /private/tmp/edgion-r0001-default-integration-serial.log.
- Independent diagnostic review confirms sudo requires a local password and no prepared alternate Linux environment exists. User was asked to configure six aliases locally. Default integration remains pending/environment-blocked, not waived or passing. No commit, closure of repair tasks, or pruning promotion until this required gate succeeds. All parent and implementer test/build processes have completed.

## Loopback prerequisite resolved

User configured all six aliases; local ifconfig confirms 127.0.0.2 through 127.0.0.7. Resumed untargeted default integration with prepared binaries (log /private/tmp/edgion-r0001-default-integration-ready.log). All isolated scenarios, including MetricsLifetime, passed; shared gateway tests are running. No source or HEAD drift and no commit yet.

## Default integration uncovered stale DSL fixture contract

Untargeted default integration completed: 290 groups, 278 passed, one failed, 11 existing skips, zero timeouts. All isolated scenarios and final residue/metrics/requeue guards passed. Only `dsl_freeze_query_param` failed. Report: tmp/integration_testing/testing_20261029_111645/report.log.

Independent baseline diagnosis confirms this fixture and test predate R0001 unchanged: ordinary DSL req.query reads current logical query after set_uri; explicit original request URI remains frozen. The current authority supersedes the old entry-frozen ordinary-query expectation. No production change is proposed. Revised plan is under independent review for three exact additions: local and K8s `EdgionPlugins/Dsl/26_EdgionPlugins_dsl-freeze-query.yaml` fixtures and `edgion-tests/src/client/suites/edgion_plugins/dsl/dsl.rs`. The shared client requires both mirrored fixtures to assert before/current/entry views consistently. Total proposed implementation scope becomes 40 paths only after approval.

DSL three-path plan and final review both approved. Scope is now exactly 40 implementation paths. Preserve original test/resource identities; four exact echoed lines validate warmed pre-edit query, current post-edit query, frozen entry URI and current URI. Both local/K8s fixtures remain identical. Latest canonical prepare, workspace check, Clippy with existing warnings, formatting and docs passed. Targeted EdgionPlugins/Dsl integration completed exit 0 including dynamic query lookup and final cache residue guards; log /private/tmp/edgion-r0001-dsl-targeted.log. Final untargeted default integration is running at /private/tmp/edgion-r0001-default-integration-final.log.

## Accepted final outcome

Final untargeted default integration completed exit 0: 290 groups, 279 passed, zero failed, zero timed out, 11 existing default skips. Report: tmp/integration_testing/testing_20261029_113901/report.log. Log: /private/tmp/edgion-r0001-default-integration-final.log. All isolated scenarios and final metrics/unknown-fields/requeue/cache-residue guards passed; owned services cleaned up. The earlier full workspace sweep passed 13,152 tests; the subsequent DSL fixture/client change passed fresh workspace check/Clippy, fixture parse/mirror checks, targeted DSL and the full default integration. Independent final review has no remaining Must Fix.

Independent closure review authorizes removing six completed campaign-created task files D0002/D0003/D0004/D0006/D0007/D0008; historical identities remain in the deferred index. D0001 advisory-documentation policy and D0005 duplicate-field accounting contract remain open. Protected user spec and impl directory remain untouched. R0001 is accepted for local commit, not a pruning coverage claim.
