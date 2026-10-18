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
| Connection filters and Stream plugins | Connection bridge, ordered runtime dispatch, connection-rate-limit callback, global IP profile selection, GeoIP callback and shared denial translation inspected. Static denial reasons now borrow literals. Deeper ACL/GeoIP matchers, local-gate/Redis implementations and publication construction remain open. |
| TCP/TLS connection handling and relay | TCP, terminated TLS and passthrough TLS direct process/route/connect/log orchestration inspected, plus shared L4 relay. Existing snapshot/inspection-buffer release boundaries retained. Listener-log direct serialization deferred below. Deeper route selection, backend resolution and transport dependencies remain open. |
| TLS handshake callbacks | Direct select/certificate/completion/free callbacks and certificate/mTLS/cipher application helpers inspected. Temporary SNI and early-abort metadata copies removed below. Compiled certificate ownership, expiry checks and ex-data handoff retained. Client-certificate extraction and leaf-only verification orchestration inspected below. Deeper matcher, allowlist evaluator and verification extraction cost remain open. |
| UDP packet/session lifecycle | Direct packet, session admission, receiver, retirement, shutdown and log projection inspected. Selected route ownership and socket Arc transfer tightened below. Deeper backend selection, route publication and socket dependencies remain open. |

## Deferred decisions

- [Publication-time global/Gateway plugin lists](early-01-precompute-plugin-lists.md)
- [Cache policy fixture verification failure](verification-01-cache-policy-fixture.md)
- [Stream-suite parallel verification failure](verification-02-stream-parallel-failure.md)
- [Prebuilt read-only response-body dispatch](body-01-prebuild-readonly-dispatch.md)
- [Direct TLS listener log serialization](tls-01-direct-listener-log-serialization.md)

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

## Stream rejection reason ownership

`StreamPluginResult::Deny` now holds `Cow<'static, str>`: fixed IP, connection
rate-limit and GeoIP reasons borrow literals, while group-specific and compiled
fail-closed reasons remain owned. The canonical AccessDecision mapper retains
exact text and invokes its denial callback once. GeoIP metric labels retain
underscores independently of the existing hyphenated lookup-failure text.
Stage-1 silence, TCP/UDP bounded reasons and TLS masked detail are unchanged.

The connection bridge pins one ready resource and drops its publication before
awaiting; runtimes borrow plugin vectors. Global profile selection borrows the
selected name from one captured snapshot. Rate-limit durations and entry scope
are already resolved at construction; its small per-call policy uses an empty
String without heap allocation. No configuration cache, limiter semantics or
logging behavior was changed.

Independent plan and source reviews passed. The Stream suite passed serially
(112 passed, one existing ignored test), and all 14 TLS consumer tests passed.
The first parallel Stream run had one failure; that case passed in isolation.
Its cause is not established and no baseline binary comparison was run; see the
deferred verification task above. Formatting, workspace check/Clippy and seven
repository guards passed, with Clippy warnings outside the changed files.
No full integration suite was run.
The allocation claim concerns fixed reason strings only, not whole asynchronous
plugin execution or dynamic policy/group reasons. No throughput claim is made.

## TCP/TLS connection and relay review

No production change selected in this batch. TCP retains its selected route unit
rather than the whole route table across I/O, and owns its formatted upstream
address in the log context.
TLS drops the publication and executable plugin view after their required use;
selected route/owner state remains available to connect retries. Passthrough
forwards all consumed initial bytes, including trailing data, through an owning
replay helper that releases the inspection buffer before the long-lived relay.
The PP2 helper receives the bracketed socket-address string, not a reconstructed
bare IPv6 address from log fields.

The canonical relay owns two bounded 8 KiB buffers and preserves partial-write
cursors, independent directional progress, downstream flush and a shared idle
clock. Replacing it with a simple copy loop, shrinking buffers or removing flushes
would need separate behavioral/performance evidence; none is proposed here.

Fresh existing tests: TLS 85, TCP 27 and shared relay 21 passed. Formatting,
workspace check/Clippy and agent-doc validation passed; Clippy warnings are
outside this documentation-only change. No real-network or throughput validation
is claimed. Independent plan review approved recording the listener-log candidate
for user decision; independent final record review also passed.

## UDP selection and session ownership

New-session route selection now returns the selected Arc from a synchronous
helper, releasing the whole table snapshot before admission and other I/O.
Selection stays after the established-session fast path and retains the same
selected generation. Session construction moves its newly created upstream
socket Arc instead of cloning it; the receiver still owns its independent Arc.

Established sessions retain their fixed upstream. Session handles across awaits,
retirement identity checks, lifecycle locks, permits, cancellation and shutdown
ordering remain unchanged. Packet buffers remain bounded; upstream source checks
still precede forwarding and activity updates. No new observability was added.

