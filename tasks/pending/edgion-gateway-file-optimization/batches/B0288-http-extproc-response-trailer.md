# B0288 — HTTP ExtProc Response Trailer

- Pipeline state: analysis-ready; change candidate, complex / plan-first (error-lifecycle ownership boundary despite one-file scope).
- Active anchor: `edgion-gateway`; refreshed baseline `3402cb4e`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs`; blob `9256f1ffd9a99ba55ed241d82a987bd03aff6db2`.
- Analyst: `b0288_extproc_response_trailer_analysis`; full 753-line target and body/trailer/logging consumers examined read-only.
- Confirmed: `finalize_incomplete` and `on_terminal_preempted` only check local completion and can overwrite an already settled shared owner. Logging cleanup precedes summary extraction; earlier trailer failure can likewise erase a later owner's decisive outcome. The normal callback already checks shared-owner completion.
- Proposed scope: target only, cleanup guards and inline deterministic tests. Mark local completion but terminalize only an active owner. Preserve release-only semantics and normal verdict correction after transport completion; global first-write-wins is incorrect.
- Test evidence needed: settled fail-close/superseded/fail-open/suppressed outcomes preserved by both hooks, repeated cleanup, active-owner cancellation/preemption, release-only logging fallback, and invalid-verdict correction after exchange success.
- This owns the same trailer finding identified by B0287; no duplicate deferred task. D0148-D0152 concern separate boundaries.
- No edits or verification authorized before serial promotion. Refresh against intervening commits; preserve integration freeze, binary/access-log ownership, redaction/restraint, canonical mechanisms, streaming/caps/failure contracts and English-only writes.
- Refresh confirmed B0286 did not change target/shared owner/body companion/driver/dispatcher/logging. Analyst corrected initial simple classification: cleanup changes final error/preemption ownership and therefore requires plan-first.
- Tests should use the existing private TrailerExchange scripted double, not AttachedExtProc::test_lazy (its ExtProcClient construction accesses the global TARGET_RUNTIMES cache). Cover both hooks, repeated/cross cleanup, inactive outcome preservation, active terminal writes exactly once, release-only pending summary, and success-to-bad-verdict correction for both failure policies. No new production seam or wider scope needed.
