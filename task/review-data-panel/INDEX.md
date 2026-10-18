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
| `request_filter` | Temporary metadata path copy removed in `4a976e022`. XFF snapshot preserves bounded aggregate size and owned log state; RealIp extractor borrows tokens and traverses in reverse. TLS metadata copies feed request-owned state. Routing, plugin dispatch and deeper RealIp policy dependencies remain open. |
| Upstream selection, connect, upstream request | `connected_to_upstream` direct body inspected: borrowed SAN digest, one timestamp, owned connection-establishment log. `upstream_request_filter` direct body and deferred credential nomination guard traced to shared Connection parser. Upstream selection and deeper framing/transcoder/credential consumers remain open. |
| Request body, trailers and relay | Direct action/tee/trailer orchestration, mirror payload budget and observer dispatch inspected. Nullable-state fast path, moved handler vector, bounded independent mirror copy and terminal-only log drain retained. Concrete streamed-handler/WAF implementations and deeper storage/relay integration remain open. |
| Response headers, hold/commit, body and trailers | Connection sanitizer inspected. Body/trailer orchestration, runtime dependency checks, processor driver and downstream cancellation wrapper inspected; prebuilt read-only runtime-chain candidate deferred. Concrete processors, conditions, hold/commit and remaining header chains remain open. |
| Errors, retries, logging and connection reuse | Terminal logging direct body and extension projection inspected. Borrowed bounded projection implemented below. Connect/proxy retry direct bodies inspected; retained per-attempt errors and peer context remain owned. Terminal error rendering direct body inspected: exact gRPC denial is consumed, protocol-specific replies and final-status ownership retained. Deeper finalizers and retry consumers remain open. |
| Redirect HTTP | Direct callbacks and shared client-policy application inspected. Missing-host and listener-admission checks now share one normalized hostname. Raw Location authority, query-free log URL and session-owned keepalive remain unchanged. |
| Connection filters, TLS callbacks, TCP/TLS/UDP paths | Pending. |

## Deferred decisions

- [Publication-time global/Gateway plugin lists](early-01-precompute-plugin-lists.md)
- [Cache policy fixture verification failure](verification-01-cache-policy-fixture.md)
- [Prebuilt read-only response-body dispatch](body-01-prebuild-readonly-dispatch.md)

## Completed implementation batch

Borrow the raw URI path in `build_request_metadata` until synchronous validation
finishes. Retain owned raw/normalized context snapshots and the existing query
transfer. Independent plan and final code reviews passed. Formatting, workspace
check and seven repository guards passed. Request-filter tests: 44 pass, one
existing policy-fixture failure reproduced alone and serially (see above).
Workspace Clippy passed with warnings outside the changed lines. The nine
request-metadata/XFF tests also passed in an isolated serial run. The baseline
failure attribution is source-based, not a baseline binary comparison.
No throughput claim is made: the source change removes one temporary path String.

## Completed implementation batch: Connection helper allocations (`7d891d438`)

The canonical `plugins/http/common/http_client.rs` parser compares borrowed
tokens before allocating lowercase output for first occurrences. Its combined
classifier returns before lowercase allocation for static matches and empty
dynamic lists. Nonempty dynamic-list comparison keeps its exact original
lowercase-input semantics, including noncanonical caller-supplied lists.

Reachable consumers inspected: deferred credential nomination, response header
sanitization, outbound request forwarding, mirror header forwarding, endpoint
probe filtering, credential injector, and response-cache key exclusions.
No forwarding-boundary, list-order, failure or logging behavior is changed.
Unique-token scans now use case-insensitive comparison; this is an allocation
reduction claim, not a measured overall CPU or throughput improvement.

Independent plan review passed. Allocation guards confirm zero allocation for
static/empty fast paths and duplicate-token allocation parity. All 105 targeted
tests passed: helpers (7), upstream request (14), response headers (22), outbound
request (23), mirror (27), credential injector (7), cache keys (5). Formatting,
workspace check, workspace Clippy and seven repository guards passed. Clippy
warnings remain outside the changed code. Independent final source review
passed; no full integration or throughput benchmark was run for this batch.

## Body callback review

No production change selected in this batch. Current source confirms:

- Mirror copies retain exact admitted payload rather than arbitrary Bytes
  backing allocations; preserve `request-mirror/regression-guards/mirror-resource-budgets.md`.
- Request observers borrow bytes, use bounded slots and isolate failures;
  body-handler vectors move out and back rather than being cloned per chunk.
- Response driver leases release inflight owners on cancellation, including
  before first callback poll. The mutex is request-local execution ownership,
  not a shared configuration lock to replace mechanically.
- Empty read-only body-filter vectors still enforce IP dependency availability.
  Terminal-before-trailers cannot be folded into ordinary EOS or skipped.
- Existing async-hook profiling precedent remains applicable; no public
  Pingora fast-path API change is justified by allocation existence alone.

Fresh existing tests: 103 passed (request body/action 29, response body 31,
response trailers 11, downstream cancellation 6, driver 3, observers 19,
mirror budget 4). These are unit-level lifecycle checks, not fresh real-socket
cancellation, throughput or whole-plugin coverage. Independent plan and final
record reviews approved the documentation-only deferral. Formatting, workspace
check/Clippy, agent-doc validation and diff-whitespace checks passed; Clippy
retains warnings outside this documentation-only diff.

## Access-log extension projection

`logging()` collects declared extension values from request/response headers,
entry-query cache, the canonical cookie parser, ctx variables and route metadata.
Resolution now borrows these sources; insertion copies only the UTF-8-safe prefix
within the existing value cap. Small owned strings still transfer their allocation,
and owned oversized capacities are released even when their value is short.
Field admission, tier overwrite order, masking and serde escaping are unchanged.
The final extension block and async JSON payload still own their data.

Independent plan and final code reviews passed. All 49 access-log tests and four
canonical cookie-parser tests passed. Allocation guards verify a bounded prefix
allocation and no value allocation for rejected fields; source-resolution tests
verify borrowed header/cookie/initialized-query/ctx values. Formatting, workspace
check, workspace Clippy and seven repository guards passed. Clippy warnings are
outside the changed files. No full integration suite was run for this batch.
The optimization removes temporary full-source copies; no throughput measurement
or change to the logger channel/serialization policy is claimed.

## Redirect hostname normalization

The redirect request callback previously constructed the same normalized hostname
for both its missing-host guard and listener-admission check. Both private
predicates now borrow one typed normalized value. The raw authority still owns
Location construction and log fields; reusing normalized text there would lose
IPv6 brackets, case and trailing dots. Full query-bearing Location and query-free
log URL remain separate projections. Connection persistence uses a zero-sized
marker and the shared parsed client policy; no additional configuration cache or
keepalive change is needed.

Independent plan and final code reviews passed. All 13 redirect tests passed on
the final source, including in-memory wire checks for IPv6, mixed-case/FQDN
admission, query separation, missing/unowned hosts and connection reuse.
Formatting, workspace check/Clippy and seven repository guards passed; Clippy
warnings remain outside the changed files. The normalized value is explicitly
released before any response-write await. No full integration suite was run.
This removes one temporary hostname normalization for nonempty hosts; no
throughput improvement is claimed without measurement.
