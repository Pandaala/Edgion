# B0037 — Plugin Outbound Call

## Scope

- Baseline: `ae88bb1c334675b6e29d8f5cc6f0a764339d7b8b`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- File: `edgion-gateway/src/plugins/http/common/outbound/call.rs`
- Forbidden files: every other source or test file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the source file plus B0036/B0037 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0037 analysis turn.
- Decision: `change`, simple. Public `OutboundCallError::Connect(String)` and `Exhausted(String)` values retained URL-build and reqwest transport/body details through ordinary `Debug` and `Display` formatting.
- Sensitive-data evidence: path templates may resolve `${secretRef:...}` values, URL parsing errors embed the rendered URL, and reqwest errors may retain request URLs. The public error surface therefore allowed resolved credentials or destinations to survive beyond the classification boundary.
- Consumer evidence: ForwardAuth, AI guard, response guardrail, and Webhook callers already collapse these failures to fixed diagnostics, labels, bodies, or metrics. No caller consumed the dynamic payload.
- Preserved semantics: canonical client/TLS/egress, denied redirects/proxy environment, service authority pinning, replayable-only retries, backoff/Retry-After, timeout/cancellation, passive health and half-open CAS, authority rejection, terminal HTTP response handling, and streaming hard response caps.
- Freshness revalidation: valid at `ae88bb1c3`; B0036 was records-only.

## Implementation

- Converted Connect and Exhausted to detail-free unit variants and erased builder/dial/framing/reqwest/body-stream detail only after live errors supplied required timeout/connect classification.
- Removed the unused last-error string state.
- Added a structural regression pinning safe static Debug and Display output for every error variant while retaining the response header/body redaction regression.
- Added no log, metric, schema, client, resolver, retry path, or body cache.

## Independent review

- Reviewer: `b0001_review`, independent from the B0037 analyst and implementer.
- Verdict: approved with no findings after enumerating every producer and consumer, confirming no alternate storage/logging path, checking classification timing and all retry/health/body invariants, assessing diagnostics value, and validating exact scope.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | Outbound call 35, complete common outbound 87, Webhook 50, ForwardAuth 7, AI guard 6, and response guardrail 17 tests passed. |
| Independent review checks | passed | Gateway check/Clippy, formatting, no-tracing, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
