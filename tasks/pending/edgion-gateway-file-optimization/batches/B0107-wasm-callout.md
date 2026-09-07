# B0107 — Wasm HTTP Callout

## Scope
- Baseline: `3ca80316`
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/callout.rs`
- Baseline blob: `84cca95f42b651653f50761a68771425f3178f03`
- Support file: `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- Support blob: `988999248afd8a329dc20bc88da42ec822eea726`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/callout.rs`; `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- Forbidden files: every file outside the exact two-file allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace outbound Wasm HTTP call ownership, canonical client/TLS/egress mechanisms, request/response byte bounds, VM/host-ABI interactions, failure behavior, hot-reload generations, logging/access-log diagnostics, and deterministic coverage.
- Preserve the distinction between bounded plugin-initiated exchanges and the always-streaming main upstream response, as well as sensitive-data, observability, no-tracing, and cross-cutting-unification rules.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: independent read-only analysis completed at `512d485d`; the target blob remained `57206002b67385608cf37b6544ff32d9a579f2d2`
- Promotion freshness: revalidated at `dde42825`; B0106 changed only WAF/plugin-store/support and campaign paths, while the callout target blob and Wasm callout ownership chain remained unchanged.
- External drift: before implementation could complete, the user advanced HEAD to `3ca80316`; both `callout.rs` and its `plugin.rs` consumer changed in committed external work. The prior implementation brief is stale and must be independently refreshed before any edit.
- Integration baseline: an independent diagnostic attributed the freeze failure to committed external work, not B0107. The manifest was mechanically refreshed at `3ca80316` from 3,023 to 3,061 tracked files, absorbing 45 changed fingerprints and 38 newly tracked files; no `edgion-tests/**` file was edited and the refreshed freeze check passed.
- Inventory reconciliation: current HEAD contains 534 tracked Gateway files. Five externally added files were added as pending ledger rows, three externally removed files were retained as historical `removed` rows, and the invalid `n/a` state on the current `lb/ewma/score.rs` was restored to pending.
- Refresh analyst: independent current-HEAD analysis completed at `3ca80316`; external commit `c97146e5` already delivered the previous ownership/copy optimization in full.
- Decision: change. The final buffered callout body must canonically own framing: guest-provided stale/duplicate `Content-Length` or `Transfer-Encoding` currently reaches Hyper unchanged and can truncate or fail the exchange. Reuse `sync_outbound_body_framing` after the final body is installed.
- Complexity: moderate. The change spans one Wasm builder and the minimum visibility widening of the existing shared framing helper, without changing the helper's semantics.
- Preserved behavior: allowlist, canonical egress/client/proxy/redirect policy, deadline and semaphore, ordered duplicate non-framing headers and Host override, invalid-header skipping, empty-body behavior, status/failure callbacks, metrics, VM poisoning/cancellation, hot-reload grants, and the 256 KiB streaming response cap.
- Tests: add a deterministic file-local request-construction regression for stale/duplicate length replacement, transfer-encoding removal, exact body, Host, duplicate normal-header order, and invalid-header skipping; run adjacent shared framing tests and focused Wasm callout coverage.
- Deferred: D0072 owns a cross-cutting canonical outbound-client decision for bounded HTTP/2 decoded response header lists.
- Integration: frozen authorized/denied request-callout and tick-callout scenarios are sufficient; no integration issue is needed.
- Implementation: widened the canonical framing helper only to `pub(crate)`, built one owned request after applying URL/authority/valid guest headers/final body, synchronized framing last, and dispatched it through the unchanged client. File-local tests cover stale/duplicate length replacement, transfer-encoding removal, body/Host/ordinary duplicate-header preservation, invalid-header skipping, and empty-body behavior.
- Implementer verification: Wasm callout 7/7 and shared outbound request 11/11 passed; locked targeted Gateway check and Clippy, format, diff, no-tracing, and frozen-path guards passed.
- Mechanical containment: current HEAD and both baseline blobs remained unchanged; implementation diff is exactly the two-file allow-list and `edgion-tests/**` remains untouched.
- Review: LGTM with no Must Fix or Should Fix. The independent reviewer confirmed reqwest builder/execute equivalence, duplicate-header order, exact framing ownership, minimal visibility, preserved egress/deadline/semaphore/response-cap/VM lifecycle, and continued D0072 ownership.
- Verification: Wasm callout 7/7, shared outbound request 11/11, request-hook callout 4/4, reference guest callout 2/2, reference guest tick callout 1/1, and VM tick callout 5/5 passed. `cargo fmt --all` and `--check`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and refreshed frozen-path guards passed on `3ca80316`; Clippy retained only the existing warning set.
- Commit: `16613ca6`
