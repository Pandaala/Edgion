# B0288 — HTTP ExtProc Response Trailer

- Pipeline state: analysis-ready; change candidate, target-local.
- Active anchor: `edgion-gateway`; baseline `d26bcd5a`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs`; blob `9256f1ffd9a99ba55ed241d82a987bd03aff6db2`.
- Analyst: `b0288_extproc_response_trailer_analysis`; full 753-line target and body/trailer/logging consumers examined read-only.
- Confirmed: `finalize_incomplete` and `on_terminal_preempted` only check local completion and can overwrite an already settled shared owner. Logging cleanup precedes summary extraction; earlier trailer failure can likewise erase a later owner's decisive outcome. The normal callback already checks shared-owner completion.
- Proposed scope: target only, cleanup guards and inline deterministic tests. Mark local completion but terminalize only an active owner. Preserve release-only semantics and normal verdict correction after transport completion; global first-write-wins is incorrect.
- Test evidence needed: settled fail-close/superseded/fail-open/suppressed outcomes preserved by both hooks, repeated cleanup, active-owner cancellation/preemption, release-only logging fallback, and invalid-verdict correction after exchange success.
- This owns the same trailer finding identified by B0287; no duplicate deferred task. D0148-D0152 concern separate boundaries.
- No edits or verification authorized before serial promotion. Refresh against intervening commits; preserve integration freeze, binary/access-log ownership, redaction/restraint, canonical mechanisms, streaming/caps/failure contracts and English-only writes.
