# B0289 — HTTP ExtProc Streamed Request

- Pipeline state: analysis-ready; examined-no-change candidate, no mutation authorized.
- Active anchor: `edgion-gateway`; baseline `3402cb4e`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`.
- Analyst: `b0289_extproc_streamed_analysis`; independently examine the full target, ownership/driver/error/cap consumers, original knowledge and deterministic coverage. B0286 supporting Bytes changes do not establish full-file coverage.
- B0287 and B0288 precede this candidate; refresh before serial promotion. Existing issues D0148-D0152 and B0288 must not be duplicated.
- Preserve all graded gates, canonical body/streaming/access-log ownership and failure policy. No technical writes, integration additions/edits, network tests or protected user-path changes.
- Full 728-line target and 17 scripted tests inspected; current blob `7079fcb1ceeb4a70b7c079bfae99e34d41bceab8`. Verdict validation precedes mutation; Data/Complete/Abandoned, EOS owner retention, fail-open preservation and fail-close fork-pump exit are coherent. Bytes corrections remain intact.
- Protobuf Vec conversion is required by the current generated body type; no second aggregate cache. Request-side contracts must not be confused with the response 1 MiB window cap. Cosmetic imports/detail parameters/guarded matches do not justify protocol/body churn.
- Safe proposed verification: `cargo test -p edgion-gateway --lib plugins::http::ext_proc::streamed::tests --locked` (17 pure scripted tests, no server/global-state mutation). No verification executed by analyst. Refresh before promotion.
