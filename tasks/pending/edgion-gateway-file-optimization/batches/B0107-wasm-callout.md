# B0107 — Wasm HTTP Callout

## Scope
- Baseline: `dde42825`
- Pipeline state: in-progress
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/callout.rs`
- Baseline blob: `57206002b67385608cf37b6544ff32d9a579f2d2`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/callout.rs`
- Forbidden files: every file outside the owned file, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace outbound Wasm HTTP call ownership, canonical client/TLS/egress mechanisms, request/response byte bounds, VM/host-ABI interactions, failure behavior, hot-reload generations, logging/access-log diagnostics, and deterministic coverage.
- Preserve the distinction between bounded plugin-initiated exchanges and the always-streaming main upstream response, as well as sensitive-data, observability, no-tracing, and cross-cutting-unification rules.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: independent read-only analysis completed at `512d485d`; the target blob remained `57206002b67385608cf37b6544ff32d9a579f2d2`
- Promotion freshness: revalidated at `dde42825`; B0106 changed only WAF/plugin-store/support and campaign paths, while the callout target blob and Wasm callout ownership chain remained unchanged.
- Decision: change. Consume the already-owned pending callout, move its request body and valid header values into reqwest, and transfer the uniquely owned capped response `Bytes` into `Vec<u8>` instead of copying.
- Complexity: simple; all production and test call sites are file-local and ownership is compiler-enforced.
- Preserved behavior: allowlist, canonical egress/client/proxy/redirect policy, deadline and semaphore, ordered duplicate headers and Host override, invalid-header skipping, status/failure callbacks, metrics, VM poisoning/cancellation, hot-reload grants, and the 256 KiB streaming response cap.
- Tests: targeted callout and Wasm request/tick coverage; add only a file-local deterministic body/wire regression if needed.
- Deferred: D0072 owns a cross-cutting canonical outbound-client decision for bounded HTTP/2 decoded response header lists.
- Integration: frozen authorized/denied request-callout and tick-callout scenarios are sufficient; no integration issue is needed.
- Review: pending
- Verification: pending
- Commit: pending
