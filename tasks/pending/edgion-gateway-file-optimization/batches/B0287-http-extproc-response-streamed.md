# B0287 — HTTP ExtProc Streamed Response

- Pipeline state: analysis-ready; examined-no-change candidate with separate trailer-owned finding.
- Active anchor: `edgion-gateway`; baseline `989d2a27`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs`.
- Analyst: `b0287_extproc_response_streamed_analysis`; response/window ownership, terminal arbitration, bounds, consumers and coverage/value analysis assigned.
- B0285 owns serial plan review; B0286 is queued ahead. Refresh after preceding commits before implementation or independent review.
- Existing integrations and user handoff/WAF paths protected; no technical mutation authorized.
- Full target and typed driver/owner/phase/framing/summary consumers inspected. Matching per-chunk verdicts, independent 1 MiB input/replacement caps, awaited bounded exchange, fail-open original bytes and fail-close terminal ownership are coherent. Existing deterministic tests are meaningful; no worthwhile target-owned change.
- Target blob `51a47472d81f2e59a4e0169e11d6e39d028d368e` unchanged at report completion. No whole-response buffering or semantic SSE guarantee; D0149 remains the stale phase-knowledge owner.
- Separate finding for independent review: response_trailer.rs::finalize_incomplete overwrites a settled fail-close/dependency or superseded attached outcome with cancelled/logging-fallback when its companion remains unfinished. Logging invokes this cleanup before attached summary extraction. Preserve settled owners while cancelling truly unfinished work; do not broadly change mark_terminal to first-write-wins. Proposed tests cover failure/preemption preservation, unfinished cancellation and repeated cleanup.
