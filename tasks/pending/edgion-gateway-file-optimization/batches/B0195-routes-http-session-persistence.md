# B0195 — HTTP Session Persistence

## Scope

- Pipeline state: terminal
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/session_persistence.rs`
- Analysis baseline: `7ac8a760`
- Freshness check at `387ec0d7`; previous B0194 is committed.
- Existing integration files and user-owned paths remain protected.
- Implementation agent: `b0195_implementation`; independent review follows implementation.

## Investigation

- Investigate token extraction, expiry, backend identity, retry behavior, cookie wire semantics, allocation cost, and canonical shared mechanisms before choosing a value outcome.

## Analysis Outcome

- Decision: change; simple. Target blob `322fbd4fec66e347e4848fe2516b66e7c966a7d1` at HEAD `7ac8a760`.
- Cookie extraction reads only the first field; reuse public `session_adapter::cookie_value` to honor tokens in later Cookie fields. Preserve canonical first-match semantics.
- Malformed expiry currently strips its suffix and becomes a valid unexpired bare hash; retain invalid input for a lookup miss/refresh. Preserve bare hashes, valid numeric expiry, and existing expiry boundary.
- Reuse the already-owned token String where possible instead of copying it again.
- Proposed technical scope: target source, new `skills/04-review/routing/regression-guards/session-persistence-token-extraction.md`, and `skills/04-review/routing/SKILL.md`.
- Verification pending: target and canonical cookie helper tests, real in-memory request extraction, malformed/valid expiry, and owned-buffer reuse where meaningful.
- Deferred integration required when promoted: later Cookie-field affinity for HTTP/gRPC and malformed-expiry refresh, keeping existing integration files frozen.

## Implementation

- Implementer: `b0195_implementation`; exact three-file technical scope above respected.
- Replaced the first-field-only parser with the canonical borrowed cookie helper, retained malformed token suffixes for a lookup miss, and truncated valid expiry suffixes in the existing owned buffer.
- Added four unit tests and strengthened expiry/known-backend cases. Backend selection, strict affinity, retry, local replies, and valid/bare token semantics remain unchanged.
- Future HTTP/gRPC integration coverage is tracked by D0109; no integration file was modified.

## Independent Review

- Reviewer: `b0195_independent_review`; LGTM, no Must Fix or Should Fix. Independently reran both targeted filters and checked all HTTP/gRPC consumers and the knowledge conclusion.

## Verification

- Session-persistence filter: 21/21 (19 target-module tests); canonical cookie helper: 4/4.
- Full HTTP routes unit filter: 394/394 with the default parallel runner.
- Workspace format, locked workspace check/Clippy, agent-doc validation, SSA, metrics, Pingora source, Gateway tracing, English-only, and diff checks passed.
- Locked Gateway binary build passed. Frozen-path check passed; final exact staging gate follows.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `56cda803`.
