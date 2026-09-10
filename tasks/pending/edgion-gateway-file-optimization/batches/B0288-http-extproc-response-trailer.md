# B0288 — HTTP ExtProc Response Trailer

- Pipeline state: changed; committed after independent plan/final LGTM and all checks.
- Active anchor: `edgion-gateway`; refreshed baseline `60374898`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs`; blob `9256f1ffd9a99ba55ed241d82a987bd03aff6db2`.
- Analyst: `b0288_extproc_response_trailer_analysis`; full 753-line target and body/trailer/logging consumers examined read-only.
- Confirmed: `finalize_incomplete` and `on_terminal_preempted` only check local completion and can overwrite an already settled shared owner. Logging cleanup precedes summary extraction; earlier trailer failure can likewise erase a later owner's decisive outcome. The normal callback already checks shared-owner completion.
- Proposed scope: target only, cleanup guards and inline deterministic tests. Mark local completion but terminalize only an active owner. Preserve release-only semantics and normal verdict correction after transport completion; global first-write-wins is incorrect.
- Test evidence needed: settled fail-close/superseded/fail-open/suppressed outcomes preserved by both hooks, repeated cleanup, active-owner cancellation/preemption, release-only logging fallback, and invalid-verdict correction after exchange success.
- This owns the same trailer finding identified by B0287; no duplicate deferred task. D0148-D0152 concern separate boundaries.
- No edits or verification authorized before serial promotion. Refresh against intervening commits; preserve integration freeze, binary/access-log ownership, redaction/restraint, canonical mechanisms, streaming/caps/failure contracts and English-only writes.
- Refresh confirmed B0286 did not change target/shared owner/body companion/driver/dispatcher/logging. Analyst corrected initial simple classification: cleanup changes final error/preemption ownership and therefore requires plan-first.
- Tests should use the existing private TrailerExchange scripted double, not AttachedExtProc::test_lazy (its ExtProcClient construction accesses the global TARGET_RUNTIMES cache). Cover both hooks, repeated/cross cleanup, inactive outcome preservation, active terminal writes exactly once, release-only pending summary, and success-to-bad-verdict correction for both failure policies. No new production seam or wider scope needed.
- Analyst revalidated target/dependencies/knowledge unchanged after coordinator-only B0287 commit. Fresh implementer `b0288_implementation` assigned plan-only; exact technical allow-list is target source and its inline tests only. No edits or test execution before independent plan LGTM.

## Proposed implementation plan

- Retain local completion guards, set local done, and terminalize only active shared owners in both cleanup hooks. Normal callback, failure classification and release production behavior remain unchanged.
- Extend only the existing test Scripted double with mutable owner inactivity, summary, terminal/release counters and optional successful final-exchange settlement.
- Four deterministic tests cover settled outcome matrix across both hooks, active cleanup once, release-only pending summary plus release idempotence, and transport-success-to-invalid-verdict correction under both failure policies. Repeated/cross cleanup must preserve the decisive outcome. No global first-write-wins or real client/global cache setup.
- Fresh independent reviewer `b0288_plan_review`: LGTM, no MustFix. Confirmed shared driver serialization makes the local cleanup guard sufficient without a new atomic mechanism; inactive and final-summary state must remain distinct in tests. Original implementer authorized within the single-file plan.

## Implementation and verification

- Implemented exactly the approved single-file plan, with four new deterministic lifecycle tests; no material deviation. Normal callback/verdict correction and release-only production behavior are unchanged.
- `cargo test -p edgion-gateway --lib plugins::http::ext_proc::response_trailer::tests --locked`: 14/14 passed (40.69s compilation, 0.02s tests), no failures. Scoped rustfmt apply/check and whitespace checks passed.
- Root workspace format/apply/check, locked workspace/all-targets check (10.48s), locked Clippy (15.74s, existing warnings, exit 0), agent-doc validation, SSA force, metrics inventory, Pingora policy, Gateway tracing boundary, English-only, frozen integrations and diff whitespace passed.
- Fresh final reviewer `b0288_final_review`: LGTM, no MustFix. Independently checked full target, actual owner/driver/dispatcher/logging order, test assertions and all gates. No network/global client setup or integration additions/edits.
- Commit `b93fd0f3`; no push. B0290 analysis produced standalone future integration task D0153; its production candidate remains queued and unmodified.
