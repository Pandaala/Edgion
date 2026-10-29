# Verification issues

Current status: the following sections are historical baseline evidence. All recorded baseline failures were repaired and verified by R0001; see final resolution below.

## Baseline full-test failure (diagnosis complete)

- Command: `cargo test --all`
- Baseline: `75c4d51ecd195193ba6a91d210736c0be4e4676f`; no tracked edits existed.
- Session 4176 completed with exit 101; compile took 7m20s.
- Controller library: 2,038 passed, 4 failed, 6 ignored. Cargo stopped before remaining workspace suites, so Gateway behavioral baseline is not established.
- Full local output: `/private/tmp/edgion-pruning-baseline-tests.log`.
- B0001 implementation remains held with both allowed files unchanged. Diagnostic agent: `/root/baseline_tests_diagnostic`.

Failures:

1. `conf_mgr::conf_center::etcd::leader_election::tests::diagnostic_panic_preserves_pending_campaign_and_shutdown`: expected shutdown/win outcome assertion failed after injected diagnostic panics.
2. `conf_mgr::sync_runtime::resource_processor::processor::tests::policy_precedence_test::policy_precedence_tls_source_status_list_watch`: actual spec includes `preparedAncestorAdmission: {mode: unrestricted}`, expected spec does not.
3. `conf_mgr::sync_runtime::resource_processor::processor::tests::policy_precedence_test::policy_precedence_traffic_source_status_list_watch`: same extra prepared ancestor field mismatch.
4. `observe::unknown_fields_registry::tests::metric_rendering::rendering_is_deterministic_across_repeated_calls`: same two metric series appear in opposite order.

No failure has been accepted, suppressed or bypassed. Diagnosis and targeted reproduction must determine next action. Read-only semantic investigation may continue; no commit gate has passed.

## Independent diagnostic conclusion

`/root/baseline_tests_diagnostic` verified stable policy fixture failures, leader-election test flakiness (7/12 isolated failures), and metric ordering flakiness (8/12). No network or sandbox cause. The original affected files match baseline. These are separate Controller/shared-fixture repairs, not necessary Gateway batch support. Mutation and commit remain held; read-only analysis is safe. See deferred D0002-D0004 for exact repair boundaries.

Resume requires separately reviewed repairs, meaningful targeted regression passes and a successful rerun of the required full-suite baseline. Do not accept a lucky isolated pass. No test/diagnostic process remains live; the redundant narrower Cargo diagnostic was cancelled (130), distinct from the original failing result.

## Continuation audit

Current HEAD and tracked contents still match the original baseline. Independent `/root/resume_gate_audit` reran policy tests against the original test binary: exit 101, one passed and the same two source-status list/watch tests failed. Mutation and commit remain held. Useful verification is still available: `cargo test -p edgion-gateway --lib` is now running as session 66208 with output `/private/tmp/edgion-pruning-gateway-baseline.log`, to fill the Gateway behavior gap left by the earlier workspace abort. This is not a substitute for the required failed workspace gate.

Independent continuation adjudication: finish Gateway baseline while keeping mutations/commits held. The shared policy fixture also feeds Gateway event_dispatch_test, but independent Controller election and metric mechanisms are not Gateway support edits. A narrow exception to anchor ordering needs separate user authorization or externally supplied repairs; no assertion waiver is allowed. An asynchronous authorization question was sent for exactly these three prerequisite repair scopes. No answer has yet been received, and elapsed time is not approval.

## Gateway library baseline

Session 66208 completed with exit 101: 6,341 passed, 79 failed, 19 ignored. Log: `/private/tmp/edgion-pruning-gateway-baseline.log`. Many failures report Wasm preparation capacity exhaustion; other observed failures include peer-pool digest and audited context layout sizes. Do not infer root causes or fix expected values before independent contract verification. `/root/gateway_baseline_diagnostic` is performing focused read-only diagnosis. No source edits precede these failures.

Observed candidate-relevant tests within the failing Gateway run: all 21 `api::` tests, 9 etcd apply tests, 13 accounting finalizer tests, 27 attempt-runtime tests and 52 usage tests passed. These are individual baseline results only, not a passing Gateway suite. Correct future API filter to `api::` (the proposed `api::tests` prefix discovers zero tests because actual submodules are admin_auth/router_shape_tests/etc.).

Gateway diagnostic completed: 76 Wasm failures arise from process-global preparation admission shared by parallel tests; unchanged sequential Wasm group passes 272/272. Peer-builder group fails 5/5 parallel runs with global map lost-update evidence and passes sequentially; production writer concurrency remains unproven. Two context layout failures reproduce independently (184/176 and 400/432), requiring field-level audit rather than constant replacement. Recorded separate D0006-D0008. No diagnostic process remains live. Default Gateway suite remains failed; targeted serial passes do not waive it.

This continuation made progress by completing missing Gateway baseline verification and diagnosing its failures. Cross-anchor repair authorization remains unanswered; no dependent mutation is authorized by elapsed time.

## Third consecutive blocker audit

HEAD remains `75c4d51ecd195193ba6a91d210736c0be4e4676f`; tracked files remain unchanged. No external repair or user response authorizing the bounded Controller prerequisite exception has arrived. All diagnostic agents and test processes are terminal. The previous continuation made concrete progress through the missing Gateway baseline and diagnosis; that available work is now complete. Three candidate batches remain held behind the same required-verification failure. Independent gate adjudication requires separate authorization or externally supplied repairs before mutation can resume. The thread goal is being marked blocked, not complete. Do not rerun unchanged failing suites or bypass the single-anchor rule to create apparent progress.

## Authorization resolved (2026-10-29)

The user authorized repairing these failures first and then continuing. The anchor-order exception is granted for the bounded baseline repair phase. Failures themselves remain open until reviewed fixes and fresh verification pass.

## R0001 final resolution

User-authorized baseline repairs passed independent plan and final reviews within 40 paths. Full workspace test sweep: 13,152 passed, zero failed, 67 ignored. The subsequent DSL-only integration fixture repair passed targeted DSL and untargeted default integration: 290 groups, 279 passed, zero failed/timed out, 11 existing skips. All isolated scenarios and final metrics/unknown-fields/requeue/cache-residue guards passed. Latest workspace check, Clippy with existing advisory warnings, format and documentation checks passed. No baseline verification blocker remains; D0001 advisory documentation alignment and D0005 duplicate-field contract remain separate deferred work.
