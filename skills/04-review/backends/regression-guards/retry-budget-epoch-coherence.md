---
name: retry-budget-epoch-coherence
description: Review RetryBudget epoch publication, monotonic windows, delayed operations, and request generation capture.
knowledge_type: regression-guards
---

# Retry-budget epoch coherence

## Conclusion and scope

`EdgionBackendTrafficPolicy.spec.retryConstraint` controls aggregate retry
admission for Service and AI backend targets. `RetryBudget` now uses one
monotonic elapsed sample per public operation and two epoch-tagged slots per
window. Each slot publishes its epoch, attempts, and retries in one atomic word.
The percentage and minimum-rate intervals remain independent.

This is the correction for `retry-budget-generation-and-window-coherence`.
Implementation, independent review, targeted tests, integration, and required
workspace checks passed on 2026-09-07 on macOS arm64.

## Rationale and evidence

The original `BudgetWindow::rotate_at` published `window_start` before four
separate count mutations. A reader could see half-rotated numerator/denominator
pairs, and a new-window increment could be moved to previous. Long-idle rotation
also retained already expired history, and `try_retry` sampled wall time up to
five times despite claiming one sample.

The replacement `EpochWindow` uses atomic epoch/count publication and a validated
two-slot snapshot. It never moves counts; only the sampled epoch and its immediate
predecessor contribute. It preserves millisecond configuration precision.
See the [algorithm contract](../../../12-algorithms/retry-budget-epoch-buckets.md)
for the atomic ordering argument, fixed-grid math, and representation bounds.

The original task's Service generation claim was partly addressed by
[Service snapshot capture](ebtp-atomic-service-snapshot.md): existing policy Arcs
were retained, but an initial `None` could be recaptured after a hot reload added
a policy. Independent follow-up review reproduced that gap. Capture now runs only
on the first request attempt, preserving absence as well as existing Arcs. Current
`begin_upstream_attempt` captures policy before `on_attempt`, and
`retry_budget_for` reads that Arc for later gates. AI dispatch and retry gates
likewise use `AiHttpState.effective_policy`. Keep those call chains; this fix
adds the missing first-attempt boundary without changing AI policy capture.

Authority: the user-approved task requires monotonic time, coherent epochs,
one sampled instant, allocation-free nonblocking accounting, and the existing
soft cap. [Time conventions](../../../03-coding/03-time-conventions.md) and
[graded rules](../../../00-design-rules.md) remain canonical.

## Alternatives and limits

The user explicitly approved the native 128-bit atomic dependency/startup gate
and the saturation/delayed-operation behavior before implementation. A sampled
decision whose history is unavailable denies. A delayed attempt cannot overwrite
a newer epoch. A floor increment superseded after budget admission is discarded
without rollback. Saturated retained history denies; identifiers never wrap.

N concurrent deciders retain up to N-1 soft over-admissions. The unrelated
Webhook `SlidingWindowCounter` now uses a separate whole-window admission CAS;
see [its protocol and verification](../../concurrency/regression-guards/sliding-window-check-then-increment.md).
Its stricter admission contract does not inherit RetryBudget's soft cap.

## Verification and re-evaluation

Window-fix verification on 2026-09-07, before the absent-policy follow-up:

- `cargo test -p edgion-gateway --lib backends::policy::backend_traffic::retry_budget::tests -- --test-threads=1`: 18 passed.
- `cargo test -p edgion-gateway --lib retry_budget -- --test-threads=1`: 24 passed,
  including the window, Service Arc, AI dispatch, and store retention cases.
- The same freshly built Gateway test executable ran
  `routes::http::proxy_http::pg_upstream_peer::tests::service_try_retry_uses_same_captured_budget_arc --exact`
  (1 passed), `ai::dispatch::tests --test-threads=1` (22 passed), and
  `backends::policy::backend_traffic::store::tests --test-threads=1` (26 passed).
  These filter counts overlap; they are not a count of unique tests.
- `./edgion-tests/integration/scripts/integration/run_integration.sh -r EdgionBackendTrafficPolicy -i RetryBudget`:
  PASS, 18 report gates passed, none failed or skipped. Both business cases
  (low-volume floor and budget denial under load) passed. Report:
  `tmp/integration_testing/testing_20260907_152603/report.log`.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy reports existing
  warnings outside the edited implementation/test lines.
- Agent-doc, SSA-force, metrics-inventory, Pingora-source, and Gateway
  no-tracing guards passed. The integration preflights also passed English-only,
  types-purity, and shell-portability guards.
- `cargo audit` passed with three allowed existing unmaintained warnings;
  `cargo deny check` passed advisories, bans, licenses, and sources.
- Independent read-only review: LGTM, including epoch coherence, delayed floor
  accounting, native-atomic startup gating, and knowledge applicability.

The absent-policy follow-up reran the Gateway test executable after rebuilding:
all 87 `pg_upstream_peer::tests`, 22 `ai::dispatch::tests`, and 24 `retry_budget`
filtered tests passed. The new regression also passed individually. Counts
overlap; they are not a unique-test total. Workspace check and Clippy, format,
agent-doc, SSA-force, metrics-inventory, Pingora-source, and Gateway no-tracing
checks passed again; Clippy retained existing warnings. A fresh independent
review approved the absence fix and checked the attempt-count lifecycle across
Service, gRPC, DirectEndpoint, and AI paths.

The follow-up `service_retry_preserves_missing_policy_after_reload` regression
publishes a new policy between attempts through the real store. It checks that
old requests keep no policy, their retries do not fund the new budget, and new
requests capture and fund it. Existing tests cover retained nonempty captures.
This is an in-process hot-reload scenario, not a network-level dynamic test.
No full `cargo test --all` or amd64 runtime validation is claimed.

The new deterministic tests pause writers after publication and before CAS,
interleave snapshot loads, expire floor commits, and test counts, saturation,
millisecond windows, multi-interval idle, and exactly one public-operation time
sample. Test-only clocks/hooks do not exist in production builds.

Re-evaluate if epoch packing, ordering, CPU support, retention, saturation, or
Service/AI policy capture changes. Historical Service snapshot results are not
fresh verification of this window implementation.

## Resolution references

- `retry-budget-generation-and-window-coherence` — real window defect; Service
  Arc generation capture originally missed absent-policy retention, corrected
  by the first-attempt capture gate; fixed and verified
  2026-09-07. On 2026-09-09, source revalidation found the pending task still
  described the replaced implementation. The user approved closing it as
  already fixed, without another runtime change. Fresh read-only independent
  review returned LGTM. `cargo fmt --all`, workspace check and Clippy, agent-doc,
  SSA-force, metrics-inventory, Pingora-source, and diff-whitespace checks passed.
  Clippy retained warnings in existing code. Agent-doc validation required two
  unrelated missing-task links to be described as unverified historical paths.
  This closure removes the stale pending task. Unit and integration tests were
  not rerun; the test results above remain historical evidence.
