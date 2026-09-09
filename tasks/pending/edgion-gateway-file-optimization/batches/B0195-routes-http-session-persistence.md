# B0195 — HTTP Session Persistence

## Scope

- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/session_persistence.rs`
- Analysis baseline: `7ac8a760`
- Read-only candidate; B0194 owns the serial lane.
- Existing integration files and user-owned paths remain protected.

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