The ownership regression covers empty selection, old-table and unselected-unit
release, selected-generation survival, and subsequent selection after reload.
This removes one socket Arc increment/decrement and shortens table retention;
no throughput or RSS improvement is claimed without measurement.

Independent plan and final source reviews passed. All 58 UDP route/session tests
and four UDP log tests passed. Formatting, workspace check/Clippy and seven
repository guards passed; Clippy warnings are outside the changed files.
No full integration suite or throughput benchmark was run.

## TLS callback temporary copies

The select-certificate callback moves its resolved SNI into connection metadata
and borrows that string for matching, avoiding a second temporary String. Client
SNI lowercasing, the exact configured fallback value, matching order and error
logs remain unchanged. The SSL free callback owns its recovered Box and now fills
a missing early-abort error directly in its metadata before synchronous logging,
instead of cloning that metadata. Its null/tls_id gates and redacted panic
boundary remain unchanged; the logger queues an owned serialized String.

The accepted borrow-and-clone ex-data store and verify-time metadata peek remain
unchanged. Certificate-callback Arc handles still support mutable context use;
completion moves metadata into the digest. Certificates, keys, CA stores and
cipher text are already compiled at publication. Runtime expiry checks and
security-policy-before-identity installation remain necessary. No additional
precompilation or callback-order change was selected.

These are source-level copy reductions, not measured throughput improvements.
Existing focused tests and compiler checks validate surrounding consumers;
this batch does not establish complete handshake-log lifecycle coverage.

Independent plan and final source reviews passed. All 15 gateway TLS runtime
tests passed. Formatting, workspace check/Clippy and seven repository guards
passed; Clippy warnings are outside the changed files. No full integration suite
or throughput benchmark was run.

## Client-certificate extraction

Subject rendering now captures the first CN during its existing entry traversal,
reusing that entry's UTF-8 conversion. The first-CN marker is independent of decode
success: an unsupported first CN still produces no CN, even if a later CN is
usable. Subject order, separators and invalid-entry placeholders remain unchanged.
BMP OtherName decoding now consumes big-endian UTF-16 units directly, removing
the intermediate u16 vector while retaining the odd-length rejection and existing
UTF-16 semantics, including accepted surrogate pairs and embedded NULs.

The canonical extractor serves both handshake metadata and HeaderCertAuth.
Ordinary typed SAN enumeration and the separate OtherName DER pass retain their
order and projections. The mTLS callback still evaluates allowlists only at leaf
depth and retains its terminal-audit claim. Its formerly eager full metadata extraction before typed SAN validation is
reduced in the following batch. CN validation still uses the full canonical
projection; authorization and failure diagnostics remain unchanged.

New signed-certificate regressions cover duplicate CN order/selection and an
unsupported GeneralString first CN followed by a valid CN. Subject/CN preserve
BoringSSL OpensslString C-string truncation at NUL; OtherName BMP keeps embedded
NULs. Direct BMP regressions cover the existing UTF-16 acceptance boundaries.
These changes remove a subject traversal/repeated CN conversion and an intermediate
BMP allocation; they are not measured throughput improvements.

Independent plan and final source reviews passed. The final-source focused tests
passed: certificate extraction 11, HeaderCertAuth 20 and mTLS callback 10.
Formatting, workspace check/Clippy and seven repository guards passed; Clippy
warnings are outside the changed files. No full integration suite or live mTLS
handshake verification was run for this batch.

## mTLS allowlist projection timing

The leaf allowlist callback now acquires the peer certificate once. Its private
synchronous certificate helper always rejects a missing certificate first, even
when constraints are absent or empty. Nonempty SAN constraints use the existing
typed projection and matcher. Full subject/CN/display-SAN/fingerprint extraction
runs only for a SAN rejection or a nonempty CN constraint; SAN-only success and
unconstrained validation no longer create that unused metadata. Empty lists keep
the canonical validators' unconditional acceptance for a present certificate.

Failure branches retain the same canonical extractor, diagnostic formatting and
sensitive-value wrappers. SAN rejection still precedes CN validation. The
callback's leaf-depth classification, panic boundary, context ownership and
terminal-audit claim are unchanged. The helper is an internal in-memory test seam,
not a new certificate-validation API. No serving-identity cache or precompilation
of peer identity was introduced.

New signed-certificate tests exercise missing/present certificates, absent/empty
constraints, SAN-only/CN-only/combined success, rejection precedence, exact
case-sensitive CN failure, missing CN, typed URI-versus-DNS distinction and
existing masked diagnostic formatting. This is removal of unused projection work,
not measured throughput improvement or a live handshake proof.

Focused tests passed: mTLS callback 14, mTLS validators 6, certificate extraction
11 and HeaderCertAuth 20. Formatting, workspace check/Clippy and seven repository
guards passed; Clippy warnings are outside the changed files. Independent plan
and final source reviews passed. No full integration suite or throughput
benchmark was run.
