# B0164 — gRPC Timeout Formatting

## Scope

- Candidate analysis and promotion HEAD: `d99393754bce4cfd761e99ddd3f5e6abb1e5c947`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/grpc_timeout.rs`
- Baseline blob: `c926bb1d32e00d7e6b89d30ec79cf3955031d8bf`
- Exact implementation allow-list:
  - `edgion-gateway/src/routes/grpc/grpc_timeout.rs`
  - standalone follow-up task (historical batch artifact; resolved externally and deleted, with evidence retained in the gRPC timeout regression knowledge)

## Handoff

- Analysis: `b0160_final_review` read the complete formatter/parser, request deadline anchoring, retry and upstream-header rewrite paths, Pingora timeout folding, tests and protocol precedent. Decision: change; complex because this is a hot-path wire-protocol boundary.
- Defect: coarser unit conversions truncate. A non-integral duration such as `100_000_001ns` becomes `100000u`, shortening the deadline sent upstream; large seconds-to-minutes conversions have the same problem. Official gRPC implementations round upward when encoding a coarser unit.
- Required invariant: choose the finest unit whose ceiling-divided value fits eight digits. Parsing the formatted value must never produce less than the input; overstatement remains below one selected-unit quantum. Preserve zero as `1n` and maximum-hour saturation.
- Integration gap: add a deferred task for observing the rewritten upstream wire header; do not edit existing integration files.
- Plan: `b0131_final_review` proposed an overflow-safe quotient-plus-remainder ceiling loop across a fixed six-unit table, explicit zero/saturation handling, transition and representable-domain properties, and the frozen wire-coverage task. Independent reviewer `b0163_final_review` approved it without Must Fix or Should Fix and clarified that the non-shortening property applies only to `0 < d <= 99_999_999H`.
- Implementation: replaced truncating coarser-unit conversions with the approved ceiling selection. The formatter now chooses the finest eight-digit unit without shortening representable deadlines, keeps `0` as `1n`, and saturates above the maximum as `99999999H`; parsing and all callers remain unchanged.
- Tests: 14 focused tests passed. Coverage includes every unit transition, hard-coded affected encodings, independent parse-back non-shortening/overshoot assertions, zero, the maximum wire value, the first saturated value, and maximum `Duration`.
- Follow-up closed: `GRPCRoute/TimeoutRewriteWire` now observes the sanitized upstream header after a fixed plugin delay, across two scripted `REFUSED_STREAM` retries, and through gRPC-Web bridging. The exact-zero `1n` race remains covered at unit level because its post-peer-selection timing window cannot be driven deterministically.
- Review: fresh independent final reviewer `b0164_final_review` re-read the two-file diff, parser and upstream rewrite caller, independently checked all unit boundaries and confirmed tests are not tautological. Verdict: LGTM with no Must Fix, Should Fix or additional deferred finding.
- Verification: `cargo fmt --all`; focused tests 14/14; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,107-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `9329e2f0`; never pushed by the campaign.
