# B0186 — HTTP Response Filter

## Scope

- Analysis baseline: `ec774be0545717b7816622590cc755d1ba9abc2d`
- Target blob: `b3bfe9dc90d17c55b2c647970963711421bad40e`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. A response-stage native-WAF denial can stage an admitted informational status, fail `claim_final_response`, and currently fall through to the original upstream response. The minimal fix must return a nonretryable `HTTPStatus(500)` before any downstream commit. The same-file compression selector can also skip framing-header scans immediately when compression is disabled or the response is SSE.
- Frozen implementation scope: `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` and `skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md`.
- Required preservation: valid 200–599 local replies, interim/101 handling, response onion, H1 close, retry/outlier ownership, SSE/AI eligibility, head holding, streaming body/trailer processing, and canonical access-log ownership.
- Deferred candidates: audit terminal-status validation across WAF and other producers. The native-WAF response-stage status-100 integration follow-up was later resolved by D0104.
- Baseline tests: target 8/8 passed.
- Deferred records: `D0103` owns the wider producer/schema terminal-status audit. D0104's wire integration follow-up is resolved in [maintained knowledge](../../../../skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md). Existing integration files remained frozen during B0186.
- Previous batch: `29f620b8`; never push.

## Implementation Plan

1. Add a private helper that claims a staged final response or returns an explicitly nonretryable static `HTTPStatus(500)` error without delivery-side state changes.
2. Propagate claim failure before keepalive or downstream writes; preserve the valid claim → H1 close → header → written marker → body → replacement diagnostic → sentinel order.
3. Return compression level zero before framing-header scans when compression is disabled or the response is SSE; preserve all eligible-response decisions.
4. Pin valid, informational, already-owned, disabled/SSE, and existing framing cases with inline tests.
5. Extend the response-stage denial regression knowledge with fail-close claim and retry/outlier ownership.

## Outcome

- Implementation: every staged local reply now passes through one final-writer claim helper before keepalive or downstream writes. Rejected invalid statuses and existing-writer conflicts return a static, explicitly nonretryable `HTTPStatus(500)` control-flow error; an existing committed response remains authoritative. Disabled compression and SSE now return level zero before framing-header scans.
- Review: the first fresh final review found that an existing writer bypassed pending-reply consumption in the real control flow. The corrected implementation always consumes and validates the pending reply and adds two live H1 response-filter tests. Re-review returned LGTM with no remaining Must Fix or Should Fix item.
- Verification: target tests passed 13/13 and the complete proxy-http suite passed 249/249. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and frozen-path guards passed.
- Commit: `848e96e9`; never pushed by the campaign.

## Follow-up closure

- D0104 is resolved by the `WafDynamic` wire regression recorded in
  `skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md`.
