# Data-plane callback optimization review

Status: in progress. Anchor: `edgion-gateway`. Baseline: `81291f899`.

Review callbacks and their reachable data-plane work incrementally. Apply only
behavior-preserving optimizations through IMPL plan and final independent review,
verify each batch, and commit separately without pushing. Deferred proposals need
the user's decision. This directory follows the user's explicit singular `task/`
destination.

## Coverage

| Stage | Current evidence / remaining work |
| --- | --- |
| HTTP callback dispatch (`proxy_http/mod.rs`) | Callback inventory inspected; implementations still require staged review. |
| `new_ctx`, `init_downstream_modules` | Direct bodies inspected: pinned config and parsed policy; no change selected. Context construction and module internals remain open. |
| `early_request_filter` | Direct body and request-ID resolution inspected. Arc handles support mutable context dispatch; do not blindly replace with references. Publication-time list resolution deferred below. Trace-ID and preflight runtime internals remain open. |
| `request_filter` | Metadata entry traced; temporary path copy selected for removal. Routing, plugin dispatch, RealIp, XFF and TLS metadata require further review. |
| Upstream selection, connect, upstream request | Pending. |
| Request body, trailers and relay | Pending. |
| Response headers, hold/commit, body and trailers | Pending. |
| Errors, retries, logging and connection reuse | Pending. |
| Redirect HTTP, connection filters, TLS callbacks, TCP/TLS/UDP paths | Pending. |

## Deferred decisions

- [Publication-time global/Gateway plugin lists](early-01-precompute-plugin-lists.md)
- [Cache policy fixture verification failure](verification-01-cache-policy-fixture.md)

## Current implementation batch

Borrow the raw URI path in `build_request_metadata` until synchronous validation
finishes. Retain owned raw/normalized context snapshots and the existing query
transfer. Independent plan and final code reviews passed. Formatting, workspace
check and seven repository guards passed. Request-filter tests: 44 pass, one
existing policy-fixture failure reproduced alone and serially (see above).
Workspace Clippy passed with warnings outside the changed lines. The nine
request-metadata/XFF tests also passed in an isolated serial run. The baseline
failure attribution is source-based, not a baseline binary comparison.
No throughput claim is made: the source change removes one temporary path String.
