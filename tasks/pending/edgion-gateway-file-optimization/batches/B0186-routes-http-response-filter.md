# B0186 — HTTP Response Filter

## Scope

- Analysis baseline: `ec774be0545717b7816622590cc755d1ba9abc2d`
- Target blob: `b3bfe9dc90d17c55b2c647970963711421bad40e`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. A response-stage native-WAF denial can stage an admitted informational status, fail `claim_final_response`, and currently fall through to the original upstream response. The minimal fix must return a nonretryable `HTTPStatus(500)` before any downstream commit. The same-file compression selector can also skip framing-header scans immediately when compression is disabled or the response is SSE.
- Frozen implementation scope: `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` and `skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md`.
- Required preservation: valid 200–599 local replies, interim/101 handling, response onion, H1 close, retry/outlier ownership, SSE/AI eligibility, head holding, streaming body/trailer processing, and canonical access-log ownership.
- Deferred candidates: audit terminal-status validation across WAF and other producers, and later add a standalone native-WAF response-stage status-100 integration scenario without editing existing integration files.
- Baseline tests: target 8/8 passed.
- This is a queued candidate and cannot enter the serial mutation/review lane before B0185 commits.
