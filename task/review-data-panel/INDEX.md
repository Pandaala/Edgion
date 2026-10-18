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
| `new_ctx`, `init_downstream_modules` | Direct bodies and context/default construction inspected: pinned config, parsed policy, absent optional owners and lazy containers. Pinned-fork module construction inspected; outer module storage is deferred below. Compression/bridge processing internals remain open. |
| `early_request_filter` | Direct body and request-ID resolution inspected. Arc handles support mutable context dispatch; do not blindly replace with references. Publication-time list resolution deferred below. Trace-ID parsing/generation/reconciliation and direct preflight runtime inspected below; deeper source adapters remain open. |
| `request_filter` | Temporary metadata path copy removed in `4a976e022`. XFF snapshot preserves bounded aggregate size and owned log state; RealIp extractor borrows tokens and traverses in reverse. TLS metadata copies feed request-owned state. Routing, plugin dispatch and deeper RealIp policy dependencies remain open. |
| Upstream selection, connect, upstream request | `connected_to_upstream` direct body inspected: borrowed SAN digest, one timestamp, owned connection-establishment log. `upstream_request_filter` direct body and deferred credential nomination guard traced to shared Connection parser. Direct upstream deadline/backoff, concurrency admission and authority orchestration inspected; admission borrows its Service key below. Direct HTTP selection, sticky retry/fallback, timeout folding, policy capture, DirectEndpoint/ExternalJump and URI finalization inspected. ExternalJump borrows its domain below. Service peer dispatch and shared TLS metadata projection inspected below. Upstream request credential guards, deferred mutation ownership, request-ID reconciliation and transcoder attempt header validation inspected below; deeper endpoint algorithms, body conversion and credential publication remain open. |
| Request body, trailers and relay | Direct action/tee/trailer orchestration, mirror payload budget and observer dispatch inspected. Storage scalar queries, memory scan ownership, digest caching and text predicate updates inspected below. Literal pattern preparation extends the existing DSL ownership decision. Nullable-state fast path, moved handler vector, bounded independent mirror copy and terminal-only log drain retained. WAF structured capture adapter and its usage-accounting accessors inspected below; deeper WAF rule/parser execution, other concrete streamed handlers and remaining storage/relay integration remain open. |
| Response headers, hold/commit, body and trailers | Connection sanitizer inspected. Body/trailer orchestration, runtime dependency checks, processor driver and downstream cancellation wrapper inspected; prebuilt read-only runtime-chain candidate deferred. Concrete processors, conditions, hold/commit and remaining header chains remain open. |
| Errors, retries, logging and connection reuse | Terminal logging direct body and extension projection inspected. Borrowed bounded projection implemented below. Connect/proxy retry direct bodies inspected; retained per-attempt errors and peer context remain owned. Terminal error rendering direct body inspected: exact gRPC denial is consumed, protocol-specific replies and final-status ownership retained. Deeper finalizers and retry consumers remain open. |
| Redirect HTTP | Direct callbacks and shared client-policy application inspected. Missing-host and listener-admission checks now share one normalized hostname. Raw Location authority, query-free log URL and session-owned keepalive remain unchanged. |
| Connection filters and Stream plugins | Connection bridge, ordered runtime dispatch, connection-rate-limit callback, global IP profile selection, GeoIP callback and shared denial translation inspected. Static denial reasons now borrow literals. Deeper ACL/GeoIP matchers, local-gate/Redis implementations and publication construction remain open. |
| TCP/TLS connection handling and relay | TCP, terminated TLS and passthrough TLS direct process/route/connect/log orchestration inspected, plus shared L4 relay. Existing snapshot/inspection-buffer release boundaries retained. Listener-log direct serialization deferred below. Deeper route selection, backend resolution and transport dependencies remain open. |
| TLS handshake callbacks | Direct select/certificate/completion/free callbacks and certificate/mTLS/cipher application helpers inspected. Temporary SNI and early-abort metadata copies removed below. Compiled certificate ownership, expiry checks and ex-data handoff retained. Client-certificate extraction and leaf-only verification orchestration inspected below. Matcher snapshot selection and shared SAN/CN allowlist evaluation inspected below. Prepared SAN policy is deferred; deeper verification extraction and remaining matcher costs remain open. |
| UDP packet/session lifecycle | Direct packet, session admission, receiver, retirement, shutdown and log projection inspected. Selected route ownership and socket Arc transfer tightened below. New-session L4 policy/Service selection handoff and route table/unit ownership inspected; section borrowing candidate skipped below. Deeper selection algorithms, publication helpers and socket dependencies remain open. |

## Deferred decisions

- [Quota worker future representation](quota-01-worker-future-representation.md)
- [Publication-time global/Gateway plugin lists](early-01-precompute-plugin-lists.md)
- [Cache policy fixture verification failure](verification-01-cache-policy-fixture.md)
- [Stream-suite parallel verification failure](verification-02-stream-parallel-failure.md)
- [Backend TLS verification failures](verification-03-backend-tls-tests.md)
- [Prebuilt read-only response-body dispatch](body-01-prebuild-readonly-dispatch.md)
- [ExtProc protobuf body representation](body-02-extproc-protobuf-body-storage.md)
- [Request-body mutation snapshot retirement](body-03-mutation-snapshot-retirement.md)
- [Direct TLS listener log serialization](tls-01-direct-listener-log-serialization.md)
- [Pending response-header execution ownership](response-01-pending-header-ownership.md)
- [ResponseRewrite rename value fidelity](response-02-rename-value-fidelity.md)
- [ExtProc local-reply value ownership](response-03-extproc-local-reply-value-ownership.md)
- [Service selected-slot lookup cost](backend-01-selected-slot-lookup.md)
- [JWT claim condition evaluation ownership](condition-01-jwt-claim-evaluation.md)
- [GeoIP record decoding and projection](geoip-01-record-decoding.md)
- [Redis compiled-policy validation ownership](redis-01-compiled-policy-validation.md)
- [HMAC signed-header membership ownership](hmac-01-signed-header-membership.md)
- [Preinitialized HMAC key state](hmac-02-preinitialized-mac-state.md)
- [Raw URI fragment serialization](log-01-raw-uri-fragment-serialization.md)
- [Wasm header snapshot and replay ownership](wasm-01-header-snapshot-ownership.md)
- [DSL string consumer ownership](dsl-01-string-consumer-ownership.md)
- [DSL outbound method normalization](dsl-02-http-method-normalization.md)
- [Endpoint probe result address ownership](endpoint-01-result-address-ownership.md)
- [Outbound Content-Length integer storage](outbound-01-content-length-integer-storage.md)
- [ProxyRewrite staged template compilation](rewrite-01-staged-template-compilation.md)
- [Repeated GraphQL frontend parsing](graphql-01-repeated-frontend-parsing.md)
- [Prepared SAN allowlist policy](tls-01-prepared-san-policy.md)
- [Response-cache arena initialization timing](cache-01-arena-initialization.md)
- [Downstream HTTP module context storage](context-01-downstream-module-storage.md)

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

## Upstream concurrency-admission Service key

`upstream_peer` now borrows the backend context's Service key through synchronous
circuit-breaker admission instead of cloning it before checking whether a cap or
endpoint exists. The only use is the shed metric call; its borrow ends before the
503 response await. The metrics facade still owns its label on the shed branch.
This removes the caller's temporary String copy, not all metric allocations.

The captured effective policy, prior-permit release, endpoint Arc ownership,
permit acquisition and HTTP/gRPC shed response branches are unchanged. Authority
rendering retains its compiled Arc because the session adapter mutably borrows
context; peer, log and request authority strings keep their distinct owners.
Deadline/backoff order and post-sleep checks remain unchanged. No new helper or
mirror test was introduced for the local borrow adjustment, and no throughput
claim is made.

Independent plan and final source reviews passed. All 103 upstream-peer tests
and five circuit-breaker permit tests passed. Formatting, workspace check/Clippy
and seven repository guards passed; Clippy warnings remain outside the changed
files. No full integration suite or performance benchmark was run.

## ExternalJump domain ownership and peer orchestration

ExternalJump now borrows the domain from its request-owned jump preset across
`resolve_domain` instead of cloning the string solely for that call. The preset
remains owned by `ctx.jump` through the await. Peer construction still owns its
SNI; DNS error propagation, first-address selection, backend-context seeding and
post-resolution SSRF checks remain in the same order. No DNS result cache or
configuration-time resolution was introduced.

The remaining direct upstream-peer orchestration was inspected. Route-unit and
backend-plugin Arcs permit mutable request-context use during selection and
plugin dispatch. Strict sticky retries retain their endpoint address and fail
closed; non-strict fallback refreshes the session binding. Parsed timeouts,
request-captured policy presence/absence, permit release and deadline folding
remain unchanged. DirectEndpoint retains its pinned Service proof and TLS policy;
ExternalJump has no Service policy. Deeper LB/peer and plugin internals remain
separate review work.

This removes one temporary domain String per external peer attempt. Existing
resolver tests use IP literals, localhost and an empty host; they do not establish
external DNS integration or measured throughput gains.

Independent plan and final source reviews passed. All 103 upstream-peer tests
and 14 network utility tests passed. Formatting, workspace check/Clippy and seven
repository guards passed; Clippy warnings are outside the changed files.
No full integration suite or performance benchmark was run.

## Borrowed upstream TLS metadata projection

The canonical `extract_tls_config` returns the policy hostname by reference.
Ordinary Service, self-resolved ExternalName and DirectEndpoint metadata writes
consume that reference synchronously. The AI path likewise borrows either the
retained policy hostname or the selected endpoint SNI. Peer constructors still
own their separate SNI String; `record_tls_to_upstream` owns the final log field
and continues clearing it on plaintext attempts. Invalid-policy rejection,
write order, CA/identity preparation, SAN hooks and pool keys are unchanged.

A focused regression verifies effective-policy pointer identity, final log
ownership after the lookup is dropped, and Absent/Invalid projection values plus
TLS-to-plaintext clearing. This removes temporary metadata SNI copies; it does
not eliminate peer/log ownership allocations or establish measured throughput.

BackendSelector is generation-compiled; HTTP/gRPC selectors return a slot index.
Smooth weighted round robin precomputes its sequence and advances an atomic
cursor per selection. Service peer dispatch retains one selected generation and
captures Service geometry for endpoint/DNS work. Gateway/gRPC handles support
mutable context use; sticky selection retains the live backend owner used by
connection accounting. No selector algorithm or publication boundary changed.

Independent plan and final source reviews passed. Upstream-peer tests (103) and
AI attempt tests (6), formatting, workspace check/Clippy and seven repository
guards passed. Backend tests: parallel 334 passed/3 failed; serial 336 passed/1
failed. The pre-change `7abcce8af` baseline ran serially with 335 passed and the
same DNS admission-fixture failure. The two parallel-only failures remain
unexplained; see the linked backend TLS verification task. Existing Clippy
warnings remain outside the changed lines. No full integration suite or
performance benchmark was run.

## Borrowed Connection options for credential checks

The shared Connection parser now exposes trimmed, nonempty borrowed options.
Its owned-list API still lowercases and deduplicates for existing consumers.
The final upstream credential guard and CredentialInjector consume borrowed
options directly for their pure case-insensitive existence checks, avoiding
intermediate token Strings and lists. Both request views, repeated fields,
UTF-8 rejection, credential expiry, error precedence and materialization order
remain unchanged. The final-hook regression measures zero allocations on both
allowed and nominated-target paths with mixed case, empty and repeated options.

The upstream-request dependency audit also inspected deferred mutation storage,
request-ID reconciliation and gRPC transcoder attempt preparation/validation.
Credential/header clones retain attempt-local ownership and sensitivity; the
saved gRPC timeout detects mutations across later header writers. Transcoder
request length and deadline framing remain attempt-local. No ownership or
protocol change was selected in those paths. Deeper transcoder body conversion
and credential publication remain outside this batch's coverage.

Independent plan and source reviews passed. All 15 upstream-request hook tests,
7 common HTTP-client helper tests and 7 CredentialInjector tests passed,
including the zero-allocation regression. Formatting, workspace check/Clippy
and all seven repository guards passed. Clippy warnings are outside the changed
lines. No integration suite or throughput benchmark was run.

## Borrowed response Connection nominations

The response sanitizer consumes the canonical borrowed Connection tokenizer
and checks retained names case-insensitively before constructing a HeaderName.
It no longer builds a temporary String list for each field line or allocates a
second HeaderName for repeated nominations. The retained owned name list still
supports atomic validation before response mutation. All field lines, invalid
ASCII token skipping, non-text rejection, 101 preservation, Transfer-Encoding
tracking and static hop-header removal keep their existing behavior.

A regression compares allocation counts and final headers for identical unique
nominations with and without mixed-case duplicates in an additional field line.
This establishes duplicate-allocation stability, not overall throughput.
Repeated options scan the retained name list; removing the per-field temporary
deduplication changes comparison work for duplicate-heavy inputs.
Response plugin Arc owners, the two sanitizer/framing boundaries, local-reply
write/abort ordering and final processor-driver freeze remain unchanged.

Independent plan and source reviews passed. All 23 response-filter tests and
7 common parser tests passed. Formatting, workspace check/Clippy and seven
repository guards passed; existing Clippy warnings are outside the changed
lines. No full integration suite or throughput benchmark was run.

## Response header ownership audit

Inspected pending-header application/drain, direct stream-processor header
callbacks, response-cache lifecycle capture/seal/delivery/finalization, and
storage Pending creation plus publication gates. Stream-processor header calls
already borrow installed owners directly. Cache capture serializes bounded
metadata into an admitted pending entry; final header sealing validates its
contract rather than replacing it with a borrowed live header. Publication
retains origin/epoch checks and awaits terminal transport completion. These
owners span callbacks or cache lifetime; no ownership change was selected.
Deeper cache key, freshness, storage eviction and concrete processor internals
remain open.

Pending-header replay unconditionally clones before discovering protected,
Remove or false-condition operations. Shared borrowed/owned execution and any
name-preparation extension require the explicit design choice recorded above.
This documentation-only batch does not change production behavior or tests.

## Borrowed non-SSE usage chunks

UsageExtractionProcessor now borrows its input for synchronous observation and
clones Bytes only when handing an SSE chunk to the retaining decoder. The
non-SSE JSON/character observers borrow bytes directly, and ordinary forwarding
restores the original owner. SSE raw fallback, semantic batches, overflow,
encoding diagnostics, quota gates and terminal precedence are unchanged.

The regression warms JSON parser state, proves a fresh boxed-slice Bytes clone
allocates, then measures one ready poll of the actual non-SSE callback with
setup outside the counter. It compares complete allocation statistics against
an otherwise identical static-backed callback and checks original pointer/byte
preservation. The first absolute-zero assertion observed three callback-path
allocations; their source was not established. The differential measurement
isolates first-clone allocation; reduced reference-count work follows from
source inspection. This is not a throughput measurement or an allocation-free
claim for JSON parsing.

Concrete processor entry points inspected: Wasm observer, ExtProc streamed
response, native Anthropic Messages, generic usage and AI attempt composite.
Wasm retains a Bytes handle for host callback state; ExtProc needs its bounded
protobuf Vec copy across the awaited exchange; native Messages and SSE usage
retain decoder-owned raw slices. Composite dispatch iterates existing owners
and stops after terminal sink state. Their deeper host ABI, remote exchange,
provider JSON and terminal consumers remain open. No response aggregation or
new processor mechanism was introduced.

Independent plan and source reviews passed. After correcting the allocation
fixture and its comparison scope, all 50 usage and 77 SSE-filtered tests passed.
Formatting, workspace check/Clippy and seven repository guards passed; existing
Clippy warnings are outside the changed lines. No full integration suite or
throughput benchmark was run.

## Lazy SSE raw-handle iteration

WindowRecord::raw_slices_cloned now yields cloned Bytes handles lazily instead
of first cloning the whole handle Vec. CanonicalRecordBatch's two fallback
loops consume the same ordered slices while retaining each original record.
Failure returns still clone the original record as before; raw/metadata caps,
fallback classification, coalescing and admission are unchanged.

A static-slice regression measures zero iterator/output-array allocations,
preserves empty slices and byte order, checks unchanged source and pointer
identity, and keeps yielded owners after the source record is dropped. Owned
Bytes can still allocate on first-clone promotion; the removed cost is the
intermediate handle vector, not every possible backing-owner operation.
The audit also inspected batch record/metadata fallback and SliceChain's append,
coalescing, normalization and range-projection ownership. No normalization,
resident-budget or consuming fallback-output redesign was selected.

Independent plan and source reviews passed. SSE-filtered tests (78), canonical
SSE tests (9, also included in the SSE filter), and semantic response tests (21)
passed. Formatting, workspace check/Clippy and seven repository guards passed;
existing Clippy warnings are outside the changed lines. No full integration
suite or throughput benchmark was run.


## Borrowed listener owner during route eligibility

The canonical listener matcher returns a borrowed Arc tied only to the supplied
GatewayInfoIndex. HTTP and gRPC obtain the final owned handle after remaining
method/header/query predicates succeed. TLS clones its selected winner; the
stream bridge clones only after the same publication's gateway scope is available.
Parent order, listener isolation, namespace/kind checks, predicate ordering and
snapshot boundaries are unchanged. No index slot or owner is cached across updates.

Named and unnamed listener tests compare reference counts, preserve pointer
identity and retain the explicitly selected owner after dropping the index.
The route compiler, regex/radix candidate order and preflight dispatch were also
inspected. Existing cold compilation and ordered candidates remain; the existing
predicate-prefilter decision still lacks measured fanout evidence for an index.
This removes premature reference-count work, without a throughput claim.

Independent plan and final source reviews passed. Matcher (23), HTTP (19), gRPC (11), TLS (16),
stream bridge (6), regex (17) and radix (23) filtered tests passed. Formatting,
workspace check/Clippy and seven repository guards passed; Clippy warnings are
outside the changed lines. No full integration suite or throughput benchmark ran.


## Service selector and selected-slot projection audit

Inspected captured Service selection, post-selection slot projection, sticky
address recovery, compiled RR/CH ownership and P2C dispatch. Selector construction
and address maps are already cold; P2C clones only its winning backend handle.
The ordinary slot-identity scan and sticky socket scan remain candidates with
representation, memory and invariant tradeoffs, recorded in the deferred task.
Current normalization's sorted unique sockets are an alternative worth evaluating
before adding a map. No algorithm, health/recovery or ownership change was made.
This evidence does not complete deeper discovery or endpoint runtime review.
Independent plan and final source reviews passed. Agent-doc validation and diff
whitespace checks passed. This documentation-only batch ran no Rust tests or
benchmarks; earlier test results are not fresh verification of this proposal.


## Borrowed RR and consistent-hash intermediate winners

RoundRobinSelector and ConsistentHashRing now return a Backend borrowed from
their own immutable storage. RrCore and ChCore use that reference for the same
address-map resolution and retain the final owned Arc<LbBackend>. This removes
one intermediate Backend clone on successful selection without changing health
predicate calls, weighted ordering, ring traversal, distinct-candidate budgets,
empty-key RR fallback, atomic cursor behavior or final generation ownership.

New primary/fallback tests verify pointer identity with selector-owned storage.
Existing distribution tests explicitly copy addresses when they need owned map
keys or comparison values. The removed clone includes Backend's address and
Extensions cloning; the amount of work depends on their contents. This is not
an allocation-free selection or throughput claim: CH's large SeenSet remains.
The separate selected-slot lookup proposal stays deferred.

Independent plan and final source reviews passed. Selection tests (38), compiled
selector tests (8) and backend selector tests (12) passed. Formatting, workspace
check/Clippy and seven repository guards passed; Clippy warnings are outside the
changed lines. No full integration suite or throughput benchmark was run.


## Borrowed affinity-map build inputs

The ordinary Service affinity-cache miss builder now projects borrowed backends
directly from its captured section instead of cloning every backend Arc into a
temporary Vec. The shared private builder takes an exact-size iterator, reserves
the same capped map capacity and consumes only the first 1024 input positions
before Inet filtering. Order, canonical address/token derivation and duplicate
insertion behavior remain unchanged. Scoped views still own the closure's returned
slice for synchronous construction and remain uncached.

The existing miss lock, generation/membership checks before and after building,
publication and cache-hit paths remain unchanged. A regression verifies prefix
consumption, live reference counts during iteration, retained entries and omitted
tail. Existing IPv4/IPv6, scope-isolation and stale-publication tests remain.
Source inspection also traced token extraction/expiry and rule-wide sticky lookup;
request-owned tokens and response cookie formatting remain unchanged. This removes
the ordinary miss-path temporary vector and Arc work, not map/token allocations or
all discovery work. No cache prebuilding or throughput claim is introduced.

Independent plan and final source reviews passed. Runtime-state tests (7),
session-persistence tests (22) and discovery-coordinator tests (41) passed.
Formatting, workspace check/Clippy and seven repository guards passed; existing
Clippy warnings are outside the changed lines. No full integration suite or
throughput benchmark was run.


## Concrete request-body consumer audit

Inspected ExtProc streamed request handler and its response-window counterpart,
plus buffered request phase admission and the owned message send/await boundary.
Their protobuf body copies are tracked as a separate representation decision;
no transport or mutation behavior changed. Digest scanners (MD5/HMAC/SHA-256)
already consume borrowed slices. WAF structured capture borrows the current
window and accounts bounded inspection state; its deeper engine remains open.
BodyStorage's scan dispatch retains an owned snapshot/scanner/permit when entering
the blocking pool, while small memory scans can execute synchronously. Those
cross-task owners remain necessary. Observer dispatch preserves per-request
budgets, terminal bytes and panic-protected finalization; no change was selected.
This source audit does not claim complete storage, parser or ExtProc transport
coverage, and historical test results are not fresh verification.
Independent plan and final source reviews passed. Agent-doc validation and diff
whitespace checks passed. No Rust tests or benchmarks ran for this documentation
batch.


## Bounded RandomHex generation scratch

RandomHex correlation-ID generation uses a 64-byte stack array after the same
8..=64 runtime count check, replacing its temporary random-byte Vec. The loop
makes the same ordered per-byte RNG calls and lower_hex receives only the used
prefix. Final string/header ownership, prefix handling and rejection remain.
Boundary tests exercise direct runtime invalid counts (including usize::MAX),
valid minimum/default-sized/maximum output and lowercase hexadecimal suffixes.

The audit inspected W3C parsing/generation, custom rule compilation, trace-state
validation, bounded correlation ownership, suppression, reconciliation and masked
serialization. Header names are already compiled; forwarding keeps owned header
values because wire maps and request identity have independent lifetimes.
This removes one temporary allocation for configured RandomHex generation; it is
not an allocation-free generation or throughput claim. Other generators and the
canonical W3C root generator are unchanged.

Independent plan and final source reviews passed. Trace-ID tests (29) and
request-ID-filtered tests (15) passed. Formatting,
workspace check/Clippy and seven repository guards passed; existing Clippy
warnings are outside changed lines. No full integration suite or throughput
benchmark was run.


## Owned correlation strings become header values

Three RequestId conversions now move their already-owned String into HeaderValue:
the generated prefix/suffix value, validated combined tracestate, and a non-header
KeyGet source in the session adapter. The locked http 1.5.0 implementation applies
the same byte validation and initializes the same sensitivity flag for both APIs.
All rule ordering, eager source extraction, limits, masking and reconciliation
remain unchanged. The source strings are local construction or clones, not buffers
removed from request caches.

This avoids copying string contents into a second byte buffer. The Bytes owner
may retain spare String capacity and allocate a control block; TryFrom<String>
constructs that owner before validation, so even rejected input may incur that
metadata cost. This is not an unconditional allocation-count, memory or throughput
improvement claim. Existing generator, tracestate, source and lifecycle tests are
used instead of duplicating the dependency's conversion tests.

Trace-ID (29), request-ID (15) and early-request-filter (17) filtered tests passed
(the latter filters may overlap). Formatting, workspace check/Clippy and seven
repository guards passed; existing Clippy warnings are outside changed lines.
No full integration suite or throughput benchmark was run.
Independent plan and final source reviews passed.


## Shared condition evaluation audit

Inspected compiled condition construction and synchronous/async dispatch,
log-less per-chunk response wrapping, borrowed key/mTLS reads and JWT extraction.
Regex initialization, time bounds, descriptors and dependency binding already
occur during compilation; test-only schema evaluation helpers do not describe
production dispatch. Live input semantics and skip/run ordering remain unchanged.
JWT context copying/parsing and eager candidates remain open under the deferred
ownership task, including their unresolved conflict with the no-per-chunk-allocation
constraint. This is not an accepted exception or complete condition/auth audit.

Independent plan and final source reviews passed. Agent-doc validation and diff
whitespace checks passed. This documentation-only batch ran no Rust tests or
benchmarks.

## Borrowed response-cache variation values

RequestState::bind now borrows the request-header value for an already checked
Header variation source, instead of copying it through the owned KeyGet adapter.
HeaderMap::get and HeaderValue::to_str preserve first-value selection, case
handling and invalid-text rejection. Presence framing still distinguishes an
empty value from absence. Representability, certificate updates and hash framing
stay in the same order; the borrow is consumed synchronously.

Storage inspection retained preadmitted body/metadata capacity, reader-held arena
leases, strict decode checks, identity-checked eviction and epoch-fenced terminal
publication. These owners and locks protect distinct lifetime and consistency
requirements. This removes a temporary value copy for configured variations;
it is not a throughput result or complete cache key/freshness/storage audit.

Formatting, workspace check/Clippy and seven repository guards passed; Clippy
warnings remain outside the changed code. Cache module tests passed (36).
Actual-tail cache tests passed nine cases and reproduced the already deferred
policy admission fixture failure in verification-01, also in an isolated exact
run. The fixture still omits preparedAncestorAdmission and uses empty configured
variation; the changed projection is not executed in that case. Attribution is
source-based, not a fresh baseline binary comparison. No full integration suite
or throughput benchmark ran.
Independent plan and final source reviews passed, including the unchanged
fixture failure attribution.

## Single-pass cache-control lifetime extraction

Freshness construction now consumes the lifetime returned by the private supported
header validator, avoiding a second singleton lookup and strict Cache-Control
parse on the same immutable response. Boolean consumers still use the same
validator. Status/count, forbidden fields, content type, encoding, Vary and
Cache-Control checks retain their order. Date/Age validation, monotonic age and
max-TTL clamping remain unchanged. Each capture/decode/seal/publication boundary
still validates its own header; no parsed state is cached across mutations.

The key/freshness audit also inspected bounded header sorting/framing, Connection
exclusions, Vary certificates and origin stamps. Header values remain live request
inputs, and freshness uses the origin timestamp rather than restarting on replay.
Deeper publication dependencies and repeated exclusion construction remain open.
This removes duplicate parsing work, without a measured allocation or throughput
claim. A TTL boundary regression covers zero, lower, equal and higher caps.

Independent plan and final source reviews passed. All 37 cache module tests,
formatting, workspace check/Clippy and seven repository guards passed. Existing
Clippy warnings are outside changed lines. No full integration suite or throughput
benchmark ran; the previously deferred request-tail fixture issue remains open.

## Borrowed bounded Connection exclusion index

Cache-key exclusions now retain at most 16 borrowed Connection tokens in a fixed
stack array instead of allocating a Vec and lowercase Strings. The existing
single-field, 512-byte and 16-option grammar gates still run before the canonical
token iterator. First-occurrence case-insensitive deduplication and membership
remain unchanged; duplicates still count against the raw grammar limit.

The index lives only within a synchronous header-key or variation lookup. It does
not cache across body scanning, header mutation or callbacks. Repeated exclusion
construction remains a separate open optimization; the fixed index removes its
heap-owned token representation, not all repeated work. Tests cover every slot,
mixed-case duplicates and 17 duplicate options, with fixture construction outside
allocation counters. The existing malformed-field and size-bound tests remain.
No throughput improvement is inferred from allocation counts.

Independent plan and final source reviews passed. All 38 cache module tests,
formatting, workspace check/Clippy and seven repository guards passed. The new
allocation assertions report zero calls for the measured paths. Existing Clippy
warnings remain outside changed lines. No full integration suite or throughput
benchmark ran; previously deferred verification issues remain open.

## Single borrowed IpList lookup after permission checks

RealIp trust evaluation led to the shared ConfigData scalar readers. Successful
membership and lazy ACL matcher iteration previously checked typed-index presence
and then looked up the same key again in one immutable snapshot. A private
snapshot helper now returns the borrowed matcher after the same denied-scope,
key-shape, raw identity, enable, type and visibility gates. Availability remains
presence-only; membership and iteration consume that same lookup result.

Scalar plugin capabilities, dispatch-local snapshot capture, required-root checks,
RealIp all-reference availability and per-hop trust ordering remain unchanged.
No matcher or snapshot loan is exposed to plugins, no Arc clone or new cache is
introduced, and lazy configured matcher order/group identity remain intact.
This removes one typed-index lookup from each readable membership/iterator hit;
it does not eliminate the separate availability preflight or prove throughput.
RealIp string canonicalization and broader decision-time reuse remain open.

Independent plan and final source reviews passed. Final-source tests passed:
ConfigData 22, IP references 7, ACL 4 and RealIp-filtered 32. Formatting, workspace
check/Clippy and seven repository guards passed. An introduced Clippy suggestion
was corrected; remaining warnings are outside the diff. No full integration suite
or throughput benchmark ran.


## GeoIP lookup and projection audit

Inspected HTTP/stream fact-demand compilation, shared CIDR reference dispatch,
lazy direct/remote slots, RealIp invalidation, provider lookup/failure mapping and
MMDB projection. Facts and rule inputs already borrow where their owners permit;
owned projected facts survive database reload and shared request-slot references.
Access logs do not initialize slots. CIDR-only paths avoid database lookup.

The complete City decode and first-subdivision projection expose a possible
transient collection/decode optimization, deferred in the linked task because
narrower decoding can change failure semantics. Borrowed strings and typed Names
are not incorrectly counted as owned translation maps. Updater internals, full
rule evaluation and malformed-record equivalence remain outside this audit.

Independent plan and final source reviews passed. Agent-doc validation and diff
whitespace checks passed. No Rust tests or benchmarks ran for this documentation
batch; no runtime or dependency code changed.

## Stream GeoIP denial-reason traversal

Shared geographic rules normalize ISO-coded values during construction, borrow
facts for linear membership, and short-circuit group matching. Stream denial
reason selection formerly repeated deny matching even when there were no allow
groups. In that case a deny hit and the deny default share deny_country, so the
runtime now selects that reason without traversing the rules a second time.
The canonical first decision, reference availability, MMDB demand/failOpen and
metric/reason strings remain unchanged. Configurations with allow groups retain
their existing reason disambiguation; no shared result API or rule reordering is
introduced. The skipped readers have no mutation or diagnostic side effects.

The existing reason/metric regression now also covers no-allow deny-hit and
default-deny outcomes. This removes redundant work on that rejection path only;
no throughput benefit or complete GeoIP subsystem coverage is claimed.

Independent plan and final source reviews passed. Stream GeoIP tests (11),
shared Geo IP-reference tests (5), resource GeoRule tests (14) and GeoIpLocation
tests (15) passed. Formatting, workspace check/Clippy and seven repository guards
passed; existing warnings are outside the changed code. No full integration suite
or throughput benchmark ran.

## Redis quota TTL diagnostic construction

ConnectionRateLimit inspection retained cold entry binding and per-port coherent
budget transitions. The bucket Arc releases the port-map lock before state locking;
changing this ownership mechanically would alter contention boundaries. Scoped
Redis keys, bounded timeouts, listener-before-source order and failure policies
remain unchanged. Shared script descriptors and exact NOSCRIPT-only fallback
already avoid per-runtime remote preloading.

In the shared executor, cumulative quota TTL validation eagerly formatted its
diagnostic field label even on success. The private integer validator now accepts
a Display value, so TTL callers pass format_args and format only on failure.
The synchronous argument never crosses an await. Integer conversion, zero and
Lua exactness bounds, bucket/marker ordering and error text remain unchanged.
A successful pair now avoids two temporary label allocations; this is not an
allocation-free Redis operation or throughput claim. Other numeric argument/key
construction and deeper Redis client behavior remain open.

Independent plan and final source reviews passed. Shared Redis-core tests passed
29 cases; connection-rate-limit tests passed 30 with one existing measurement
case ignored. The valid TTL pair allocation assertion reports zero calls and the
unsafe-TTL message matches exactly. Formatting, workspace check/Clippy and seven
repository guards passed; existing warnings are outside changed lines. No live
Redis integration, full integration suite or throughput benchmark ran.

## Redis rate-limit numeric arguments

The three ordinary rate-limit algorithms now pass validated i64 arguments into
the canonical private script executor. Sliding-window, fixed-window and token
bucket calls avoid four, three and five temporary numeric Strings respectively.
Validation still precedes conversion; rate/cost bounds fit i64, and timestamps
retain the Lua exact-integer bound. Argument order, fixed-window key formatting,
scripts, error text and exact NOSCRIPT-only retry remain unchanged. Quota and
reservation callers were outside that batch; their extension is recorded below.

Inspection of the locked Fred 10.1.0 source confirmed that both RESP2 and RESP3
command codecs encode integer arguments as decimal bulk/blob strings. Existing
fallback clones retain shared byte owners and containers, rather than copying
all String contents again. Those containers and command encoding still allocate;
this change makes no zero-allocation or throughput claim. Deeper client behavior
and Cluster/failover behavior remain outside this bounded audit.

Independent plan and final source reviews passed. Shared Redis-core tests passed
29 cases; connection-rate-limit tests passed 30 with one existing measurement
case ignored. The existing Redis script lifecycle integration test passed against
an exclusively owned temporary Redis 7.4 standalone instance, covering all five
script families, warm/cache-flush recovery, replacement clients and weighted
concurrent debit. The instance was removed after verification. Formatting,
workspace check/Clippy and seven repository guards passed; existing warnings are
outside changed lines. No full integration suite or throughput benchmark ran.


## Redis batch validation and result ownership audit

Traced HTTP quota preparation and sequential core calls, Stream dimension policy
construction, DSL resolved policy invocation, and core result merging. Results
move through the merger without heap copies. HTTP preparation releases oversized
key backing allocations before Redis awaits; single-source key resolution already
moves its value and empty key lists retain global-bucket semantics. Multi-source
key adapters and composition remain open for deeper inspection.

Repeated numeric policy checks expose a preparation candidate, deferred in the
linked task because the raw-entry and all-policy-before-debit boundaries must be
preserved. Cold textual duration parsing already exists; the repeated work is
bounded numeric validation/conversion. The task distinguishes a private per-call
validated path from a shared immutable compiled policy owner, without accepting
an unchecked public API or claiming measured CPU savings.

Independent plan and final source reviews passed. Agent-doc validation and diff
whitespace checks passed. No production changes, fresh Rust tests or benchmarks
were part of this documentation batch.

## Rate-limit two-source temporary storage

The canonical identity helper serves Local, Redis and AI quota admission. Its
multi-source branch now stores two owned optional values inline in SmallVec,
removing the separate slot-vector allocation for exactly two configured sources.
Larger lists still spill; source strings and the composed result retain their
ownership. The zero/single-source fast paths remain unchanged. The small inline
buffer increases local stack storage; no throughput improvement is claimed.

Resolution still visits every source in configuration order before composition.
Some(empty) remains present, missing slots keep their position, byte-length
prefixes and output preallocation are unchanged, and all-missing inputs retain
existing default/allow/deny semantics. No early truncation, source caching,
namespace, logging or caller failure-policy change is introduced. The owning
knowledge now includes the AI quota consumer and its own result mapping.

Independent plan and final source reviews passed. Shared-helper tests passed 14,
Local/Redis tests passed 28 and AI quota tests passed 27. The two-empty-source
allocation test initializes mock dispatch before measurement and observes only
one output allocation; ordered three-source spill and all-missing two/three-source
fallback tests also pass. Formatting, workspace check/Clippy and seven repository
guards passed; existing warnings remain outside changed code. No live Redis,
full integration suite or throughput benchmark ran for this batch.

## Borrowed decoded query condition inputs

The KeyGet source audit found that Query conditions copied a selected decoded
value on every evaluation, including response chunks. PluginSession now exposes
a borrow tied to the current session read, and the production adapter obtains it
from the existing canonical frozen entry-query cache. The owned query getter
copies that same borrow for consumers requiring ownership. The condition accessor
uses it synchronously; no predicate result or new request snapshot is cached.

First access remains lazy and can allocate the parsed cache. Rewriting the live
URI does not change frozen decoded values, and raw whole-query access stays live.
Empty values remain present and missing names remain absent. No earlier query
initialization or change to the existing pre-entry-capture boundary is introduced.
Other owned KeyGet sources and multi-source rate-limit value copies remain open.

Independent plan review, the exact supporting-file scope amendment and final
source review passed. The only supporting test-crate edit adds the required
method to ResultSession with its existing unexpected-operation panic contract;
no integration scenario or assertion changes. Shared conditions passed 67 tests,
query-cache tests passed 7, identity adapter tests passed 4 and the source
allocation inventory passed 1. The two borrowed-query tests also passed (one
is included in the condition group). The real adapter reports one owned value
allocation versus zero for each repeated compiled match after cache initialization.
Formatting, workspace check/Clippy and seven repository guards passed; existing
warnings remain outside changed code. No live Redis, full integration suite or
throughput benchmark ran for this batch.


## Borrowed live context condition inputs

The adapter now exposes a short borrow from the existing context map. Ctx
conditions read it synchronously, while the owned getter still copies for callers
that need ownership. JWT claim extraction parses the same borrowed source text;
only the whole-source copy is removed. Parsed JSON and candidate allocations
remain, and the linked JWT design task stays open with updated evidence.

No source or result is frozen across evaluations. Context replacement/removal,
empty-versus-missing values and missing-only JWT-to-OIDC fallback remain intact.
The required ResultSession method is the sole supporting test-crate change and
retains its unexpected-operation panic; no integration scenario is modified.

Independent plan and final source reviews passed. Both real-adapter tests passed;
shared conditions passed 67, RequestRestriction passed 18, RequestAccessPolicy
passed 18 and the source allocation inventory passed 1. The Ctx test compares one
owned-value allocation with zero per compiled match across live mutations; JWT
parsing/candidate allocation remains outside that zero-allocation claim. Formatting,
workspace check/Clippy and seven repository guards passed; existing warnings are
outside changed code. No full integration suite or throughput benchmark ran.


## Borrowed captured identity condition inputs

TraceId and Host conditions now borrow their existing request-owned values for
one synchronous evaluation. TraceId reads the canonical standard W3C identity;
Host reads the frozen normalized entry hostname. Missing initialization/capture
remains absent. The private identity helper now returns a borrow, while owned
KeyGet callers explicitly copy it. No current-header fallback or new cache exists.

The audit also corrected stale condition knowledge: RequestId custom rules and
default-header suppression do not mutate the current standard W3C ID. Live Host
and traceparent header rewrites remain distinct from these captured identities.
Only the required methods are added to the supporting ResultSession test adapter,
with its existing unexpected-operation panic contract.

Independent plan and final source reviews passed. The real-adapter identity
allocation test passed, along with shared conditions (67), identity accessors (4),
trace-ID lifecycle (29), request-ID-filtered tests (15) and the allocation source
inventory (1); filters may overlap. Each selected owned read allocates once while
repeated compiled matches allocate zero times. Formatting, workspace check/Clippy
and seven repository guards passed. New needless-lifetime warnings were removed
from concrete implementations while the trait retains Mockall's required explicit
lifetime. Remaining warnings are outside changed code. No full integration suite
or throughput benchmark ran.


## Composite-key presence without construction

KeyExist(RemoteIpAndPath) now checks the current RemoteIp's nonemptiness without
formatting ip:path. The existing owned accessor returns a value for any nonempty
RemoteIp, including an empty Path; path reads have no side effects. Negation stays
outside the presence decision and compiled conditions continue to read each time.
Actual value matching, secret comparison and keyed probability still use the
unchanged composite string. Their encoding/hash behavior is outside this change.

Independent plan and final source reviews passed. The real-adapter test passed
across live RemoteIp transitions, empty/ordinary/long paths and both negation
settings, with zero allocations per existence evaluation. Shared conditions
passed 67 tests, RequestRestriction passed 18 and RequestAccessPolicy passed 18.
Formatting, workspace check/Clippy and seven repository guards passed; existing
warnings remain outside changed code. No full integration suite or throughput
benchmark ran. Composite content consumers still allocate and remain open for
further review; this does not establish allocation-free conditions in general.


## Pre-sized owned composite keys

The canonical RemoteIpAndPath accessor now reserves the exact output byte length
and appends IP, colon and current normalized Path. Missing IP still returns None
before reading Path; empty paths, IPv6 and UTF-8 retain identical bytes. Owned
consumers, matching and keyed probability keep the same value contract.

The real-adapter value/allocation matrix and existing composite-existence test
passed (2 tests). The fixed source inventory passed (1), measuring a reduction
from two allocations requesting 36 bytes to one requesting 19 bytes for its
composite fixture. Shared conditions passed 67, HTTP common helpers passed 186,
local/Redis rate limiters passed 28 and AI quota passed 27. Formatting, workspace
check/Clippy and seven repository guards passed; warnings are outside changed
code. Independent plan and final source reviews passed. No full integration suite
or throughput benchmark ran. Composite values still require one owned allocation;
this does not establish allocation-free conditions in general.

## Borrowed HMAC quoted parameters

The request plugin and compiled HMAC conditions share the same synchronous
Authorization parser. Its unescaped quoted values now borrow the input until
conversion, while escaped values retain the existing owned decoder. Final
username, signed-header names and signature bytes remain owned. Duplicate keys,
unknown-value validation, arbitrary escapes and error ordering remain unchanged;
no borrow crosses body capture, header mutation or authentication result handling.

Independent plan and final source reviews passed. HMAC tests passed 29, shared
conditions passed 67 and the wider hmac filter passed 50 (filters overlap).
Unescaped decoder cases assert source-pointer identity and zero allocations;
escaped/malformed values and final ownership have semantic regression coverage.
Formatting, workspace check/Clippy and seven repository guards passed; existing
warnings are outside changed code. No full integration suite or throughput
benchmark ran. Parameter keys, parser containers and final outputs still allocate;
this removes only unescaped intermediate value copies, not whole-parser costs.

## Borrowed HMAC signing query

HMAC request-target signing now uses the existing live raw `request_query`
accessor instead of cloning through `get_query`. Both read the current URI;
encoded bytes and URI rewrites remain visible, independently of the frozen
entry-query cache. Both target spellings preserve omission of the question mark
for absent/empty queries. The final signing string still owns its output.

Independent plan and final source reviews passed. The real-adapter regression
passed (1), HMAC-filtered tests passed 51 and shared conditions passed 67 (filters
may overlap). Borrowed query reads allocate zero times; nonempty owned reads
allocate once. Existing fixed signature vectors and new query cases retain byte
coverage. Formatting, workspace check/Clippy and seven repository guards passed;
existing warnings remain outside changed code. No full integration suite or
throughput benchmark ran. This removes the intermediate query copy per target
component, not signing-string allocation or all HMAC runtime costs.


## HMAC signed-header membership audit

Source inspection confirms separate parser duplicate-detection and verifier
required-header membership tables. Replacing them entails list-size complexity
or ordered-owner memory tradeoffs, recorded in the deferred task above. Existing
cold preparation of enforced names, signature order and failure ordering remain
required. No runtime behavior or input limits changed.

The owning prebuilt-verifier knowledge was corrected: the current schema helper
returns false for HMAC; runtime header/body predicates use compiled verifiers,
and the removed verify_headers_config is not a fallback. Independent plan and
final record reviews passed; agent-doc validation and diff checks passed. This
documentation-only batch ran no Rust tests or performance benchmark and establishes
no whole-authentication or allocation-free body-condition coverage.

## HMAC algorithm selection without lowercase allocation

The Gateway Authorization parser now reaches a canonical resources helper that
compares its three fixed algorithm names without building a lowercase String.
ASCII case variants, unknown/non-ASCII rejection and non-trimming behavior are
unchanged. The supporting resources scope is limited to hmac_match.rs and its
new mounted unit test; schema, serde and credential transport are untouched.

Independent plan and final source reviews passed. Resources HMAC-filtered tests
passed 28, Gateway HMAC-filtered tests passed 52 and shared conditions passed 67
(filters may overlap). Tests cover all 384 ASCII case variants and malformed
boundaries; selected valid/invalid names allocate zero times in the Gateway
measurement. Formatting, workspace check/Clippy and seven repository guards
passed; existing warnings are outside changed code. No full integration suite or
throughput benchmark ran. This removes algorithm-name normalization storage,
not other Authorization parser allocations or authentication costs.

## Stack storage for HMAC body-digest encoding

After the canonical body SHA-256 read, HMAC encodes the fixed 32-byte digest into
44 stack bytes instead of allocating an encoded String. Exact padded STANDARD
Base64 text comparison is retained; client text is not decoded into a more
permissive equivalence. Outer Digest-header trimming and owned expected text
across asynchronous capture/hash remain unchanged, as do authentication order,
body storage and read/mismatch failures.

Independent plan and final source reviews passed. HMAC-filtered tests passed 53
and shared conditions passed 67 (filters may overlap), including existing signed
body match/tamper and read-failure cases. The comparison matrix uses independently
encoded references and measures zero allocations for selected match/mismatch
inputs; padding, pad bits and whitespace boundaries remain covered. Formatting,
workspace check/Clippy and seven repository guards passed; existing warnings are
outside changed code. No full integration suite or throughput benchmark ran.
This removes encoded-comparison storage only, not the expected-header owner or
all body-verification allocations.


## HMAC key initialization audit

The pinned HMAC implementation repeats key reduction and inner/outer pad-state
initialization during signature verification, despite configuration/verifier
construction already being cold. Immutable prepared-state cloning is a candidate,
but retained sensitive state scales with credentials, allowed algorithms and
verifier generations. The deferred task records CPU/memory/ownership alternatives
and acceptance without changing runtime behavior or claiming a measured benefit.

Independent plan and final record reviews passed. Agent-doc validation and diff
checks passed; this documentation-only batch ran no Rust tests or benchmark.
Current-time timestamp checks and owned expected body-digest text across await
remain intentional request work; this audit does not establish full HMAC or
whole-callback optimization coverage.

## Borrowed CORS decision inputs

EdgionPlugins CORS now borrows Origin for validation and Timing-Allow-Origin
selection, and borrows the private-network flag for its exact true comparison.
It copies only accepted selected ACAO output and timing echoes that must survive
session mutation. Missing/non-text values, first-header selection, star/echo
semantics, matcher construction, header order, preflight/reflection and Max-Age
behavior are unchanged. Native Gateway API CORS is outside this implementation.

Independent plan and final source reviews passed. Gateway CORS-filtered tests
passed 41 and resources CORS tests passed 34. Warmed allocation cases measure zero
for denied origins and disabled/denied/star timing decisions, and one for nonempty
accepted output or timing echo. Existing eight-header order/no-spill coverage
remains intact. A new test type-complexity warning was corrected without lint
suppression; the focused plugin rerun passed 15 tests and formatting/workspace
check/Clippy passed again. Seven repository guards and final document/diff checks
passed. Remaining warnings are outside changed code. No full integration suite or
throughput benchmark ran; header insertion and other CORS allocations remain.

## Lazy CORS requested-header reflection

The canonical CorsConfig getter now invokes an owned-value producer only for
allow_headers **. Gateway preflight preparation supplies the existing header
accessor lazily, avoiding discarded Access-Control-Request-Headers copies for
fixed lists and *. Reflections still move an owned String, preserving empty
versus missing fallback and lifetime before session mutation. Actual responses
continue to skip the getter. The supporting resources scope is limited to the
existing CorsConfig helper and its unit tests; no schema/cache fields changed.

Independent plan and final source reviews passed. Resources CORS tests passed 35
and Gateway CORS-filtered tests passed 42. Tests prove lazy/exactly-once invocation,
moved-pointer identity and preflight/actual-response output parity. Selected
header-pair preparation allocates zero except one nonempty reflection copy, with
Max-Age disabled during measurement. Formatting, workspace check/Clippy and seven
repository guards passed; existing warnings remain outside changed code. No full
integration suite or throughput benchmark ran. Final header insertion and other
CORS work retain their existing ownership and costs.

## Native CORS prepared response values

Native Gateway API CORS consumes validated configuration into gateway-owned
response values: fixed method/header/exposure lists and Max-Age are prepared
once, with origin wildcard presence and reflection policy resolved at creation.
Header-pair preparation borrows fixed text and the caller-owned origin; only
requested-header reflections retain owned values before session mutation.
Raw configuration is not duplicated. Canonical validation, origin matching,
live Cookie/Authorization presence, header order, omission/empty behavior and
native actual-response versus preflight output remain unchanged.

Independent plan review passed. Final source review corrected a test fixture
that incorrectly expected mixed wildcard lists to pass canonical validation;
the test now asserts rejection. Runtime validation was unchanged. Gateway CORS
tests passed 44, including warmed zero-allocation fixed six-header preparation
and two allocations for two nonempty reflections. Formatting, workspace check,
workspace Clippy and seven repository guards passed. Remaining Clippy warnings
are outside changed files. Independent final source and record reviews passed. Coverage remains limited to native CORS header preparation,
excluding origin acquisition and final header insertion; no full integration
suite or throughput benchmark ran.

## Borrowed native CORS request Origin

Native CORS request processing now borrows Origin through the existing header
accessor. Its preflight classification and local response preparation helpers
take shared session references; the local ResponseHeader owns inserted values
before asynchronous writes. The upstream-response callback retains owned Origin
across response-header mutation. Reflection values still own their text.
First-value selection and text filtering, matching, logs, dispatch, header
ordering and write-error behavior are unchanged.

Independent plan review passed. Callback coverage includes missing, empty,
non-text and duplicate Origin values, denied/allowed matches, GET/OPTIONS and
requested-method presence; it forbids owned header acquisition and checks local
204, ACAO/Vary, log outcomes and header/body write order. Final Gateway CORS
tests passed 45. Formatting, workspace check/Clippy and seven repository guards
passed; Clippy warnings remain outside changed files. Independent final source and record reviews passed. This removes the request Origin copy
without claiming zero allocations for the async callback or a measured
throughput gain.

## Borrowed native transform queries

RequestRedirect and URLRewrite now borrow the current raw URI query through
request_query instead of copying it through get_query. Both real adapter
accessors read the same live URI; neither uses the frozen entry cache. Owned
Location/upstream URI output is still completed before session mutation or
await. Missing/empty query omission and all nonempty raw bytes remain unchanged.
This removes one temporary nonempty query copy per transform; final-string
construction and response ownership are outside this optimization.

Independent plan review passed. New callback cases forbid the owned getter
and cover missing/empty query, percent escapes, plus signs, duplicate parameters
and separators. The existing real-adapter live-URI/pointer/allocation test is
included in this batch's verification and passed. Native Gateway API filter
tests passed 60. Formatting, workspace check/Clippy and seven repository guards
passed; existing warnings remain outside changed files. Independent final source
and record reviews passed. No full integration
suite or throughput benchmark ran.

## Native redirect Location assembly

RequestRedirect now borrows original/full-replacement paths and writes scheme,
host, optional decimal port, path and nonempty query into one preallocated
Location String. Prefix replacement retains its canonical helper and owned
result. The port allowance reserves six bytes for any u16 rather than claiming
exact capacity. Host normalization, scheme/port selection, query bytes, path
semantics and missing-prefix failure remain unchanged.

Independent plan review passed. Pure-builder tests cover ordinary/IPv6/missing
hosts, default and nondefault listener ports including 0 and 65535, path modes
and absent/empty/raw queries. They measure one allocation for original/full paths,
two with prefix replacement, and zero when prefix context is missing. These
counts exclude callback/response-header/logging work. All 62 native Gateway API
filter tests passed, along with formatting, workspace check/Clippy and seven
repository guards. Existing Clippy warnings remain outside changed files.
Independent final source and record reviews passed.
No full integration suite or throughput benchmark ran.

## Borrowed prefix-replacement parts

The canonical native path helper now returns three borrowed output slices.
Element-count matching, empty/unmatched suffix fallback and conditional slash
trimming are unchanged. RequestRedirect and URLRewrite consume these slices
directly, reserving the final Location or path-plus-query once. No intermediate
prefix-path String remains and there is no duplicate prefix implementation.
This supersedes the preceding redirect batch's two-allocation prefix case.

Independent plan review passed. Helper goldens cover parameterized and UTF-8
paths, repeated slashes and empty/slash-only/unmatched suffixes, measuring zero
allocations before test-only joining. Redirect's existing output matrix now
expects one allocation in every path mode. Rewrite's full-path query matrix
measures one allocation with absent, empty or nonempty query. Existing callback
and missing-context failure checks remain. All 63 native Gateway API filter
tests passed. Formatting, workspace check/Clippy and seven repository guards
passed; existing warnings are outside changed files. Independent source and final record
reviews passed. These builder claims exclude response
construction/logging and do not establish throughput gains or whole-callback
allocation counts. No full integration suite ran.

## Remaining native filter direct-path audit

Inspected the native request/response header modifier direct loops and their
shared HeaderModifierPlan: validation, deduplication and HeaderValue preparation
remain cold work; request clones retain shared values for session ownership.
ExtensionRef direct request, async response, sync response and body dispatch
retain selected runtime handles across mutable session calls. Body dispatch
resolves silently without per-chunk PluginLog creation; the empty async stage
keeps its early gate. Name copies feed retained log entries, so this inspection
does not establish that their ownership can be removed locally.

DebugAccessLog uses the borrowed AccessLogEntry projection and owned JSON before
response mutation, with early Wrap suppression. Its conditional frozen URI
assembly remains an intermediate String. The new deferred task records a
fragment-serialization alternative and canonical masking/measurement boundaries.
The owning raw-URI knowledge page preserves the cache objection while removing
unsupported universal performance claims; adjacent serialization cost claims
are explicitly left for follow-up evidence review.

Independent documentation plan and final record reviews passed. Agent-doc
validation and diff checks passed. This audit covers direct filter orchestration and the
identified projection chain, not complete nested plugin runtime, session
mutation, serialization or logging coverage. No runtime changes, Rust tests or
throughput measurements were made in this documentation batch.

## ResponseRewrite fixed header preparation

ResponseRewrite prepares fixed add/set HeaderValues at construction and clones
their shared values into typed session writers. The existing native
PreparedHeader owner moved to the common plugin runtime so both consumers use
one cold parsing/byte-promotion implementation. Native first-equivalent
deduplication remains separate from ResponseRewrite's duplicate-preserving
execution. No duplicate raw fixed-value configuration is retained.

Schema validation still precedes preparation; direct invalid construction
retains its logged no-op and publication still rejects invalid configuration.
Rename remains dynamic, with the original protected checks, first-value read,
set/remove behavior and failure policy. Status then rename/add/set/remove
ordering, static error logs and continued processing are unchanged. Typed
writers retain response protection and queued/direct ownership.

Independent plan review passed. Existing generation-independence, action order,
duplicate-add, writer failure and invalid-publication tests remain, with fixed
writes using typed mock expectations. New tests check first-clone zero allocation,
shared byte pointers and repeated add/set execution. Existing native prepared
value tests exercise the shared implementation. Initial test compilation caught
a stale manually constructed private fixture; it now uses the prepared type
while retaining the intended validation bypass for runtime rename protection.
Fresh ResponseRewrite tests passed 10, native Gateway API filter tests passed 63,
and the real-adapter typed-value/pending-replay test passed. Formatting,
workspace check/Clippy and seven repository guards passed, with existing warnings
outside changed files. Independent source and final record reviews passed. Allocation claims concern value clones only, excluding names, response
maps, queues and logs; no full integration suite or throughput benchmark ran.

## Dynamic ResponseRewrite rename audit

The remaining rename path copies the first text-eligible response value into
String and reparses it for destination insertion. It skips a non-text first
value rather than searching duplicates. Source inspection establishes that a
sensitive textual HeaderValue loses its flag through the fresh string parse;
no production secret exposure was reproduced. Typed cloning would reduce
conversion work and preserve required metadata, while removing text eligibility
would be a separate behavior expansion.

The deferred task records the bounded typed/text-filtered option and explicit
opaque/multivalue/atomic alternatives without approving runtime changes.
Known sensitive metadata must be preserved in any implementation; the current
loss is not accepted as an optional compatibility policy. Dynamic clone costs
need measurement and do not inherit fixed-value preparation's zero-allocation
result. Independent documentation plan and final reviews passed; agent-doc
validation and diff checks passed. No Rust test or benchmark ran in this
source-record batch.

## Wasm callback property capacity

`WasmPlugin::properties` reserves six entries for its five common properties and
optional response status. This avoids intermediate table growth during snapshot
construction without changing getter order, owned values, guest mutation budgets
or callback-frame release. No cross-request capacity is retained and no throughput
improvement is claimed. Independent plan review passed.

The initial parallel plugin run passed 18 and failed 25, reporting preparation
capacity exhaustion, load-path mock failures and a tick-callout timeout. The same
43 tests passed with `--test-threads=1`; 42 host ABI tests also passed serially.
Formatting, workspace check/Clippy and seven repository guards passed. Existing
Clippy warnings remain outside the changed file. No full suite or benchmark ran.
Independent final review passed.

## Wasm header snapshot ownership audit

Guest header mutations retain separate readable snapshots and ordered replay
operations, with duplicated String payloads covered by explicit allocation
profiles. Immutable sharing is a candidate, but owner overhead, detached replay
lifetime and admission rejection boundaries require a reviewed representation.
The deferred task preserves both logical views and records those decisions.
Initial text-only session snapshots and body storage are outside its scope.
Independent documentation plan review passed; this batch changes no runtime code
and makes no benchmark or complete host-memory-bound claim.
Independent final review, agent-doc validation and diff checks passed.

## Borrowed DSL response-header reads

`resp.header`, `resp.content_type` and `resp.has_header` read the live response
map without the intermediate owned session-getter String. The first-value
`HeaderValue::to_str` gate, absent/unbound behavior and name conversion remain.
Value-producing reads check exact byte length before creating an independent
DSL Arc string; presence checks remain unrestricted by the output string limit.
The existing borrowed helper is reused, with no session API or policy change.

Eight mock sites now supply real HeaderMaps. Exact/over-limit header fixtures
use legal four-byte ASCII instead of non-ASCII text that the real adapter would
reject; existing borrowed-helper multibyte/zero-allocation rejection coverage
remains. New cases cover absent/empty/sensitive values, duplicate order,
opaque-first eligibility, mixed-case names and unrestricted presence length.
Independent plan review passed; 492 DSL tests passed serially. No full suite or
throughput benchmark ran; argument conversions and final DSL ownership remain.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed DSL response-header names

`resp.header_names()` traverses live HeaderMap keys directly instead of copying
every name into a temporary Vec before truncation. The first 256 unique names
retain map order and per-name byte checks, independently of `max_list_len`.
Opaque values remain named; duplicate values do not duplicate names. The output
Vec and independent Arc strings remain owned by the VM.

Four mocks now provide real HeaderMaps. Ordered writer expectations check the
map's name order with duplicate and opaque values; exact/over-limit and empty
cases remain. The prefix fixture confirms the oversized name is actually 257th,
so it is ignored beyond the cap. Independent plan review passed and 492 DSL
tests passed serially. No full suite or throughput benchmark ran.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed DSL request-header scalars

Host, content type, scheme and content length use the existing borrowed header
accessor. String-producing branches validate the selected value before final
Arc allocation. Scheme retains raw case, empty-value precedence and ordered
fallback; length failure does not trigger fallback. Content length preserves
signed i64 parsing, nil for malformed/overflow input, and first-value text
eligibility without adding a string limit or HTTP validation.

Seven existing mocks now use HeaderMaps and reject the owned getter. New cases
exercise scheme fallback/duplicate/opaque/empty/case selection, exact/over-limit
strings and signed/maximum/non-text content-length values with duplicates.
Independent plan review passed; 495 DSL tests passed serially. No full suite or
throughput benchmark ran; final DSL string ownership remains unchanged.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed DSL request metadata

Method, path, entry request path, direct-peer IP, resolved remote IP, certificate
CN and fingerprint reuse the borrowed string-result helper. This removes five
temporary to_string calls and two certificate-field clones, preserving the same
getters, optional nil states, exact byte-limit errors and independent Arc output.
Live/frozen request data and direct/resolved client identity remain distinct.

The existing metadata boundary test now covers the five additional getters with
mixed-case method, distinct raw path spellings and IPv4/IPv6 text. Certificate
multibyte and absent-value coverage remains. Independent plan review passed and
495 DSL tests passed serially. No full suite or throughput benchmark ran.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed DSL dynamic headers and raw query

Dynamic `req.header(name)` and `req.query_string` use the existing borrowed
interfaces and result helper. Dynamic name conversion, first-value text
eligibility, nil/empty distinction, current raw query bytes and exact string
limits remain unchanged; final DSL Arc values still own their contents.

Header mocks were migrated across six test modules, including once-only ordered
compiler inputs and the large-value bounded-log fixture. The initial panic-test
migration raised inside a mock predicate, poisoning mockall's internal mutex
and causing a second panic at destruction: 496 tests passed and one failed.
The independently reviewed correction injects through the unchanged query getter
instead, retaining exact invocation, fail-open outcome and fixed-log assertions.
No runtime adjustment was made for the mock limitation. Fresh serial DSL tests
passed 497, including dynamic-header first/opaque/invalid-name/limit cases and
raw-query absent/empty/encoded/duplicate/limit cases. Formatting, workspace
check/Clippy and seven guards passed, with existing warnings outside the diff.
No full suite or throughput benchmark ran.
Independent final review passed.

## Borrowed DSL decoded query and cookies

`req.query(name)` borrows the frozen decoded cache and `req.cookie(name)` uses
the existing canonical borrowed Cookie parser. Neither changes argument
conversion, nil/empty handling, byte-limit errors or independent Arc results.
Cookie multi-field order, key case, embedded equals and malformed-before-match
failure remain. Initial query-cache parsing and final VM allocation remain.

Two getter fixtures now use borrowed data. Tests cover query nil/empty/multibyte
byte boundaries and Cookie empty/duplicate/case/multiple-field/non-text ordering
plus exact/over-limit values. Generic VM panic injection now uses the unchanged
owned URI getter, retaining once-only, fail-open and log assertions without
depending on a getter removed from the hot path. Independent plan review passed;
499 DSL tests and the real-adapter frozen decoded-query test passed. No full
suite or throughput benchmark ran.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed DSL path parameters

The session's new borrowed path-parameter getter owns the unchanged cache
initialization/lookup logic; the owned getter delegates and copies its result.
DSL borrows before byte validation and final Arc creation. Production captures
remain frozen before plugins; synthetic fallback initializes empty results once.
The trait documentation now distinguishes those paths accurately.

The cache-backed DSL limit matrix covers path parameters, and a real-adapter
test checks capture survival after URI rewrite, nil/empty parity, init-once empty
fallback and zero allocations for a warmed borrowed lookup. Initial extraction
and final VM ownership are outside that allocation claim. Independent plan review
passed. Workspace checking identified one additional hand-written test adapter
in `edgion-tests/tests/dsl_redis_rate_limit.rs`; an independently approved exact
scope extension adds only the required panic stub, preserving its fail-on-use
policy and leaving the ignored isolated-Redis scenario untouched.

Fresh DSL tests passed 499, the new adapter test passed and ProxyRewrite tests
passed 17. An initial context test filter selected zero tests; it was corrected
to select the actual cached-query module. No live Redis test, full suite or
throughput benchmark ran.
All seven selected context-cache tests passed after that correction.
Formatting, workspace check/Clippy and seven repository guards passed after the
support stub was added, with existing warnings outside changed files.
Independent final review passed.

## Borrowed DSL protocol and gRPC metadata

The three session metadata getters now borrow their existing storage or protocol
fallback literals. DSL validates byte lengths before creating independent Arc
strings. Explicit protocol values, including empty strings, retain priority;
missing protocol metadata alone uses the existing gRPC flag fallback. gRPC
metadata reads add no protocol gate. The unused owned optional-result helper
was removed after its last callers migrated.

Independent plan review passed. Boundary tests cover nil/empty and UTF-8 byte
limits; real-adapter tests cover protocol precedence, existing metadata identity
and allocation-free getters only. The first DSL selection passed 402 tests and
failed one new fixture because its deny call omitted the required body argument;
the fixture was corrected and the rerun selects the full DSL module. No runtime
change was needed for that failure. No full suite or throughput benchmark ran.

The corrected full DSL selection passed 500 tests; the real-adapter test passed.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## DSL string ownership evidence boundary

Inspected remaining URI result construction and argument conversions. Read-only
argument consumers, Body-owned Bytes transfers and composite URI results have
different ownership constraints; their decision boundaries are recorded in the
linked task. The SSO migration page now labels historical workload estimates and
library observations, removes unsupported current dominance/net-regression
conclusions, and no longer treats whole-representation deferral as a prohibition
on local borrowing. Historical proposal/spec paths are absent from this tree.
No runtime change or measurement was made. Independent plan review passed.
Independent final review, agent-doc validation and diff checks passed. Rust
checks were not rerun for this documentation-only batch.

## Borrowed CSRF Cookie verification input

CSRF verification now borrows the canonical Cookie parser's selected value
instead of copying through get_cookie. Current headers, safe methods, token
comparison/decoding, signature/expiry and fixed denial/log outcomes remain.
The owned getter already delegates to this parser; no alternate parser or cache
was added. Decoding and cryptographic allocations remain.

Existing fixtures now use actual Cookie fields. A new matrix covers field order,
opaque-before/after-match behavior, case-sensitive keys, empty and duplicate
matches, embedded equals and valid padded tokens while forbidding the owned
getter. Independent plan review passed. No full suite, live integration or
throughput measurement was requested or run for this batch.
Fresh CSRF tests passed 17 and canonical Cookie parser tests passed four.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files.
Independent final review passed.

## Fragmented CSRF signature input

CSRF HMAC input is now fed as ordered literal/decimal/nonce fragments, removing
the complete temporary signing String. Standard i64 formatting, HMAC setup,
hex output, signature verification, lifetime and token wire shape remain.
The timestamp text and final signature still allocate; extra update calls are
not claimed to improve CPU or throughput.

Independent plan review passed. Tests compare the previous composite-input
implementation across integer/key/nonce boundaries and retain the known signature.
A fixed-fixture allocation comparison includes complete signature generation
on each side, with fixture construction excluded. No full suite, live integration
or throughput benchmark was run.
Fresh CSRF tests passed 19. For nonce lengths 64 and 4096, the original complete
operation requested 186 and 4218 allocation bytes (three calls each); the new
operation requested 83 bytes (two calls) for either fixture. These are test-build
fixture results, not retained-memory or production-throughput measurements.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Borrowed JWT credential extraction

JWT extraction now borrows the selected Header, frozen decoded Query or canonical
Cookie value through synchronous verification. Verification returns owned claims
and issuer plus a store-bound group reference, so no session token borrow is
needed during later mutation or failure awaits. Source priority, query opt-in,
Bearer trimming, empty/opaque handling, anonymous policy and credential hiding
remain. Parsing, verification and claims ownership still allocate.

Independent plan review passed. Existing tests use borrowed query fixtures and
real Cookie headers while forbidding owned getters. New extraction cases cover
priority, duplicate/opaque fields, exact Cookie keys, empty/whitespace rules and
query suppression. Pointer checks cover borrowed selected-value ownership only. An initial mocked
allocation assertion observed one allocation and failed, while value/pointer
assertions passed (46 tests passed, one failed). Its cause was not established;
an independently reviewed amendment removed the unsupported zero-allocation
assertion while retaining pointer, value and owned-getter prohibition checks. No full suite, live
integration or throughput benchmark ran.
Fresh rerun passed all 47 JWT tests. Formatting, workspace check/Clippy and
seven repository guards passed; existing warnings are outside changed files.
Independent final review approved the corrected tests and limited claims.

## ExtProc trailer status cardinality

The response-trailer validator now inspects at most two grpc-status values
instead of collecting them into a temporary Vec. Whole-map size and forbidden
field checks still precede status handling; absent non-gRPC status, duplicate
rejection and the existing u16 text/range rules remain. Protobuf ownership,
atomic candidate replacement, failure policy and terminal handling are unchanged.

Independent plan review passed. Pure tests cover both protocol modes, missing,
valid, duplicate, opaque and numeric boundaries plus field-cap/forbidden gates;
allocation instrumentation excludes fixture construction. Existing asynchronous
ExtProc tests remain. No full suite, live wire integration or throughput benchmark
ran for this batch.
Fresh serial ExtProc selection passed 140 tests. The pure validator matrix
measured zero allocations for its prepared maps; callback allocations remain.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## Shared borrowed KeyAuth inputs

The condition evaluator's borrowed KeyGet mapping moved unchanged into the
session adapter helper `key_value`; conditions and KeyAuth now share it.
KeyAuth retains its four-source whitelist, first-nonempty policy, invalid-key
rejection and config-owned source reference. The session value borrow ends after
hash lookup, before metadata mutation, credential hiding or failure awaits.
Other owned KeyGet consumers and RegionRoute selection are unchanged.

Tests use actual header/Cookie maps and borrowed query/context values, forbid
owned key_get and retain source identity, async metadata/hiding, denial and
sensitive-log assertions. The condition accessor tests follow the relocated
function. Knowledge now separates source-read reuse from selection-policy
consolidation and corrects historical unsupported allocation/closure-cost claims.
Independent plan review passed. No full suite, live integration or throughput
benchmark ran for this batch.
Fresh selections passed: 14 KeyAuth tests, 67 condition tests, two borrowed-query
matches and one borrowed-context adapter test. Selections may overlap and are not
a unique-test coverage count. Formatting, workspace check/Clippy and seven
repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## Borrowed RegionRoute source values

RegionRoute now shares canonical `key_value` reads while retaining its separate
first-nonempty source policy. Borrowed strings support direct and configured map
selection, bypass comparison, hash parsing/calculation and sensitive logging;
RemoteIpAndPath keeps owned composition. Overlay, failover and route ordering
remain unchanged.

Independent plan review passed. Tests cover source priority/identity and ownership,
direct/map/bypass routing, and hash passthrough/writeback. The first compile found
a test-only import at the wrong re-export level; the import was corrected.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh serial selections passed all 23 RegionRoute and 67 condition tests.
Formatting, workspace check/Clippy and seven repository guards passed; Clippy
retains existing warnings outside the changed files. Independent final review
passed.

## Borrowed DirectEndpoint input and captures

DirectEndpoint now uses canonical borrowed source reads and resolves an input
slice. Regex captures no longer become temporary Strings before IP parsing.
The returned numeric address and retained TLS policy remain owned for the jump;
source fallback, policy errors, authorization and address safety are unchanged.
Independent plan review passed. Existing tests now use real header maps and
forbid owned reads; new tests cover empty/opaque input and successful extraction.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh serial selections passed 18 DirectEndpoint-related tests (including backend
authorization and retry peer cases) and 67 condition tests. Formatting, workspace
check/Clippy and seven repository guards passed; existing Clippy warnings remain
outside changed files. Independent final review passed.

## Borrowed DynamicExternalUpstream routing input

DynamicExternalUpstream uses the canonical session `key_value` reader for its
routing input. Regex captures and domain-map lookup consume borrowed strings;
the selected target belongs to plugin configuration independently of that input.
The jump still owns domain/SNI, and Host/outcome mutations retain their ordering
and locked-jump behavior. Composite sources retain the helper's owned fallback.

Independent plan review passed. Real header fixtures forbid owned reads and
preserve successful plain/regex selection and jump/Host/outcome sequence tests.
A failure matrix covers absent, empty, opaque, regex-missing and unmatched inputs
under both reject and fallback policies. No full suite, live DNS integration or
throughput benchmark ran; this is not callback-wide zero allocation.
Fresh serial selections passed all 8 DynamicExternalUpstream-related tests and
67 condition tests. Formatting, workspace check/Clippy and seven repository
guards passed, with existing warnings outside changed files. Independent final
review passed.

## Borrowed multi-source rate-limit components

Multi-source bucket composition now retains canonical `Cow<str>` reads in the
existing two-slot SmallVec, avoiding owned copies of ordinary source values.
Composite inputs retain owned fallback, and the final bucket String remains
owned. Zero/single-source paths, exact slot encoding and capacity, missing-value
policy, quota namespace and logging remain unchanged. Redis multi-quota tests
now exercise actual headers for the multi-source path.

Independent plan review passed. Targeted tests cover encoding, spill, empty and
missing values, UTF-8 byte lengths, owned composites and warmed allocation probes.
No full suite, live Redis integration or throughput benchmark ran for this batch.
The initial helper test run passed 15 cases and failed the new composite fixture's
incorrect expected byte length (11 instead of 12); the corrected source required
a fresh rebuild because the first compile had already consumed the older test.
Both warmed two-source allocation probes passed in that run.
Fresh rerun passed all 16 helper tests. Rate-limit selection passed 127 with one
existing ignored case; AI quota passed 27 and conditions passed 67. Selections
overlap and are not a unique-test count. Warmed empty/nonempty two-header helper
fixtures each measured one output allocation. Formatting, workspace check/Clippy
and seven repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## Borrowed CtxSet processing inputs

CtxSet starts with borrowed static/source values and borrows mapping outputs;
template rendering, regex extraction and transformations retain their existing
owned outputs. The final rule result remains an owned String before context
mutation. Priority, early defaults, whitespace/empty handling, one-operation
validation and sequential context updates remain unchanged.
Independent plan review passed. Tests cover combined extract/transform/map paths,
mapping/rule defaults, plain values, warmed mapping allocation and ordered
cross-rule context reads. No full suite, live integration or throughput benchmark
ran for this batch.
Fresh serial selections passed 15 CtxSet-related tests, 22 template tests and
67 condition tests. The warmed source-to-mapping rule fixture measured one final
String allocation. Formatting, workspace check/Clippy and seven repository guards
passed, with existing warnings outside changed files. Independent final review
passed.

## REST-to-GraphQL scalar bounds traversal

Tracing Gateway construction into its canonical shared binding validator found
an unnecessary array-check worklist for scalar roots. After unchanged universal
profiling and adapter cap checks, scalar values now skip that second traversal.
Containers retain nested array checks and existing error precedence. The bounded
supporting Resources edit owns this runtime validation; no binding API, schema,
body publication or separate Resources coverage claim is introduced.

Independent plan review approved the exact four-file scope. Direct tests cover
scalar text-byte limits, nested array length and profile depth/node boundaries.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh Resources tests passed 7 cases; Gateway adapter tests passed 9. Formatting,
workspace check/Clippy and seven repository guards passed, with existing warnings
outside changed files. Independent final review passed. Temporary source-Value
ownership in binding remains a separate open inspection boundary.

## REST-to-GraphQL owned binding input transfer

The Gateway transfers its existing owned KeyGet String into the canonical shared
binder through `Cow<Value>`; native body pointers remain borrowed. Profiling still
precedes clone/conversion, and plain text now moves into the final variable rather
than being copied again. JSON conversion, final bounds/type checks, header
preflight and envelope/publication ownership are unchanged. The supporting
Resources API and all actual call sites changed within the reviewed five-file scope.

Independent plan review passed. Tests compare owned/borrowed results and errors,
byte-cap behavior, owned String allocation identity and native body independence.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh Resources and Gateway selections each passed 9 tests. Formatting, workspace
check/Clippy and seven repository guards passed, with existing warnings outside
changed files. Independent final review passed. Owned plaintext pointer identity
confirms transfer of that allocation; parsing and serialization costs remain.

## Response Connection nomination inline storage

The response sanitizer's owned nomination list now uses two inline SmallVec
slots with unchanged heap fallback. All field validation still completes before
mutation; token parsing, duplicate handling, invalid-ASCII skipping, opaque
rejection, 101 preservation and Transfer-Encoding tracking remain unchanged.
Independent plan review passed. Tests measure a prepared two-standard-name case
and exercise three/eight-name spills with successful removal or atomic opaque
failure. This adds inline stack storage and does not remove custom-name allocations.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh response-filter tests passed 25 cases and shared parser tests passed 7.
The prepared two-standard-name sanitizer fixture measured zero allocations;
custom-name and larger-list behavior carries no such promise. Formatting,
workspace check/Clippy and seven repository guards passed, with existing warnings
outside changed files. Independent final review passed.

## Session persistence Max-Age formatting

The upstream-response cookie builder writes the permanent-cookie Max-Age
integer directly into the final String instead of formatting a temporary String
and copying it. Conditions, decimal bytes, attribute order and Secure handling
remain unchanged. Final-buffer growth remains possible; this is not a throughput
claim. Independent plan review passed. Exact-output tests cover zero, maximum
u64 and absent TTL, alongside existing Session and default cookie coverage.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh session-persistence tests passed 22 cases. Formatting, workspace
check/Clippy and seven repository guards passed, with existing warnings outside
changed files. Independent final review passed.

## ProxyRewrite owned substitution results

Argument and numeric-capture regex replacements now transfer owned results;
final dollar restoration reuses the owned result when no NUL is present.
Substitution order, encoding, missing-value behavior and sentinel semantics are
unchanged. Independent plan review passed. Direct restoration coverage asserts
allocation identity and exact sentinel behavior; existing resolver and sink
tests cover both URI/header contexts. Other substitution allocations remain.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh ProxyRewrite tests passed 18 cases. Formatting, workspace check/Clippy and
seven repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## ProxyRewrite final URI query assembly

Both URI rewrite branches move the resolved path into final query assembly.
Absent/empty queries reuse it unchanged; nonempty queries append literal bytes
with the existing separator rule. Available capacity is reused and growth
remains possible. Independent plan review passed. Direct tests compare exact
output with ordinary and preallocated strings, including encoded/duplicate
fields, existing queries, trailing question marks and empty paths.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh ProxyRewrite tests passed 19 cases. Formatting, workspace check/Clippy and
seven repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## ProxyRewrite borrowed variable inputs

Argument and path substitutions borrow the existing canonical frozen query and
path-capture getters until encoding completes, avoiding owned input copies.
Missing/empty behavior, encoding, stage order and final output ownership remain
unchanged. Independent plan review passed. Existing fixtures now supply borrowed
values; a combined both-context test forbids owned getters and covers repeated,
encoded, missing and empty inputs. Cache initialization and output allocations
remain. No full suite, live integration or throughput benchmark ran for this batch.
Fresh ProxyRewrite tests passed 20 cases; the real-adapter frozen-query and
path-capture cache tests each passed. Formatting, workspace check/Clippy and
seven repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## ProxyRewrite dollar-escape ownership

The existing escape helper preserves borrowed raw paths/captures or owned
encoded values when no dollar occurs. Actual dollar escaping retains the same
sentinel replacement. No stage is skipped and known substitution limitations
remain unchanged. Independent plan review passed. Direct tests cover ownership
identity, repeated dollars, existing NUL, UTF-8 and exact restoration.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh ProxyRewrite tests passed 21 cases. Formatting, workspace check/Clippy and
seven repository guards passed, with existing warnings outside changed files.
Independent final review passed.

## Consistent-hash lazy large-fleet visited tracker

Above 128 backend slots, SeenSet keeps the first index inline and allocates its
existing dense vector only on a second distinct candidate. The selection loop,
health-check order, distinct-backend budget and borrowed winner are unchanged.
Independent plan review passed. Helper tests cover boundary counts, high indices
and promotion membership. Large-ring tests isolate tracker allocation and verify
all-unhealthy traversal checks every distinct backend once. No full suite,
live integration or throughput benchmark ran for this batch.
Fresh selection tests passed 40 cases and compiled-selector tests passed 8.
The isolated large-ring fixtures measured zero tracker allocations through the
first candidate and one for the full rejection walk. Formatting, workspace
check/Clippy and seven repository guards passed, with existing warnings outside
changed files. Independent final review passed.

## ForwardAuth unsigned log arguments

The canonical PluginLog formatter accepts non-sensitive u16/u64 diagnostics and
renders decimal digits through a fixed stack buffer before the existing bounded
append. ForwardAuth passes status codes directly, removing temporary numeric
Strings without changing entry text, order, credential hiding or deny behavior.
No arbitrary Display formatter or extra log entry is introduced. Independent
plan review passed. Tests compare numeric/plain forms, integer extremes, scratch
allocations, all sensitive modes and rendered size/drop boundaries.
No full suite, live integration or throughput benchmark ran for this batch.
Fresh PluginLog tests passed 31 cases and ForwardAuth tests passed 12. Numeric
scratch fixtures measured zero allocations. Formatting, workspace check/Clippy
and seven repository guards passed, with existing warnings outside changed
files. Independent final review passed.

## Rate-limit typed integer log arguments

The canonical formatter adds signed integers and pointer-sized integer inputs.
The same bounded stack decimal helper serves both signed and unsigned variants;
there is no public arbitrary-Display input. Local and Redis quota logs pass
existing numeric fields directly, retaining local nonnegative clamping and raw
signed Redis values. Key material remains absent; templates, decisions and
response behavior are unchanged. Independent plan review passed. New tests
cover signed/pointer extrema, zero scratch allocations and masking/size/drop
parity while retaining unsigned coverage. No full suite, live Redis integration
or throughput benchmark ran for this batch.
Fresh PluginLog tests passed 33 cases; rate-limit selections passed 127 with
one existing ignored case. Numeric scratch fixtures measured zero allocations.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing warnings outside changed files. Independent final review passed.

## ExtProc typed integer log arguments

Request and response header mutation counts and immediate-response HTTP status
now use the canonical PluginLog integer arguments, removing their temporary
Strings. Templates, counts, phase ordering, status validation, mutation behavior
and failure policy are unchanged. Independent plan review passed. Existing
formatter boundary/allocation tests and ExtProc tests provide scoped verification;
this batch adds no log entries or body-phase logging. No throughput benchmark
or deployed integration suite is claimed.
Fresh verification passed 33 PluginLog tests and 140 ExtProc tests. Formatting,
workspace check/Clippy and seven repository guards passed; Clippy warnings
remain outside the changed source. Independent final review passed.

## Wasm typed integer log arguments

Four existing callout/completion/local-response log sites pass counts and status
directly to canonical PluginLog integer formatting, avoiding temporary numeric
Strings. Templates, count extraction, payload release, local response selection
and callback ordering remain unchanged. Guest messages remain unlogged; no
body-phase logs are added. Independent plan review passed. Existing formatter
parity/allocation tests and Wasm tests provide scoped validation, not a new
lifecycle-wide audit or throughput benchmark.
Fresh formatter verification passed 33 tests. The default-parallel Wasm run
passed 207 and failed 56, including preparation-capacity exhaustion and downstream
mock/timer failures. Re-running the same complete Wasm selection with
`--test-threads=1` passed all 263 without source changes; parallel isolation is
not established by that serial pass. No production admission limits were changed.
Formatting, workspace check/Clippy and seven repository guards passed, with
existing Clippy warnings outside the changed source. Independent final review
passed, including the recorded parallel/serial verification limits.

## AllEndpointStatus response ownership

Private JSON response helpers now accept Bytes. Owned serialized/formatted bodies
transfer their allocation to the existing writer; fixed responses use static
bytes. Content-Length remains a byte count, and headers, write order, shutdown,
ignored write errors and termination are unchanged. The endpoint-count log also
uses the canonical integer formatter. Independent plan review passed. Focused
mock coverage checks owned UTF-8, static and empty payloads, pointer preservation,
headers and ordering on successful and failed writes. Probe collection, transport
identity, fan-out and aggregation policy are outside this change. No throughput
benchmark or deployed integration suite is claimed.
Fresh final-source verification passed 23 AllEndpointStatus tests and 33
PluginLog tests after correcting the new mock import. Formatting, workspace
check/Clippy and seven repository guards passed; warnings are outside the
changed source. Independent final code review passed.

## AllEndpointStatus borrowed fan-out inputs

Locally awaited endpoint futures borrow the selected path, query, original
headers, dynamic hop-by-hop list, backend name, transport and semaphore owner.
This removes per-endpoint input String/Vec copies and two Arc increments. The
result address remains owned and reqwest still receives its own Method. The
FuturesUnordered owner is dropped before return, including wall timeout; no
spawned work or new lifetime owner is introduced. Header filtering, endpoint
client isolation, generation checks, permits, timeout and aggregation semantics
are unchanged. Independent plan review passed. Existing helper and transport
tests are scoped evidence, not live forwarding or whole-path allocation proof.
Fresh verification passed 23 AllEndpointStatus tests and 5 hop-by-hop
selections. Formatting, workspace check/Clippy and seven repository guards
passed, with existing warnings outside the changed source. Independent final
review passed. No full suite, live probe or throughput benchmark ran.

## AllEndpointStatus borrowed route inputs

Request endpoint collection now borrows the context-owned route Arc, backend-ref
slice and namespace. It avoids a redundant Arc increment, a deep clone of the
backend-ref vector (including nested configuration), and a namespace String copy.
Collection remains synchronous and produces owned results before fan-out. Denial
checks, namespace fallback, port validation, snapshot capture, resolution order
and endpoint caps are unchanged. Request snapshots and output ownership remain
as before. Independent plan review passed. Existing collection/transport tests
and borrow checking provide scoped evidence; no new live probe or benchmark
is claimed.
Fresh final-source verification passed all 23 AllEndpointStatus tests,
formatting and workspace check/Clippy after resolving the introduced let-else
lint. Seven repository guards passed; remaining warnings are outside the
changed source. Independent final review, including the lint correction, passed.

## AllEndpointStatus lossy text ownership

Probe response decoding consumes the Cow returned by from_utf8_lossy through
into_owned. Invalid UTF-8 now transfers the already allocated replacement String
instead of copying it again; valid UTF-8 retains its existing single copy. Raw
byte accounting precedes decoding, and truncation, streaming caps and replacement
text are unchanged. Independent plan review passed. The existing raw-byte
assembly test uses synthetic completed results, not this network decode branch;
semantic equivalence follows the standard Cow ownership conversion. No live
network or throughput claim is made.
Fresh verification passed 23 AllEndpointStatus tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## DSL decoder intermediate ownership

Base64Decode, HexDecode and UrlDecode retain the lossy decoder Cow through the
existing decoded-byte length check and construct the canonical Arc<str> directly
from its text. This removes an intermediate String copy without changing decoded
content, malformed-input fallback, form-style plus handling or limit errors.
Byte-vector decoding, invalid-UTF-8 replacement storage and final Arc allocation
remain; this is not allocation-free decoding or a Value representation change.
Independent plan review passed. Existing DSL tests provide scoped regression
evidence; no new network integration or throughput benchmark is claimed.
Fresh verification passed all 500 Gateway DSL tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## DSL substring result ownership

Substr constructs its Arc<str> directly from the validated source slice after
the existing byte-length check, removing an intermediate String allocation/copy.
The character-index scan, negative clamps, nil/type handling and empty cases
are unchanged. Expanded existing utility coverage checks multi-byte boundaries,
end-overflow suffixes, equal/reversed ranges and wholly out-of-range indices.
Independent plan review passed. This retains an independent result Arc and
claims no zero-copy substring storage or measured throughput improvement.
The original error-contract table and test comment incorrectly grouped end
overflow with empty results; both now distinguish end clamping from start
overflow after independent scope review. Fresh verification passed all 500
Gateway DSL tests, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed source. Independent final review passed.

## DSL split result capacity

Split reserves its result vector using the already computed and admitted item
count, then extends it with the unchanged split/Arc iterator. Empty delimiters
still count Unicode characters plus two boundary elements; nonempty delimiters
still count non-overlapping matches plus one. This avoids vector growth without
changing scans, limits, item order, empty values or independent string ownership.
Independent plan review passed. Existing split boundary and multibyte cases
provide regression coverage; final Vec/Arc allocations remain, and no throughput
benchmark is claimed.
Fresh verification passed all 500 Gateway DSL tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## DSL UUID stack encoding

Uuid uses the locked uuid crate stack encoding buffer for the same lowercase
hyphenated v4 representation, checks the existing byte-length limit, then
constructs the canonical Arc<str>. Random generation still occurs before the
limit check; only the intermediate String allocation/copy is removed. The
existing UUID test now validates the returned identifier, v4/RFC variant and
canonical 36-byte format at the exact limit; the 35-byte rejection test remains.
Independent plan review passed. No RNG, public representation or failure-policy
change is introduced, and no throughput benchmark is claimed.
Fresh verification passed all 500 Gateway DSL tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## DSL outbound prepared input ownership

Http.call consumes its popped header map in insertion order and moves names into
the existing request-header list instead of cloning them. Value coercion/copying
and opaque-Body framing rejection remain in their original order. The reqwest
builder receives the same parsed URL already used for scheme/SSRF validation;
the locked reqwest Url conversion retains host validation without parsing the
original string again. Client selection, redirect policy, snapshot handling,
limits, error categories and header conversions are unchanged. Independent plan
review passed. No network-policy expansion or throughput benchmark is claimed.
Fresh verification passed all 500 Gateway DSL tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## DSL outbound method normalization audit

The per-call uppercase String is a candidate allocation, but a pure ASCII
replacement changes the Unicode input acceptance of the seven-method selector.
A preserving fallback has unmeasured comparison costs. The linked decision task
records alternatives and validation/error-order boundaries; runtime remains
unchanged. Independent plan review passed.
Formatting, workspace check/Clippy and agent-document validation passed.
Independent final documentation review passed. No runtime test or benchmark
was run for this documentation-only batch.

## DSL outbound header vector capacity

Http.call reserves the temporary request-header vector from the existing Map's
actual length before consuming its entries. Successful calls avoid vector growth;
empty maps and absent headers retain zero capacity. Traversal, value conversion,
Body framing checks and error order are unchanged. Rejected maps now reserve tuple
storage earlier, proportional to their already-owned entries; payload copying is
unchanged. Independent plan review passed. No new helper, network behavior or
throughput claim is introduced.
Fresh verification passed all 500 Gateway DSL tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside the
changed source. Independent final review passed.

## Endpoint local-response static header names

AllEndpointStatus's private JSON writer accepts static extra-header names and
passes them directly to Pingora, removing the temporary owned name for Retry-After.
All current callers use that literal or no extra headers. The locked Pingora
conversion retains identical name bytes and validation through static Bytes;
value ownership, header replacement, write order and ignored write errors remain
unchanged. Existing writer coverage now also checks the exact HTTP/1 header line.
Independent plan review passed. No live protocol or throughput claim is made.
Fresh verification passed all 23 AllEndpointStatus tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Endpoint probe path and query borrowing

AllEndpointStatus borrows get_path() and the canonical request_query() through
its locally awaited fan-out instead of copying path and query strings. The path
still comes from ctx.path(); the query still comes from the current upstream URI,
including trusted rewrites. Query absence, empty text and raw encoding remain
unchanged. No session mutation occurs before these borrows end, and no endpoint
future escapes the call. Owned request headers and URL construction remain.
Independent plan review passed. The existing real-adapter query test checks the
accessor's source, raw/empty/missing cases and allocation behavior; endpoint tests
cover scoped orchestration, not live fan-out interoperability or throughput.
Fresh verification passed 23 AllEndpointStatus tests and the real-adapter query
test, formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed source. Independent final review passed.

## Endpoint fan-out semaphore ownership

The per-invocation semaphore lives directly in the async frame instead of an Arc.
Endpoint futures already borrow it and use borrowed permits; the locally awaited
FuturesUnordered cannot escape, and completion, timeout or cancellation releases
its futures before the owner. Permit counts and per-probe holding scope remain
unchanged. This removes one Arc allocation for nonempty fan-out while enlarging
the inline async frame; no total-memory or throughput reduction is claimed.
Independent plan review passed. Existing tests cover permit-count calculation and
wall-timeout helpers; borrow checking establishes owner lifetime, not real network
admission/cancellation test coverage.
Fresh verification passed 23 AllEndpointStatus tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Endpoint result address ownership audit

Pending probes currently discard eagerly copied addresses on wall timeout, then
assembly copies their selected addresses again. Deferring the first copy until
completion is borrow-safe but moves allocation inside the wall deadline and
permit-holding interval. The linked decision task records this timing tradeoff
and a possible post-fan-out assembly design. Runtime remains unchanged. This is
source evidence, not a demonstrated timing regression or throughput measurement.
Independent plan review passed.
Formatting, workspace check/Clippy and agent-document validation passed.
Independent final documentation review passed. No runtime test or benchmark was
run for this documentation-only batch.

## Response compression transfer-encoding short circuit

Final response compression eligibility rejects remembered or still-present
Transfer-Encoding before examining Content-Length. These immutable predicates
already disqualify compression, so this skips unnecessary text validation and
number parsing without changing accepted lengths, repeated-field handling,
SSE/config policy, sanitization or module execution order. Independent plan
review passed. Existing tests exercise both Transfer-Encoding sources and the
length/config/SSE matrix, plus real compression-module selection. No allocation
or throughput improvement is claimed; the change removes redundant parsing work.
Fresh verification passed all 25 response-filter tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Outbound response JSON recognition without lowercase storage

ManagedOutboundClient compares borrowed Content-Type bytes case-insensitively
instead of allocating a lowercase String. The existing application/json prefix
and +json substring rules remain deliberately unchanged, including matching in
parameters and longer subtypes. First-field selection and non-text rejection
remain unchanged. A real classify_detail test uses a configured body predicate
to cover mixed case, suffixes, permissive matches, short/empty/missing/invalid
values and repeated fields. Independent plan review passed. This removes the
normalization allocation, not JSON body parsing or other outbound costs; no
throughput benchmark is claimed.
Fresh verification passed all 114 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Borrowed lowercase dynamic header names

The shared hop-by-hop classifier borrows names with no ASCII uppercase bytes
instead of allocating an identical lowercase String. Mixed-case names retain the
original conversion and exact comparison against the dynamic list, including
noncanonical-list rejection and non-ASCII byte preservation. Static and empty-list
short circuits remain. Canonical HeaderMap names benefit without a new parser or
caller policy. Mixed-case inputs incur an additional uppercase scan; this is a
scoped allocation improvement, not a universal CPU or throughput claim.
Independent plan review passed. Allocation tests construct lists outside the
measured region and check lowercase hit/miss and allowed UTF-8 names; semantic
checks retain mixed-case and noncanonical-list behavior. Repeated Connection
field handling remains outside this change.
Fresh verification passed 190 shared HTTP tests, 27 RequestMirror tests and 23
AllEndpointStatus tests, formatting, workspace check/Clippy and seven repository
guards. Remaining warnings are outside changed source. Independent final review
passed.

## Outbound status-only classification skips media-type work

ManagedOutboundClient inspects Content-Type only when a nonempty body-predicate
list is configured. The canonical resource-layer classifier already ignores the
JSON flag for absent or body-free success policies; it still owns all status and
body decisions. Existing JSON recognition remains unchanged when needed. Actual
client tests cover absent/default/custom-status policies against missing, plain,
JSON and invalid Content-Type values with malformed JSON bodies. Independent plan
review passed. This skips irrelevant header lookup/validation/scanning, without
claiming measured CPU or throughput gains or duplicating status logic.
Fresh verification passed all 115 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Outbound URL assembly without intermediate path storage

CompiledRequest formats the trimmed base, borrowed optional slash and rendered
path directly into the final URL text. This removes the separate cloned or
slash-prefixed path String while retaining exact concatenation bytes, rendering,
override evaluation, parsing and error order. It does not replace concatenation
with URL joining or path setters. Actual request-builder tests cover empty,
relative, leading/double-slash paths, base path/trailing slashes, query, percent
encoding and fragments, through templates and explicit overrides. Independent
plan review passed. Final URL and template allocations remain; no throughput
benchmark is claimed.
Fresh verification passed all 116 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Outbound Cookie assembly without per-field strings

CompiledRequest appends collected cookie names, equals signs, values and ordered
separators directly to the final String, removing per-field formatted Strings
and their intermediate Vec. Collection order, duplicates, unescaped delimiter
text, sensitivity and HeaderValue validation remain unchanged. Empty or invalid
assembly leaves any earlier Cookie header intact; valid assembly replaces it.
Actual builder tests check these cases with and without a pre-existing Cookie.
Independent plan review passed. The final String can still grow and collected
fields remain owned; no single-allocation or throughput claim is made.
The first Clippy run identified a new test type-complexity warning; a local type
alias removed it. Fresh verification on the corrected source passed all 117
shared outbound tests, formatting and workspace check/Clippy; seven repository
guards passed. Remaining warnings are outside changed source. Independent final
review and the follow-up test-only correction review passed.

## Outbound path and method input borrowing

CompiledRequest keeps rendered path/method results owned while borrowing explicit
path overrides and frozen entry path/method values through local Cow values.
The synchronous builder reads the same accessors in the same order, retaining
trusted-context capability errors and the missing-entry-method error. Final URL
and Method construction remain owned. Independent plan review passed. Existing
actual-builder tests cover frozen versus current values, missing entry metadata,
trusted capabilities and path overrides. This removes temporary copies only on
the borrowed branches; template allocation and final request ownership remain,
and no throughput benchmark is claimed.
Fresh verification passed all 117 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Outbound form literal borrowing

CompiledRequest borrows compiled literal form values while retaining owned
rendered template values. The existing form serializer consumes the same text
in the same order, preserving encoding, template errors and body framing. This
removes the temporary nonempty literal String clone; final encoded body storage
and template results remain owned. The existing real-builder form test now also
checks reserved characters, allowed UTF-8 and empty literal values alongside the
dynamic template, Content-Type and Content-Length. Independent plan review
passed. No throughput benchmark is claimed.
Fresh verification passed all 117 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
source. Independent final review passed.

## Outbound collected field ownership

The private collected-field representation borrows names and literal values from
compiled configuration. Origin values and template results remain owned, with
all provenance flags unchanged. Query, Cookie and header consumers finish while
the configuration borrow is valid and still produce owned request data. Presence
policy, evaluation order, encoding and header overwrite behavior are unchanged.
Independent plan review passed. A literal-only collector test preallocates its
output Vec outside the measured region and verifies zero allocation calls/bytes,
contents and sensitive flags. This isolates eliminated field copies; final
request construction and dynamic rendering are not allocation-free, and no
throughput benchmark is claimed.
Fresh verification on final source passed all 118 shared outbound tests,
formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed source. Independent final review passed.

## Outbound completed-response header ownership transfer

After status retry, Retry-After and declared-length checks, the shared call loop
moves the response HeaderMap into completed-response ownership instead of cloning
it. The private response seam exposes headers_mut for real and scripted responses.
Locked reqwest bytes_stream consumes only the body, so moving headers at this
point leaves streaming caps, errors and health/retry decisions unchanged. Existing
transport coverage now verifies repeated Set-Cookie order and sensitivity; a
synthetic real reqwest response test checks zero-allocation header transfer,
value storage identity and subsequent capped body consumption. Independent plan
review passed. The test knowledge page's stale source path is corrected. This is
not a live network or whole-exchange allocation/throughput claim.
Fresh verification passed all 119 shared outbound tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
lines. Independent final review passed.

## Outbound authority child Arc borrowing

The outbound call renders a compiled authority by borrowing it from the effective
policy already returned by the captured snapshot. This removes an extra child
Arc clone/drop; the owned render result leaves the lookup closure. Policy and
snapshot lifetime now includes synchronous rendering but ends before request
execution or retry waits. Lookup generation, render count/order, destination
pinning, rejection metric and AuthorityRejected behavior remain unchanged.
Independent plan review passed. Existing outbound transport and authority tests
cover absent policy, policy/trusted value changes and rejected labels. This is a
reference-count reduction, not a heap-allocation or throughput claim.
Fresh verification passed 119 shared outbound tests and 6 authority tests,
formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. Independent final review passed.

## Outbound static base URL borrowing

ResolvedTarget::pick_url and ManagedOutboundClient::dial_url carry a borrowed
static base URL through request construction. Service targets retain fresh
endpoint lookup, cursor advancement and owned formatted URLs; dynamic authority
replacement still owns its result. Literal SSRF validation remains in the same
place. The active-health caller explicitly takes ownership before its existing
probe helper, preserving that path. Independent plan review passed. A scoped
static-target test checks borrowed storage identity and zero allocation calls
and requested bytes. Final request URLs remain owned, and validation may still
allocate; no whole-call allocation or throughput claim is made.
Fresh verification passed 120 shared outbound tests and 14 webhook runtime
tests, formatting, workspace check/Clippy and seven repository guards.
Remaining warnings are outside changed code. Independent final review passed.

## Session persistence token borrowing

HTTP and gRPC affinity selection borrow extracted Cookie/Header tokens from the
live request headers through synchronous lookup. The canonical cookie parser and
wire-token parser retain first-match, empty-value, malformed-expiry and expiry
boundary semantics. Valid live tokens return a hash slice instead of allocating
and truncating a String. No token borrow escapes into context or later mutable
Session operations. Independent plan review passed. Real H1 extraction tests
cover both sources, storage identity and zero scoped allocation calls/bytes;
this is not a whole-selection allocation or throughput claim. The owning review
knowledge distinguishes current borrowing from historical owned-buffer reuse.
Fresh verification passed 23 session-persistence, 4 cookie-parser, 103 upstream
peer and 1 gRPC integration-module tests, formatting, workspace check/Clippy and
seven repository guards. Remaining warnings are outside changed code. Independent
final review passed. These are in-process tests, not new live integration runs.

## Session persistence bare wire-token ownership

The response injection path transfers its generated hash into build_wire_token.
Without a lifetime, the helper returns that same String rather than copying it;
with a lifetime, the existing formatting and saturating expiry addition remain
unchanged. Clock reads, captured policy and response injection ordering remain
unchanged. Independent plan review passed. A scoped test verifies pointer
identity and zero allocation calls/bytes for the bare-token step; exact expiry,
zero TTL and saturation checks cover the formatted branch. Hash generation and
final header/Cookie construction still allocate; no throughput claim is made.
Fresh verification passed 24 session-persistence and 11 upstream-response-filter
tests, formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. Independent final review passed.

## Response Connection rejection short-circuit

The response sanitizer returns its existing fixed InvalidHTTPHeader error at the
first non-text Connection field, avoiding unused name parsing and allocations in
later fields. No header mutation occurs on failure; successful sanitization still
validates every repeated field before removals. Invalid ASCII options, duplicate
handling, 101 bypass and framing policy remain unchanged. Independent plan review
passed. A prepared invalid-first fixture with and without later custom nominations
checks equal error-path allocation calls/bytes and unchanged header maps. Existing
invalid-middle and spilled-invalid-last cases remain. The owning knowledge page
now distinguishes atomic mutation from unnecessarily exhausting a rejected input.
This is a rejected-input optimization, not a valid-traffic throughput claim.
Fresh verification passed 26 response-filter tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code. Independent final review passed.

## BandwidthLimit prepared arithmetic operand

The concrete read-only body filter already parses its rate at construction and
has no per-chunk state, allocation or logging. Its immutable denominator is now
converted from the parsed u64 to f64 at construction as well, removing that
repeated conversion from nonempty chunks. Division and Duration conversion remain
unchanged; this does not replace division with reciprocal multiplication or alter
per-response-stream scope. Independent plan review passed. A constructor/callback
matrix compares the previous arithmetic for zero, small, power-of-two, precision
boundary and maximum rates across empty/nonempty chunks and both EOS flags.
No generated-code or throughput benchmark is claimed.
Fresh verification passed 4 BandwidthLimit and 26 response-filter tests,
formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. Independent final review passed.

## gRPC transcoder canonical status check

The converter's status parser retains its nonempty, at-most-two-byte ASCII-digit
check, integer parse and 0..16 range. It now rejects two-digit leading-zero text
directly instead of formatting the parsed code into a temporary String for a
round-trip comparison. Missing/duplicate fields, original status evidence and
terminal outcome ownership are unchanged. Independent plan review passed. Tests
cover all canonical codes, all two-digit forms, malformed text, opaque bytes,
missing and duplicate values. Prepared valid HeaderMaps verify zero allocation
calls/bytes within status parsing alone; converter-wide allocation and throughput
are not claimed.
Fresh verification passed all 26 gRPC JSON transcoder tests, formatting,
workspace check/Clippy and seven repository guards. Remaining warnings are
outside changed code. Independent final review passed.

## Contiguous binary window borrowing

RawWindow::binary_bytes borrows an empty, single-slice or coalesced message via
the existing SliceChain range view. An O(1) handle-count branch leaves fragmented
messages on their original allocation and single copy pass. State checks and the
full existing materialization admission remain before either path, preserving
acceptance limits even when storage is borrowed. The sole streaming transcoder
consumer releases the view after owned JSON decoding and before window resolution.
Independent plan review passed, including the revision that avoids a fragmented
range pre-scan. Tests cover contents, retained pointer identity, zero allocation
for contiguous views, owned fragmented fallback and unchanged admission/state
gates. This removes a contiguous message copy, not full-response buffering or all
conversion allocations; no throughput claim is made.
Fresh verification passed 80 SSE/window and 26 gRPC JSON transcoder tests,
formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. Independent final review passed.

## Guardrail decision semaphore borrowing

Semantic Guardrail decisions now borrow their processor-owned semaphore for the
locally awaited outbound call instead of cloning an Arc into an owned permit.
The permit scope, queue-inclusive timeout, closed-semaphore error and dependency
failure mapping are unchanged. This removes one Arc increment/decrement pair;
no allocation or throughput improvement is claimed. Independent plan and final
code reviews passed. Four network-free tests cover waiting cancellation without
an extra owner, semaphore closure, acquired-permit release after health rejection,
and the existing saturated-queue deadline. Formatting, workspace check/Clippy
and seven repository guards passed; remaining warnings are outside changed code.

## Guardrail decision token borrowing

The bounded unary decision parser borrows unescaped version/action strings from
its input through serde Cow fields. Escaped strings retain owned decoding, and
all strict envelope checks, error ordering and owned replacement output remain
unchanged. Independent plan and code reviews passed. Tests verify zero allocation
for prepared plain pass/reject documents without optional fields, escaped token
equivalence and malformed/duplicate/missing/type/trailing error categories. This
removes temporary token Strings; optional payloads and escaped decoding may still
allocate, and no end-to-end throughput improvement is claimed.
Fresh verification passed all 6 protocol tests, formatting, workspace check/Clippy
and seven repository guards. Remaining warnings are outside changed code.

## Guardrail replacement ordering without sort scratch

Decision parsing now uses in-place unstable sorting for replacement keys. Every
accepted key is unique, so successful output order and key/text association stay
identical; duplicate keys still return the same fixed error. Byte accounting and
cap rejection retain their precedence before sorting. This avoids the stable
sort's auxiliary heap storage on larger lists, without claiming throughput gains
or eliminating parsing allocations. Independent plan and code reviews passed.
Fresh verification passed all 8 protocol tests, including 256 scrambled keys,
field/tool ordering, exact-cap acceptance and duplicate/error precedence,
formatting, workspace check/Clippy and seven repository guards. Remaining warnings
are outside changed code.

## ExtProc unchanged trailer map ownership

A matched response-trailer verdict with no header mutation now validates the
original HeaderMap instead of cloning and replacing it. Mutation-bearing verdicts
retain the candidate clone and atomic commit, including explicit empty mutations.
Validation, transport representability, failure policy and terminal ordering are
unchanged. Independent plan and code/knowledge reviews passed. The Scripted
callback regression checks original HeaderValue storage identity, duplicate
opaque bytes, sensitivity metadata, final exchange and terminal success across
HTTP/gRPC and both failure policies. Projection still owns protobuf values;
this is not an allocation-free callback or a throughput claim.
Fresh verification passed all 17 response-trailer tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code. The owning header-fidelity knowledge page describes the narrowed clone scope.

## ExtProc local-reply header name ownership

Immediate-response conversion now consumes its owned HeaderMutation and moves
accepted set/remove names into PendingHeaderOp. Both the request-stage local
reply and streamed request-body terminal reply already own that mutation.
Typed value decoding, empty-value handling, protected-header rejection, append
conditions, set-before-remove order and log ordering remain unchanged. Independent
plan and code/knowledge reviews passed. A regression verifies retained String
storage for set/remove names and opaque value fidelity; fixture clones remain
only where tests reuse one mutation across independent operations. This removes
accepted-name copies, not value decoding or all conversion allocations.
Fresh verification passed 18 mutation, 18 streamed and 5 immediate-reply tests,
formatting, workspace check/Clippy and seven repository guards. Remaining warnings
are outside changed code. The owning header-fidelity knowledge page records the
new name transfer boundary. No throughput improvement is claimed.

## AiGuard evaluator decision parsing

The request evaluator now borrows unescaped version/action tokens through serde
Cow and sorts accepted replacement keys in place without stable-sort scratch.
Escaped token decoding retains owned fallback. Strict request-shaped envelope
fields, parsing/error precedence, duplicate rejection and replacement-byte caps
are unchanged; returned replacement text remains owned. The request envelope
stays separate from the response protocol to preserve its strict schema.
Independent plan and code reviews passed. Tests cover plain pass/reject zero
allocation, escaped actions and versions, malformed/error boundaries, 256 mixed
request keys with exact text association, and duplicate/budget precedence.
Fresh verification passed all 60 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code. Optional payloads and escaped strings may still allocate; no whole-call
allocation or throughput improvement is claimed.

## AiGuard evaluator semaphore borrowing

The evaluator's locally awaited outbound call now borrows its snapshot semaphore
permit instead of cloning an Arc into an owned permit. Queue waiting remains
inside the same absolute timeout, and error mappings and release scope are
unchanged. Independent plan and code reviews passed. Four actual-decide tests
cover saturated-queue timeout, pending cancellation without another Arc owner,
closed semaphore and acquired-permit release after early health rejection.
All use nonempty spans and prevent network access. This removes an Arc
increment/decrement pair, not a heap allocation or a measured throughput cost.
Fresh verification passed all 64 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code.

## AiGuard typed count logging

Masked and flagged outcome logs now pass usize counts directly to the canonical
PluginLog integer formatter instead of creating temporary decimal Strings.
Templates, trigger branches, sensitivity, entry limits and final-decision timing
are unchanged. Independent plan and corrected code reviews passed; final-source
wiring tests check masked 2 and flagged 0/1/42, while existing formatter tests
cover decimal equivalence, integer bounds, masking and capacity behavior.
Fresh verification passed 65 AiGuard request and 33 PluginLog tests, formatting,
workspace check/Clippy and seven repository guards. Remaining warnings are outside
changed code. This removes temporary count Strings; no throughput gain is claimed.

## AiGuard ordered evaluator projection

The private post-local-mask projection now advances one replacement cursor instead
of restarting a linear search for every span. Its sole production caller receives
spans sorted by select_spans and replacements sorted by CompiledRules::evaluate,
before external decisions are merged. The helper documents this precondition.
Lookup work is O(spans + replacements) instead of worst-case quadratic; output
order, first-equal selection and borrowed text ownership remain unchanged without
an additional index or string copy. Final validation and publication are untouched.
Independent plan and code reviews passed. The regression drives the real rule
producer with reversed inputs, then compares a 256-span projection against the
previous linear reference and exact text pointers, including sparse first/last
masks, Pass, empty replacement lists and empty span input.
Fresh verification passed all 66 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code. No end-to-end throughput benchmark is claimed.

## AiGuard external-only replacement ownership

When no local replacements exist and external keys are strictly increasing,
merge returns the evaluator's existing Vec directly. It retains the old path for
unsorted or duplicate external input, including last-value-wins behavior. Empty
results still become Pass. The production parser constructs the external Vec
with one slot per replacement; final key membership, uniqueness, decoded-byte
budget and body publication checks remain unchanged. Independent plan and code
reviews passed. Tests cover empty/single/256-item inputs across the helper's
empty-local variants, pointer/capacity retention with zero allocation in the
fast path, and unsorted/sorted-duplicate fallback results. This avoids rebuilding
the external-only collection and its quadratic insertion searches; it does not
optimize the general nonempty-local merge or claim throughput gains.
Fresh verification passed all 68 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code.

## AiGuard evaluator cardinality before merge

A small evaluator-specific boundary now rejects external replacement counts above
the selected-span count before calling merge. The parser already rejects duplicate
external keys, and merge retains each distinct external key, so this case always
failed final admission with the same evaluator bad key classification. The caller
uses the existing dependency failure policy; general merge behavior and final
membership, uniqueness, byte budget and atomic publication checks remain intact.
One source-derived example with 1,000 distinct empty external replacements fits
in 59,945 compact JSON bytes yet previously required 500,500 key comparisons when
merged with one disjoint local item before count rejection. This is a constructed
comparison count, not a runtime benchmark or a throughput claim.
Independent plan and code/knowledge reviews passed. Tests verify oversized input
rejection without merge allocation, inclusive count and overrides, and retained
final union-count/byte checks. Fresh verification passed all 70 AiGuard request
tests, formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. The owning budget knowledge page records this
early necessary bound separately from final admission.

## AiGuard ordered replacement merge

For strictly ordered unique local and external keys, merge now finds overrides
by binary search while local keys stay fixed, moves replacement Strings into
those slots, compacts unmatched external entries in their existing Vec, then
appends and sorts only when new keys remain. Both order guards protect the old
fallback for arbitrary duplicate or unsorted helper inputs. No separate output
Vec or lookup index is added; appending may still grow local capacity. Including
guard scans, fully overlapping sets take O(n + m log(n + 1)) comparison work;
the general ordered path additionally sorts the combined output rather than
performing repeated linear insertion searches. Final admission and publication
are unchanged. Independent plan and code reviews passed.
Differential tests cover 1,024 subset pairs across content/text-part keys and
explicit duplicate/unsorted fallbacks. A 256-item full-overlap regression verifies
zero allocation, retained local Vec storage and moved external String pointers.
Fresh verification passed all 72 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards. Remaining warnings are outside changed
code. No all-input zero-allocation or end-to-end throughput result is claimed.

## AiGuard ordered final key validation

Final replacement validation now uses a linear membership scan when both selected
spans and replacement keys are strictly increasing. These checks also establish
uniqueness; unordered or duplicate inputs retain the original membership and
prefix-duplicate logic, preserving valid unordered acceptance. Empty replacement
lists return immediately as before. Count admission stays first, all key checks
precede decoded-byte accounting, and publication remains unchanged. The owning
budget knowledge page records both paths. Independent plan and corrected final
code/knowledge reviews passed.
Differential tests compare the original validator across empty, sorted, unsorted,
duplicate and unknown keys, mixed content/text-part keys and multiple budgets.
A 256-span sparse fixture checks inclusive byte bounds without allocation.
Fresh final-source verification passed all 74 AiGuard request tests, formatting,
workspace check/Clippy and seven repository guards. The introduced Clippy warning
was corrected; remaining warnings are outside changed code. Ordered key work is
linear, but no claim is made that every small input or full request is faster.

## AiGuard replacement encoding lookup

After final key and byte admission, encoding multiple replacements now checks
span-key order once and uses binary search for strictly increasing span keys.
Unordered or duplicate span keys retain the original first-match lookup; empty
and single-item decisions skip the ordering scan. No index allocation is added.
JSON encoding, token-range sorting, overlap rejection, failure policy and the
single generation-bound publication remain unchanged. The owning budget knowledge
page records these lookup boundaries.

A real BodyStorage test compares emitted edits and complete output against the
original linear lookup across 18 combinations of span order, replacement order
and replacement count, including mixed content/text-part keys and a later
duplicate key pointing at a different token. Empty edits retain the generation;
nonempty edits advance it once. Independent plan and code/knowledge reviews passed.
Fresh verification passed 75 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards; remaining warnings are outside changed
code. Ordered lookup costs O(n + m log n) instead of O(n*m), without a claim that
every small input or full request is faster.

## AiGuard borrowed fixed JSON vocabulary

Request span selection now borrows unescaped role and part-type values through
a shared serde-backed borrowed JSON string decoder. Escaped values remain owned;
the original encoded-size precheck, unknown-role and skipped-part behavior remain
unchanged. Selected content still uses the unchanged owned decoder, as do the
parser and SSE consumers. No parser container or body-storage ownership changes.
The owning span-bounds knowledge page records the boundary.

Independent plan and code/knowledge reviews passed. New decoder differential
cases preserve complete-token acceptance for whitespace, trailing garbage,
wrong types, invalid UTF-8, control characters and escapes. Plain fixed values
and generic unescaped strings verify source-pointer identity and zero allocation;
escaped values verify owned results. Encoded admission edges retain their prior
meaning. Fresh verification passed 76 AiGuard request tests, 5 JSON span tests,
formatting, workspace check/Clippy and seven repository guards. Remaining warnings
are outside changed code. This removes temporary plain role/type strings, not
whole-parser allocations; no end-to-end throughput benchmark is claimed.

## AiGuard lazy local mask slots

Local rule evaluation now allocates its per-span mask slots at the first Mask
action that passes role filtering. Until then, Allow and Reject borrow original
span text through an empty slot lookup. The first Mask retains the original
exact span-count allocation; subsequent mask updates, budget checks, rule order,
flagged counts and replacement extraction remain unchanged. No extra text copy,
compiled flag or parallel rule evaluator is introduced. The owning budget
knowledge page documents the initialization boundary.

Independent plan and code/knowledge reviews passed. Six warmed allocation cases
cover allow-only, reject match/no-match, allow-before-reject, role-excluded Mask
and rejection before a later Mask; all report zero allocation. A separate ordered
Allow/Mask/Allow/Reject test verifies accumulated text visibility and flagged
counts. Existing shrink, revert and aggregate-budget cases remain intact.
Fresh verification passed 78 AiGuard request tests, formatting, workspace
check/Clippy and seven repository guards; remaining warnings are outside changed
code. Regex cache initialization can still allocate, and applicable Mask paths
retain their slot allocation. No full-request throughput result is claimed.

## AiGuard selection traversal order

Removed the final sort from select_spans: successful traversal already emits
strictly increasing GuardSpanKey values. Message indices increase, each message
chooses either one Content span or increasing TextPart indices, and skips only
leave gaps. The producer contract and owning span-bounds knowledge document this
proof. All consumer ordering checks and generic replacement sorting remain intact.

Independent plan and code/knowledge reviews passed. A regression checks exact
key/text sequences and original quoted-token ranges across 0, 1, 4 and 128 messages
with mixed content variants, skipped messages/parts, index gaps and escaped text.
Existing ambiguity and bounds tests remain unchanged. Fresh verification passed
79 AiGuard request tests, formatting, workspace check/Clippy and seven repository
guards; remaining warnings are outside changed code. The benefit is removal of
redundant sorting work on an already ordered list, not an asserted allocation or
O(n log n) improvement; no throughput benchmark ran.

## Request restriction and GraphQL frontend inspection

RequestRestriction's direct runtime uses a prepared status/body and compiled
conditions; its Bytes clone transfers shared response ownership. No change was
selected there. Inspected GraphQL shared transport/evaluation admission and its
Safelist, Guard, RateLimit and OperationRouter callers: sequential executions can
repeat parsing, but parsed-owner reuse changes admission/memory lifetimes and
per-evaluation Work accounting. The deferred task records these decisions,
mutation/proof boundaries and future acceptance; no runtime cache was added.
This is source evidence, not fresh runtime or throughput verification. Deeper
condition evaluation, GraphQL visitors and routing dependencies remain open.

## GraphQL GET skips POST-only operation digest

Shared GraphQL evaluation now computes selected-operation cache-proof metadata
only for POST. GET previously computed and discarded the same SHA-256 digest;
its response-cache key uses the bodyless branch and never consumes this proof.
POST retains the exact domain, name-presence byte and name bytes, calculated
before computation, with proof publication still gated by successful evaluation
and a current body generation. Parsing, Work, permits, failures and body freezing
are unchanged. This does not implement the deferred frontend-reuse proposal.

Independent plan and corrected code reviews passed. New GET coverage verifies
named/anonymous selection, mutation refusal, no POST proof even with an installed
body owner, successful freeze and permit release. POST coverage asserts exact
digest bytes for named and anonymous operations. Fresh final-source verification
passed 7 shared GraphQL tests, 38 response-cache tests, formatting, workspace
check/Clippy and seven repository guards. An initial test mock type error was
corrected before these successful runs; remaining warnings are outside changed
code. No whole-request allocation or throughput improvement is claimed.

## FormJsonTransform borrowed boolean text

JSON-to-form pair encoding now borrows static true/false text instead of
allocating a temporary String for each boolean. String values still transfer
their ownership, numbers retain their original formatting, and unsupported shapes
still reject. Component limits, checked pair counters, separator/encoding order,
output caps, body publication and header-failure rollback remain unchanged.

Independent plan and code reviews passed. Prepared-writer tests establish zero
allocation for boolean pair encoding and count admission, including MAX_PAIRS
and usize overflow behavior. Complete conversion checks exact 14-byte output and
rejection at 13 bytes. Fresh verification passed 16 FormJsonTransform tests,
1 CappedWriter test, formatting, workspace check/Clippy and seven repository
guards. Remaining warnings are outside changed code. Parsing and output-buffer
allocations remain; no full-transform zero-allocation or throughput claim is made.

## FormJsonTransform direct grouped serialization

Form-to-JSON now groups decoded values as a private untagged Single/Multiple
representation in the existing ordered map. A single value owns its String
directly; the second occurrence moves it into a Vec and later occurrences append.
The map serializes directly through the existing capped, panic-isolated writer,
removing singleton Vecs and the second serde_json object/array construction.
Key ordering, encounter order within repeated keys, scalar/array shape, decoding
and count/error precedence, output bounds, publication and rollback are unchanged.

Independent plan and code reviews passed. A retained original implementation in
tests provides exact byte/error comparisons across empty/single/repeated keys,
percent-equivalent names, escapes, controls, UTF-8, malformed components, cardinality
and component limits, plus exact/insufficient output caps. A 64-distinct-key fixture
verifies fewer allocation calls and cumulative requested bytes with identical
output. Fresh verification passed 18 transformer tests, 1 CappedWriter test,
formatting, workspace check/Clippy and seven repository guards. Remaining warnings
are outside changed code. No peak-memory, all-input allocation or throughput
improvement is claimed.

## FormJsonTransform borrowed plain components

Form-to-JSON grouping now retains Cow text: components without plus or percent
escapes borrow the original input, while decoded components keep owned Strings.
The escape precheck examines at most limit+1 bytes. Including the first over-limit
byte preserves malformed-percent precedence; if that prefix is plain and input
is longer, TooLarge precedes UTF-8 validation as before. Escaped inputs retain the
original decoding loop and limits. Grouping/ordering and serialization compare
text identically, and all borrows end inside synchronous conversion. The original
body was already retained for header-failure rollback; its lifetime is unchanged.

Independent plan and code reviews passed. The previous full-conversion reference
now uses an independent original decoder. New differential cases cover every
single byte, UTF-8, valid/invalid escapes around the limit and saturated limits;
plain inputs assert source-pointer identity and zero allocation, escaped values
remain owned. Fresh verification passed 20 transformer tests, 1 CappedWriter test,
formatting, workspace check/Clippy and seven repository guards; remaining warnings
are outside changed code. Group/output allocations remain, and the additional
bounded escape scan is not claimed to improve CPU time for every input.

## FormJsonTransform direct JSON object iteration

JSON-to-form now calls serde_json Map::sort_keys and consumes the object directly,
removing the intermediate member Vec and its separate sort. The locked 1.0.151
implementation is a no-op for the current ordered BTreeMap backend; its
preserve_order implementation sorts in place by the same string ordering.
Strict parsing, sorted first-error precedence, array order, output caps and
body publication/rollback remain unchanged.

Independent plan and code reviews passed. An original-implementation reference
checks exact bytes/errors for mixed scalar/array and UTF-8 fields, invalid shapes,
duplicate keys, both orders of competing BadRequest/TooLarge failures, 256/257
pairs and exact/insufficient output caps. A 64-key fixture verifies fewer
allocation calls and cumulative requested bytes with identical output. Fresh
verification passed 22 transformer tests, 1 CappedWriter test, formatting,
workspace check/Clippy and seven repository guards. Remaining warnings are
outside changed code. No alternate-feature build, peak-memory or throughput
improvement is claimed.

## JWE borrowed token and fixed segment views

JweDecrypt now borrows the trimmed token from the selected request header until
synchronous decryption returns owned plaintext. Optional prefix matching remains
case-sensitive, with the same trim order, first-header selection and invalid-text
handling. Compact parsing consumes exactly five split fields and rejects a sixth
without collecting a segment Vec. Empty-field and encrypted-key checks, strict
protected JSON, algorithms, original encoded AAD, key-failure precedence, payload
publication and credential hiding remain unchanged.

Independent plan and code reviews passed. New tests cover extraction text and
source-pointer identity with zero allocations, empty/prefix/duplicate/non-text
header cases, allocation-free structural rejection including a long dot sequence,
and borrowed segment identity with a real AES-GCM roundtrip. Existing strict
protected-header, raw-AAD and key-failure tests remain. Fresh verification passed
18 JWE tests, four header-accessor tests, formatting, workspace check/Clippy and
seven repository guards; remaining warnings are outside changed code. Header
JSON decoding and decryption still allocate, and no throughput claim is made.

Adjacent inspection found JsonSchemaValidation already retains its compiled
validator and uses borrowed body parsing input with operation-specific failure
policy. Canary retains borrowed profile selection and creates owned jump metadata
for later phases. No ownership change was selected at those boundaries.

## JWE fixed IV and authentication-tag decoding

JweDecrypt decodes the 12-byte IV and 16-byte authentication tag into stack arrays
using base64's checked decode_slice API. Returned lengths still enforce exact
sizes before AES-GCM use; ciphertext retains its owned mutable buffer. Oversized
fixed fields may now reject earlier, but every skipped decode/length failure
has the same InvalidFormat result and no external side effect. Protected-header
validation, original encoded AAD, key precedence and authentication failures are
unchanged.

Independent plan and code reviews passed. An original decrypt implementation
provides differential results for valid tokens, decoded lengths 0..33, every
ASCII last byte, padding, simultaneous malformed fields and corrupted ciphertext.
Two successful fixtures assert identical plaintext, exactly two fewer allocation
calls and fewer cumulative requested bytes. Fresh verification passed 20 JWE
tests, formatting, workspace check/Clippy and seven repository guards. Warnings
remain outside changed code. Header parsing and ciphertext still allocate; no
whole-request zero-allocation or throughput improvement is claimed.

Local/Redis rate-limit rejection header construction was also inspected. Their
custom names are passed as owned Strings because the pinned Pingora
IntoCaseHeaderName supports owned names or static str and retains original case.
Directly borrowing those dynamic str values is not an available equivalent API;
no mutation was selected there. Integer HeaderValue substitution also retains the
storage tradeoff already recorded in the outbound Content-Length decision.

## BasicAuth fixed SHA-1 verification scratch

The htpasswd SHA-1 verifier decodes its expected digest into a 20-byte stack
buffer through checked decode_slice. The returned decoded length gates the same
full XOR fold; short, oversized or malformed expected hashes still fail. Oversized
stored hashes can fail before password hashing, without changing the boolean
result or caller failure handling. Generation ownership, blocking dispatch,
authentication delay, log policy and supported hash formats are untouched.

Independent plan and code reviews passed. Differential tests retain the original
verifier and cover decoded lengths 0..40, empty/ASCII/UTF-8 passwords, padding,
every ASCII replacement in the last two encoded positions and a mismatch at each
digest byte. Correct and incorrect password fixtures verify zero heap allocations
inside the SHA-1 helper. Fresh verification passed 19 BasicAuth tests, formatting,
workspace check/Clippy and seven repository guards; existing warnings are outside
the changed files. This does not claim zero-allocation complete authentication,
new timing guarantees, credential erasure or throughput improvement.

## FaultInjection typed abort status logging

FaultInjection passes its u16 abort status directly to the existing PluginLog
integer formatter, removing the temporary decimal String. The lossless u64
argument uses the shared stack formatter and preserves the exact log bytes,
masking and bounded-entry behavior. Random selection, delay, header ordering,
status and clone-cheap response body are unchanged. The u128 delay-millisecond
formatting remains outside this change; no numeric narrowing or shared argument
layout expansion was introduced.

Independent plan and code reviews passed. Existing response wiring tests assert
the exact abort log, body and ordered duplicate headers; shared formatter tests
cover integer boundaries, masking, entry limits and scratch allocation. Fresh
verification passed seven FaultInjection tests and 29 selected PluginLog tests,
formatting, workspace check/Clippy and seven repository guards. Warnings are
outside changed code. No new test duplicating the existing coverage was added;
the performance claim is limited to the removed temporary status String.

## Certificate IP SAN projection formatting

The canonical certificate extractor keeps IP SANs as private IpAddr values until
the selected projection. Display formatting now emits the IP prefix and address
together without first owning a bare address String; typed matching still owns
its canonical address text. IPv4 decimal and IPv6 std formatting, invalid-length
skips, type prefixes, duplicate order and separate OtherName traversal remain
unchanged. The owning review knowledge now describes this intermediate accurately.

Independent plan and code reviews passed. A signed raw-SAN certificate checks
independent literal expectations for zero/max IPv4, zero/loopback/compressed,
IPv4-mapped and max IPv6 addresses in both projections. Allocation comparisons
reproduce the old IPv4 four-octet formatting and IPv6 to_string path; maximum
IPv4/IPv6 fixtures show fewer calls and cumulative requested bytes with identical
display output. This is not a universal allocation count or throughput claim.
Fresh verification passed 13 extractor, 20 HeaderCertAuth and 14 mTLS callback
tests, formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code; no live TLS handshake suite was run.

## OtherName integer conversion working storage

After the existing nonempty/64-byte INTEGER gate, certificate extraction copies
the admitted bytes into one fixed stack scratch prefix. Signed complement/carry
and decimal division mutate that same prefix instead of allocating separate
magnitude and division Vecs. Original BER bytes, the conversion algorithm, sign
formatting, digit/output storage and accepted scalar bounds remain unchanged.
The zero case retains its existing output allocation and now has a bounded copy;
no all-input CPU improvement is claimed.

Independent plan and code reviews passed. Tests compare the original algorithm
across single-byte values, representative two-byte and 1..64-byte patterns,
independent i64 limits and 0/65-byte rejection, and verify source immutability.
Signed certificates confirm negative OtherName values in display and typed
projections. The 64-byte nonzero positive and negative fixtures respectively
remove exactly one and two working allocations (64 and 128 requested bytes).
Fresh verification passed 16 extractor, 20 HeaderCertAuth and 14 mTLS callback
tests, formatting, workspace check/Clippy and seven repository guards. Remaining
warnings are outside changed code. Output still allocates; no live-handshake,
peak-memory or throughput result is claimed.

## OtherName decimal output without temporary heap digits

The admitted 64-byte INTEGER magnitude now writes decimal digits backward into a
193-byte stack buffer, reserving room for a sign. The bound follows from
256^64 < 1000^64; the existing private call chain is gated before conversion.
The division and complement/carry algorithms are unchanged. One final String
copies only the occupied ASCII slice, replacing the growing digit Vec, reversal
and negative-value formatting copy. This adds bounded stack storage rather than
retaining the digit Vec's spare capacity in typed SAN output.

Independent plan and code reviews passed. Existing old-algorithm differential,
i64-limit, BER-boundary and signed-certificate tests remain. New positive/negative
power-of-ten boundaries verify digit widths. The strengthened allocation test
checks one allocation and exactly output-length requested bytes for zero,
positive and negative 64-byte fixtures. Its original baseline predates the prior
working-buffer optimization, so comparative savings are cumulative, not solely
attributed to this batch. Fresh verification passed 17 extractor, 20
HeaderCertAuth and 14 mTLS callback tests; final-source extractor verification
also passed. Formatting, workspace check/Clippy and seven repository guards
passed with existing warnings outside changed code. No universal CPU, physical
memory or live-handshake performance claim is made.

## Defer generic EdgionTls lookup behind frontend mTLS selection

The certificate-selection callback now performs its generic eligible EdgionTls
lookup only after the Gateway frontend-mTLS branch returns. That branch uses its
own exact-attachment lookup and never consumed the generic result. Both snapshot
captures retain their original order and location; strict rejection, attachment
identity, overlay conflicts, minimum TLS version and ordinary Layer 1/2 selection
remain unchanged. The skipped lookup only reads pinned maps and may clone a
winning Arc. Certificate/key/CA preparation and candidate sorting were also
inspected and already occur during configuration publication.

Independent plan and code reviews passed. Fresh verification passed 30 TLS store
tests and the two targeted exact-overlay/published-regex callback regressions,
formatting, workspace check/Clippy and seven repository guards. Existing warnings
are outside changed code. Source control flow establishes the skipped work; no
new test duplicating that control flow was added. No live-handshake suite or
throughput benchmark was run, and callback-wide coverage remains incomplete.

## Borrow certificate callback overlay candidates before application

The certificate callback borrows the Gateway and EdgionTls overlay options for
its initial tuple match and frontend mTLS setup. It clones the overlay Arc only
after setup succeeds, immediately before certificate application needs mutable
access to the whole context. This removes the tuple's Gateway Arc clone on both
overlay and ordinary Gateway paths, and avoids unused overlay clones when setup
fails or the tuple does not match. The context retains both owners throughout;
native trust-store installation retains its own reference. Branch precedence,
verification/identity ordering, logging and accepted ex-data clone handoff are
unchanged. No Arc-count-dependent consumers were found in the affected chain.

Independent plan and code reviews passed. Fresh verification passed 30 TLS store,
two targeted overlay and 14 mTLS callback tests, formatting, workspace check/Clippy
and seven repository guards. Existing warnings remain outside changed code.
The benefit is fewer Arc reference-count operations, not removed certificate
copies or measured heap/latency savings. No live-handshake suite was run.

## UDP selection follow-through and numeric section ownership

Reinspection at `8fd46cd66` confirmed established UDP sessions bypass route,
policy and endpoint selection. Their session Arc escapes the DashMap guard before
send awaits; retirement still checks generation identity and activity under the
existing lifecycle lock. New-session routing retains only its selected unit;
the route table returns its first pre-sorted unit and the unit already owns
cold-compiled backend and parent-reference state.

The shared L4 policy gate preserves terminal denial before endpoint selection.
`select_service_endpoint` reads its input section synchronously and retains the
resolved section and discovery snapshots. A proposed borrowed section parameter
was rejected as a performance batch after independent plan review: all five
production callers currently supply Number(NonZeroU16), not Name(Arc<str>).
The four L4 paths obtain it from the private CompiledServiceTarget constructor;
RequestMirror's private target uses the same service_section_key_from_port helper.
That helper constructs only Number. Consequently the existing input clones
perform no heap allocation or Arc reference-count operation on these paths.
No API rewrite or hypothetical named-port consumer was introduced. Revisit only
if an actual owning input or production named-section consumer changes this proof.

This is scoped source inspection, not closure of UDP's deeper selection algorithms,
publication helpers or socket dependencies. No runtime behavior was changed.
Fresh verification passed 12 Service selector tests, formatting, workspace
check/Clippy and agent-doc consistency. Existing Clippy warnings are outside this
record-only change. No UDP socket tests or throughput benchmark were run.

## Read the selection decay clock only for EWMA

Captured Service selection now reads now_units only for the EWMA policy.
RoundRobin, ConsistentHash (including keyless RR fallback) and LeastConn ignore
the timestamp argument and receive zero. The exhaustive policy match stays at
the original pre-degrade position, preserving one fresh EWMA timestamp for all
candidate comparisons. Published backend counters already initialize the
monotonic epoch during construction. Snapshot capture, health/outlier filtering,
degrade counters, selector order and error mapping remain unchanged. The owning
algorithm document now states the policy-specific clock boundary.

Independent plan and final source reviews passed. Fresh verification passed 12
Service selector, eight compiled-selector and 21 EWMA score tests, formatting,
workspace check/Clippy and seven repository guards. Existing warnings are outside
changed code. The skipped clock read is established by source control flow;
tests cover selection and estimator behavior, not a measured latency saving.
No selector API, estimator arithmetic, cache or benchmark was added.

## Borrow the connected backend until load accounting is required

The successful-connect callback's commit_attempt now borrows lb_backend while
checking its policy. Only LeastConn/EWMA clone the Arc, still before inc_conn,
and transfer it into the same owning LbCommitGuard. RoundRobin/ConsistentHash
avoid an unused Arc increment/decrement. UpstreamInfo keeps its backend owner;
missing-backend and already-committed returns, finalize/Drop balancing and
header-time latency sampling remain unchanged.

Independent plan and code reviews passed. Fresh verification passed seven attempt,
three backend-counter and eight response-sample tests, formatting, workspace
check/Clippy and seven repository guards. Existing warnings are outside changed
code. The tests retain scoring/non-scoring, idempotence, cancellation, generation
counter isolation and sampling coverage. No syntax-mirroring test was added;
the removed reference-count work is evident in the branch, not an allocation or
measured throughput improvement.

## Borrow endpoint completion state from its retained runtime snapshot

Response-header sampling already skips non-EWMA policies and reuses header_at;
per-attempt outlier classification stays before retry returns. Following that
path into the discovery coordinator found an unnecessary local endpoint Arc
clone: with_current_endpoint_locked already owns the active snapshot containing
that endpoint. It now borrows the entry through the existing completion lock,
current-snapshot reload, exact membership/pointer checks and synchronous callback.
The same helper serves health completion. Guard and snapshot release order,
policy checks, retirement freezing and outlier percent arbitration are unchanged.
The accepted residual locks and metrics costs remain outside this optimization.

Independent plan and code reviews passed. Fresh verification passed 41 coordinator
and 36 outlier-detection tests, formatting, workspace check/Clippy and seven
repository guards. Coverage includes late completion rejection, policy replacement,
retirement barriers, gauge settlement and percent-cap arbitration. Existing
warnings are outside changed code. The benefit is one fewer Arc clone/drop after
a successful endpoint lookup, not removed synchronization, heap allocation or
measured throughput. No live traffic or benchmark was run.

## Borrow the completion runtime cell from its captured generation

The same completion helper now borrows runtime_cell from its already-owned
ServiceSelectorGeneration instead of cloning the cell Arc. The generation pins
the projection and cell through both active snapshot reads, endpoint locking,
identity revalidation and synchronous application. Neither snapshot read moves;
no ArcSwap guard, publication change or new lock crosses the callback. Returned
cell owners elsewhere and cold policy publication are unchanged.

Independent plan and final source reviews passed. Fresh verification again passed
41 coordinator and 36 outlier-detection tests, formatting, workspace check/Clippy
and seven repository guards. Warnings remain outside changed code. This batch
removes one cell Arc increment/decrement after a successful Service lookup;
the earlier endpoint Arc reduction is separate. No heap, throughput or lock
removal claim is made.

## Borrow the response-cache binding policy from request state

Following terminal cache publication back to request binding, RequestState::bind
now borrows its immutable intent instead of cloning its policy Arc. The owning
state stays outside the context under an exclusive borrow throughout binding,
including the GraphQL body scan await and subsequent Vary traversal. Disjoint
field updates, cancellation cleanup, admission, key framing, epoch checks and
leases remain unchanged. Nomination still retains an owned policy; route ownership
still supports mutable context adapters. This removes one Arc increment/decrement
when an unbound eligible request reaches its intent lookup, with no allocation or
throughput claim.

Independent plan and source reviews passed. Fresh verification passed 38 cache
tests and 9 request-tail/runtime tests. One existing reload fixture test failed
and reproduced alone: see [the deferred fixture issue](verification-01-cache-policy-fixture.md).
Current source again confirms missing preparedAncestorAdmission and the unchanged
fail-closed admission chain; no baseline binary was rerun. Formatting, workspace
check/Clippy and seven repository guards passed. Clippy warnings remain outside
changed code. The known fixture failure is retained, not skipped or repaired here.

## Borrow the retained request-body replay snapshot

BodyStorage::next_chunk now borrows its retained replay_snapshot instead of
cloning its Arc on each call. The exclusive mutable storage borrow pins that
owner through disk-read awaits; only disjoint failure state is updated. Disk
jobs still own their File Arc and returned memory chunks remain owned Bytes.
Rewind, consume, snapshot selection, EOF and error precedence are unchanged.
This saves one Arc increment/decrement after a successful snapshot lookup,
including EOF checks, without removing a lock or payload copy.

Independent plan and source reviews passed. Fresh verification passed all 68
storage tests, including cancellation/re-read, truncation, retry cursors and
snapshot stability, plus formatting, workspace check/Clippy and seven repository
guards. Warnings remain outside changed code. No throughput benchmark was run.

## Request-body suffix updates for long decoded chunks

TextOperationScanner::EndsWith now replaces its retained suffix directly when
the current decoded chunk is at least as long as the pattern. Only the last
pattern-length bytes matter; short chunks keep the original deque update loop.
Byte slicing preserves comparisons even when the retained suffix begins inside
a UTF-8 scalar. Empty-pattern Stop, complete lossy decoding, nonempty Continue,
EOF processing and raw scan accounting are unchanged.

Differential tests compare against String::from_utf8_lossy(...).ends_with(...)
across every three-chunk split of empty, ASCII, multibyte and invalid UTF-8 input,
plus a wrapped short-chunk tail followed by a long replacement chunk. Retained
bytes remain bounded by pattern length; VecDeque capacity growth may differ
within the same proportional bound. This removes unnecessary prefix deque
updates, not decoding or storage reads, and claims no measured throughput gain.

Independent plan and source reviews passed. Fresh verification passed 11 text
operation and 68 storage tests, formatting, workspace check/Clippy and seven
repository guards. Existing warnings are outside changed code.

## Request-body scalar snapshot metadata

BodyStorageView::len and overwrite_body now read copied length/generation
metadata instead of acquiring a temporary snapshot Arc. A private helper keeps
mutation-first selection and initial-capture readiness under the existing read
guard, releasing it before replacement construction or publication. Overwrite
already discarded its old snapshot immediately; scan, reader and replay paths
still acquire owned snapshots. Physical caps, freeze and generation-conflict
checks retain their ordering.

A regression covers incomplete capture without publication, initial and updated
lengths, empty replacement and frozen rejection. Independent plan and source
reviews passed. Fresh verification passed 68 storage tests and the separate
late-capture/view-release freeze test, formatting, workspace check/Clippy and
seven repository guards. Warnings remain outside changed code. Each successful
metadata lookup avoids one snapshot Arc increment/decrement; no allocation,
lock-removal or throughput improvement is claimed.

## Borrowed memory chunks in request-body scans

scan_snapshot_owned now reuses the existing checked memory-segment projection
and passes borrowed local slices to synchronous scanner callbacks. The owned
immutable snapshot pins Inline and memory SourceRange bytes throughout scanning;
owned reader/replay chunks and disk scratch are unchanged. Cancellation and
budget checks, chunk accounting, Stop and finish order remain in place.

Private production constructors establish valid ranges, checked slicing narrows
them, and normalization only merges adjacent ranges of the same source. For
these admitted snapshots behavior is unchanged. A fabricated invalid internal
SourceRange can now fail full-range validation earlier than an early-stopping
scanner; identical behavior for malformed private constructions is not claimed.

The new regression checks content, backing addresses and zero scan allocations
across multiple chunks for unique Inline bytes and a nonzero-offset SourceRange.
Against the original production code it failed in the first Inline case with one
allocation; that baseline execution did not reach the second case. Both cases
pass after the change. This avoids temporary Bytes ownership and its possible
first-share allocation, not payload copying that was already absent. No overall
CPU or throughput gain is measured.

Independent plan and source reviews passed. All 67 storage tests, formatting,
workspace check/Clippy and seven repository guards passed; warnings remain
outside changed code.

## Lazy scan ownership for cached request-body SHA-256

BodyStorageView::sha256_scan now clones the scan snapshot only inside the
once-cell initializer. The outer owned snapshot already pins the cell throughout
waiting; the elected initializer still transfers an owned snapshot into scanning
before any scan await or blocking work. Ready cache hits and successful waiters
avoid one Arc increment/decrement. Failed initialization, cancellation, generation
isolation and zero-byte accounting for cached results keep their existing behavior.
No digest, body, lock, permit or memory-budget ownership changes.

Independent plan and source reviews passed. Fresh verification passed 66 storage
tests covering memory/disk scans, concurrent digest initialization, mutation,
failure and cancellation, plus formatting, workspace check/Clippy and seven
repository guards. Warnings remain outside changed code. No heap-allocation or
throughput reduction is claimed; actual scan work is unchanged.

## Response-cache storage and freshness inspection

Inspected capture admission, fixed-capacity body writes, entry decode, header
contract checks, monotonic freshness, conditional eviction and reader leases.
Repeated metadata and header validation enforce corruption and final transport
contracts; no validation was removed. Canonical LRU ownership, epoch fencing
and full-capacity accounting remain unchanged. Eager arena initialization is
recorded as a separate decision above because avoiding writes to unused capacity
changes initialized-length invariants and memory-touch timing. This read-only
inspection does not establish measured throughput or complete canonical LRU
algorithm coverage.

This documentation-only batch passed formatting, workspace check/Clippy,
agent-doc validation and diff whitespace checks. No tests or benchmarks were run.

## Completed request-body capture ownership

CaptureStorage now records completion with a payload-free Finished variant.
Successful publication moves the memory bytes or file handle into InitialBody,
which permanently retains the initial source through BodyStorageView. Storage,
retained views and snapshots therefore keep their existing source lifetimes
without a second completed-capture handle. Publication still precedes completion;
failed publication leaves the capture poisoned. Spill jobs, replay, mutations,
limits and cancellation ownership are unchanged.

The regression covers empty and nonempty capture with memory and spill limits,
repeated finish, rejection of empty and nonempty late writes, replay and reading
through a retained view after storage is dropped. The disk-test helper accesses
the same backing file through the published source. All 69 storage tests,
formatting, workspace check/Clippy and seven repository guards passed. Warnings
remain outside changed code. This removes a duplicate Bytes clone or File Arc
clone at successful completion; no payload-copy, byte-budget or measured
throughput reduction is claimed.

## Request-body rewind borrows the existing view owner

The canonical private snapshot selector now belongs to BodyStorageViewInner;
BodyStorageView delegates to it, and rewind calls it through its existing owner.
This removes the temporary view's inner Arc clone/drop. Mutation-first selection,
generation pairing under the read lock, the owned snapshot Arc, error precedence
and cursor resets are unchanged. No lock, snapshot lifetime, body byte copy or
allocation is removed, and no throughput improvement is measured.

Independent plan and source reviews passed. All 69 storage tests passed,
including snapshot isolation across later mutations, retry cursor resets and
disk cancellation/failure coverage. Formatting, workspace check/Clippy and seven
repository guards passed; warnings remain outside changed code.

## Request-body clear retirement inspection

A metadata-only clear candidate was rejected during independent plan review:
its named snapshot local currently pins a prior mutated generation through
publication. Removing it can move last-owner segment/Bytes destruction into the
mutation write lock. The new retirement decision records this distinction and
a possible separately reviewed publisher-level design. Runtime code is unchanged;
byte-content tests alone would not prove equivalent retirement timing.

This documentation-only batch passed formatting, workspace check/Clippy,
agent-doc validation and diff hygiene. Independent evidence review passed.
No runtime tests or benchmarks were run; warnings remain outside changed files.

## Request-body append segment capacity

Append now reserves space for the existing segments plus its one new segment
before cloning them, matching prepend's existing capacity discipline. The prior
clone-then-push grew the temporary vector. Normalization still creates the final
vector; the snapshot local remains alive through publication, preserving old
snapshot retirement timing. Ordering, segment/physical caps, frozen-state and
generation checks are unchanged.

A synchronous allocation comparison with prepend covers captured disk and
memory sources and checks both replay orders. Against unchanged production code,
it failed in the first disk case: append requested four allocations (352 bytes)
versus prepend's three (256 bytes); that baseline did not reach the memory case.
Both cases pass after the change. These are allocator requests in the test,
not resident memory or measured throughput.

Independent plan and source reviews passed. All 70 storage tests, formatting,
workspace check/Clippy and seven repository guards passed; warnings remain
outside changed code.

## WAF structured capture adapter inspection

Inspected WafPlugin request admission through json_capture::inspect_structured,
including fresh capture, cached snapshots, native policy windows, completion,
raw-source demand and transaction restoration. Plugin construction compiles the
generation and resolves configured limits once. Per-request processor preflight
follows phase-one controls and cannot be frozen to the initial configuration
without changing request-scoped control semantics.

The adapter feeds borrowed current slices into inline JSON/XML sessions. It
bounds supplied bytes before feed and keeps canonical body capture after a
latched parser failure so transport errors retain priority. Cached length
admission precedes reader I/O; captured/available parity and actual completion
precede publication. Raw source materialization remains conditional on demand,
with its existing storage-error priority. No second body owner was introduced.

Usage refresh reads maintained parser/collector counters and the transaction's
scalar retained-byte accounting; it does not traverse the captured body or
recompute retained allocations. Delta metrics and the scoped charge preserve
release on cancellation and errors. This audit selected no runtime change.
Broader raw/structured scan sharing already has a conditional design in
`tasks/todo/waf-shared-request-body-inspection.md`; it requires workload evidence
and ordered-effect proofs, so no duplicate task was created.

Fresh verification passed all 15 json_capture adapter tests using in-process
duplex streams and temporary storage: fresh/cached JSON/XML, contiguous fallback,
limits, cancellation, error priority, raw visibility and actual-EOF windows.
Formatting, workspace check/Clippy and agent-doc validation also passed. Existing
warnings remain outside this documentation-only change. This establishes adapter
coverage, not complete WAF engine review, integration or throughput results.

## Borrowed WAF installation identity

WafPlugin::restore_transaction now borrows its owner token through the selected
WAF capability and adapter into WafRequestContext::install. Installation only
compares pointer identity; it does not retain the supplied token. SharedWaf and
the selected context retain their existing owners throughout the synchronous
call. Transaction transfer remains by value: rejected offers are destroyed,
resident transactions survive, and successful installation refreshes accounting.
Generation pinning, phase claims and fixed conflict responses are unchanged.

This removes one owner Arc increment/decrement per restoration, not a payload
copy or allocation. Response-header views remain borrowed until transaction-owned
state is built; response completion still consumes the taken transaction. No
response-body aggregation or changed WAF phase scheduling is introduced.

Independent plan and source reviews passed. Fresh tests passed 50 WAF plugin
cases, one capability borrow case, one real-adapter resident-preservation case
and two native stream cases. Formatting, workspace check/Clippy and seven guards
passed; warnings remain outside changed code. No throughput claim is made.

## Borrow WAF candidates until election completes

select_waf_owner now retains a borrowed candidate while traversing the captured
runtime lists, then clones only the final winner's owner. Immutable input runtime
Arcs pin every candidate throughout this synchronous call. Scope precedence,
higher-priority replacement, first-declaration ties and dangling-reference skips
retain their original comparisons and order. Final WafSelection remains owned.
This avoids intermediate owner Arc increments/decrements when winners are
replaced; a single winning candidate still requires one final clone.

A new regression combines priority replacement, a dangling slot and an equal
priority successor, then drops all input runtimes and verifies the winning owner
survives until selection is dropped. Existing scope and empty-list cases remain.
Independent plan and source reviews passed. All 11 resolver tests, formatting,
workspace check/Clippy and seven guards passed; warnings remain outside changed
code. No allocation or measured throughput reduction is claimed.

## Single-pass WAF finding summary

WafAccessLog::record_phase now counts loggable findings while building the bounded
projection, removing its preceding count-only traversal. Counting still includes
findings beyond the 16-entry access-log cap; nolog/noauditlog filtering, runtime
omissions, ordering, first intervention and outcome precedence are unchanged.
The phase slot is written after the synchronous loop under exclusive access;
there are no callbacks or recoverable intermediate errors exposing that order.
Existing allocation and canonical terminal-only serialization remain unchanged.

The bounded-summary test now checks 21 total/evaluated findings versus 16 retained
entries and five omissions, then verifies a repeated phase leaves the serialized
summary unchanged. Mixed logging flags also assert the filtered phase count.
Independent plan and source reviews passed. Eight WAF context and 50 WAF plugin
tests passed, together with formatting, workspace check/Clippy and seven guards.
Warnings remain outside changed code. This removes one traversal; no allocation
or measured throughput reduction is claimed.

## WAF raw-stream reachability and prefix preparation

Traced the direct raw-body callback through WafRequestContext into the leaf
transaction's stream preparation/chunk APIs. In the current tree, production
WafPlugin does not call start_request_body_stream or the capability's streaming
enabler. The enabler/adapter/context bridge exists, but actual activation calls
are in tests. This is not a missing-activation fix: prefix admission deliberately
replaced the old post-dispatch raw handler so its verdict precedes upstream
exposure, as recorded in the WAF prefix-before-dispatch regression guard.
No optimization or activation of this dormant lane was selected.

Followed the active execute_request_body_prefix path instead. Its apparent
validation call already reads CompiledGeneration.prefix_validation, populated by
check_request_body_prefix during final compilation. Complete-body dependency is
also cached then. Per-request phase, health, completion, window and physical-cap
checks remain runtime invariants. Prefix bytes are borrowed for ordered phase-two
evaluation; transforming or scanning them still has request-dependent work and
limits. This audit does not establish complete operator/parser coverage.

Fresh verification passed 50 Gateway WAF plugin tests, three canonical prefix
adapter tests and 24 leaf-crate tests matching prefix_. The leaf filter retained
one existing ignored release timing witness; it was not run and supplies no
performance evidence. Formatting and workspace check/Clippy passed. Agent-doc
validation and diff hygiene passed after this documentation update. No runtime
code, protocol policy, benchmark or deferred-decision scope changed.

## ExtProc streamed exchange ownership inspection

Followed ExtProcStreamedBodyHandler through AttachedExtProc and the client stream
exchange. The request-scoped owner moves its non-cloneable stream out under a
short state lock before awaiting, leaving Failed as the cancellation sentinel.
The live stream owns its target permit and outbound channel; the client clone
enters the lazy opening future. Borrowing these across their owner boundary or
holding a synchronous guard through exchange would change cancellation or
lifetime semantics. Last-phase half-close and matching-verdict/clean-EOF checks
remain separate required protocol steps, not redundant waits.

ProcessingMode clones copy five enum fields, with no heap-backed payload. The
allowed override list is already shared from construction. Per-request override
parsing and reduction/membership checks preserve dynamic processor decisions;
they cannot be replaced by one immutable effective mode. Fail-open deactivation
releases the stream, abandonment preserves terminal ownership rules, and valid
body mutation moves its returned Vec into Bytes. The outgoing body copy remains
covered by the existing protobuf representation decision.

No runtime optimization was selected in this audit. Fresh network-free tests
passed 18 scripted streamed-handler cases and 10 attached-owner/mode cases;
the latter construct lazy clients but do not exchange with a server. Formatting,
workspace check/Clippy, agent-doc validation and diff hygiene passed. Existing
warnings remain outside changed files. This is scoped ownership/protocol source
inspection and unit evidence, not fresh gRPC interoperability or complete
transport dependency coverage.

## ExtProc mutated trailer size validation

Removed the duplicate size scan at the end of private apply_mutation. Its sole
caller immediately invokes validate_trailer_map, whose first operation is the
same checked_map_size, followed by forbidden-name and gRPC-status validation.
There is no intervening mutation or await. Candidate ownership, representability
checks and atomic publication remain unchanged. The gRPC classification getter
now also runs for an oversized mutation; both production implementations only
read context booleans and have no side effects.

Strengthened the existing cap regression to cover mutated byte and field overflow
under both fail-open and fail-close policies. Each case preserves the original
map, returns the expected policy result, records bad-verdict and completes the
processor. All 17 trailer tests passed, along with formatting, workspace check
and Clippy; warnings remain outside changed code. This removes one repeated
traversal on successful mutation, with no allocation or throughput claim.

## ExtProc request-header projection capacity

Request-header projection now reserves its output vector once for every ordinary
field line, the two mandatory pseudo-headers, and the optional authority field.
HeaderMap.len includes repeated values. Method and URI accessors still run before
the map accessor; pseudo-header order, first Host selection, ordinary Host
projection, per-name duplicate order and raw value bytes remain unchanged.
The three call sites in streamed/bodyless and buffered request exchanges retain
their owned protobuf message across transport awaits.

The change avoids output-vector growth; it does not remove the per-field name or
byte copies or change protobuf representation. Existing projection tests cover
pseudo-headers, duplicate and opaque values, empty maps and current URI/query
semantics. No throughput measurement is claimed.

Fresh verification passed all 18 ExtProc mutation tests, formatting, workspace
check/Clippy and seven repository guards. Tests use mocks or in-process duplex
sessions, with no live network exchange. Post-update agent-doc validation and
diff hygiene passed; existing warnings remain outside changed code.

## ExtProc response-header projection bridge

Response projection now borrows the canonical live response HeaderMap and builds
owned protobuf fields directly. This removes the intermediate vector of owned
byte pairs and reserves the destination for all field lines plus optional status.
It retains the same per-field name/value copies, status-first projection, raw
opaque bytes and per-name duplicate ordering. The only production adapter serves
the same bound map through both accessors; both are empty before response binding.
Typed HeaderNames make the former byte-name UTF-8 rejection branch unnecessary.

The protobuf message still owns its fields before the response-header exchange
await. The existing opaque round-trip test now removes the source fields before
serialization and verifies the projected duplicate/empty/opaque values survive;
it also checks the unbound stage. Set-Cookie projection and the channel-based
buffered exchange fixtures use the borrowed map seam. One existing live-server
test has the same mock migration but is compiled only, not executed in this batch.
The two relevant knowledge pages now distinguish message ownership from the
obsolete intermediate-vector choice. No throughput improvement is claimed.

Fresh verification passed 18 mutation tests and nine scripted buffered-exchange
tests, formatting, workspace check/Clippy and all seven repository guards.
Post-update agent-doc validation and diff hygiene passed. Existing warnings
remain outside changed code.

## ExtProc local-reply value transfer assessment

Traced owned local-reply mutations through typed decoding, pending operations and
response-header delivery. Direct owned HeaderValue conversion can avoid copying,
but a locked-dependency probe demonstrated a one-byte decoded raw value retaining
65,536 bytes of Vec capacity after repeated singular protobuf fields. The owned
conversion retains that storage; current borrowed conversion copies final bytes.
The new response-03 decision records this lifetime tradeoff and a selective
exact-capacity transfer alternative. No runtime code or protocol policy changed;
this synthetic dependency witness is not fresh gateway interoperability evidence.

Fresh verification passed the dependency probe, 18 existing mutation tests,
formatting, workspace check/Clippy, agent-doc validation and diff hygiene.
Existing Clippy warnings remain outside this documentation-only change.

## Canonical SSE leaf serialization

The Anthropic streaming converter's canonical record writer now serializes JSON
keys, semantic strings and other scalar values directly into its existing
ValueWriter. An io::Write adapter delegates to the same checked append operation;
there is no parallel serializer or output buffer. This removes each leaf's
intermediate serde_json Vec while preserving formatting, inclusive byte limits,
semantic token offsets and publication only after the complete record succeeds.
Partial internal writes on failure are discarded with the local writer.

Streaming writes may change the original output Vec's growth and backing
capacity. Canonical batches check byte lengths and metadata, while RawWindow
admission derives incoming resident charge from length and normalizes retained
backing. The converter copies identical encoded bytes into its separate bounded
output. These paths do not make admission depend on the original backing capacity;
the unary conversion budget uses a separate serializer. This is not a claim of
identical capacities, lower peak memory for every input or measured throughput.

New tests compare full encoded bytes, reject every smaller output cap, accept
the exact cap, and decode recorded content/reasoning/tool-argument token ranges
including escapes and multibyte text. A preallocated scalar-output test isolates
serialization and verifies zero temporary allocations, excluding fixture creation
and destination growth.

Fresh verification passed 11 canonical SSE tests and 26 Anthropic response tests,
formatting, workspace check/Clippy and seven repository guards. Post-update
agent-doc validation and diff hygiene passed. Existing warnings remain outside
changed code. These are network-free unit checks, not a live provider benchmark.

## Canonical batch raw fallback materialization

CanonicalRecordBatch.into_raw_bytes now copies fallback bytes through SliceChain's
existing borrowed full-range visitor. Sliced fallback no longer constructs a
temporary handle vector or clones Bytes; coalesced fallback no longer copies its
arena into an intermediate Bytes before copying into the final output. Annotated
record consumption and the initial final-output allocation/capacity check remain
unchanged. The synchronous full range is valid by construction and no borrow
escapes. The final owned copy remains, avoiding retention of coalescing spare
capacity in the returned output.

The three consumers retain their policies: disabled semantic guard forwarding,
quota-terminal current-chunk reclamation, and Anthropic terminal output reclamation.
Tests build valid empty, annotated, sliced-fallback and coalesced-fallback batches,
compare ordered bytes, and isolate final materialization allocations from fixture
construction. Empty output allocates zero times; each tested nonempty shape
allocates only its final buffer. No whole-callback allocation or throughput claim
is made; fallback admission, coalescing and into_records behavior are unchanged.

Fresh verification passed 112 tests: 12 canonical SSE, 24 semantic response,
50 usage and 26 Anthropic response tests. Formatting, workspace check/Clippy,
seven repository guards, post-update agent-doc validation and diff hygiene passed.
Existing warnings remain outside changed code. No live transport benchmark ran.

## Anthropic fixed error body preparation

The response converter's closed FixedError set now stores six pre-serialized JSON
bodies as static bytes. The service-unavailable body is still shared by the
translation-unavailable variant with its distinct diagnostic flag. Both existing
rendering callers create Bytes from the static body instead of building an owned
JSON tree, serializing a Vec and transferring it. Status selection, single
emission, sealing, error recording and provider-detail suppression are unchanged.

The literals preserve the old compact code/message/type field order. The removed
serialization fallback cannot be reached by the former fixed string-only values;
there is no runtime serialization left in this renderer. Successful unary output
still uses its separate reservation-bearing owner. Other OpenAI error rendering
and the SSE terminal error literal remain unchanged.

A seven-variant regression compares exact bytes against the former JSON-building
expression, verifies status and diagnostic flags, and measures zero allocations
for rendering alone. Expected-body construction is outside the measurement.
This is fixed-body preparation evidence, not a throughput or whole-callback
allocation claim.

Fresh verification passed 63 tests: 27 Anthropic response, 12 canonical SSE and
24 semantic response tests. Formatting, workspace check/Clippy, seven repository
guards, post-update agent-doc validation and diff hygiene passed. Existing
warnings remain outside changed code; no live provider benchmark was run.

## Anthropic Retry-After snapshot gating

Response normalization now extracts Retry-After only for an original 429, the
only upstream status mapped to the fixed rate-limit response that reinserts it.
This skips discarded parsing and String construction on streaming non-429
responses. The 429 path retains first-value selection, text validation, original
whitespace, unary's 64-byte cap and owned snapshot before provider headers clear.
The early unavailable branch, unary header observation, status mapping and
subsequent response normalization remain unchanged.

New cases cover streaming and unary paths across 200/429/503 with numeric and
date values, surrounding whitespace, 64/65-byte values, invalid/empty/opaque
values and an invalid first duplicate. An equal-header-count allocation check
compares ordinary discarded metadata with Retry-After for streaming 200 and 503;
it verifies there is no additional normalization allocation for the unused field.
Fixture creation is excluded; the whole normalization callback still allocates.
No throughput improvement or change to Retry-After policy is claimed.

Fresh verification passed 29 Anthropic response and 12 canonical SSE tests,
formatting, workspace check/Clippy and seven repository guards. Post-update
agent-doc validation and diff hygiene passed. Existing warnings remain outside
changed code. These are network-free unit checks, not a live provider run.

## Anthropic original header validation reuse

Response normalization now classifies the original status once and shares the
result with unary header observation. A unary 200 no longer repeats the same
encoding, media-type and Content-Length validation; unary non-200 responses skip
success-header validation whose result could not affect their observation flags.
The validator is read-only, and observe_header records invalid only for raw
success. Error mappings, usage markers, Retry-After handling and normalization
retain their existing outcomes. The early unavailable path is unchanged.

UnaryHead::plan still independently validates final metadata after response
plugins; its input can differ from the original provider header. No validation
boundary, byte cap, body ownership or accounting policy was removed. This is a
reduction in redundant validation work, not a measured throughput claim.

Fresh verification passed all 56 chat-to-Anthropic unit tests, including response
normalization and final-head revalidation. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No network test or live provider benchmark was run.

## Optional canonical event metadata for Anthropic streaming

Anthropic push_json_event now requests semantic metadata only when its current
callback owns an active canonical batch. The existing canonical writer uses
WriteContext::Root for capture and Other otherwise; Other stays Other throughout
the recursive walk. Both paths retain identical JSON serialization, framing and
byte-limit checks. Disabled capture allocates neither semantic fragments/ranges
nor the discarded WindowRecord/raw-slice table. Record construction is lazy.
The active path retains the existing records and canonical batch admission.

The writer regression compares both modes against exact serde JSON bytes,
rejects every below-output byte limit in both modes, and verifies lower allocation
count with capture disabled for content, reasoning and tool-argument fields.
Fixture construction is excluded. Existing semantic range assertions remain.
Usage character counting, quota, terminal ownership and body storage are unchanged;
this establishes a local allocation reduction, not live throughput improvement.

The adjacent decoder replacement was also inspected: its temporary default
contains empty Vec-backed state and scalar fields, not preallocated event storage.
The original decoder is restored after synchronous feed. No decoder change is
justified by an assumed per-callback constructor allocation.

Fresh verification passed 92 tests: 12 canonical SSE, 56 chat-to-Anthropic and
24 semantic response tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live provider or transport benchmark was run.

## Terminal quota computation eligibility

The terminal AI quota finalizer now borrows the request and its quota owner
before computing authoritative totals and the overflow snapshot. Requests without
a quota return before these discarded field/atomic reads and checked arithmetic.
The skipped helpers are observational; diagnostic claiming remains in usage
finalization. The quota-present path retains its computation, await, result and
error handling order. Top-level attempt, usage and quota finalization order is
unchanged, including unknown usage and overflow policy.

The existing admission-bypass status regression now runs quota finalization too
and verifies that an absent quota preserves the summary's fail_open status.
No allocation or throughput improvement is claimed for this small read reduction.

Fresh verification passed 63 network-free tests: 13 accounting finalizer,
23 attempt runtime and 27 quota tests. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain
outside changed code. No live Redis or provider benchmark was run.

## Attempt log byte admission without a serialized buffer

Attempt ledger admission now passes the same Serialize implementation to a
private bounded counting writer instead of materializing a temporary JSON Vec
only to inspect its length. The writer subtracts emitted bytes from the existing
8 KiB allowance and rejects an excess write. Exact-cap output remains accepted;
serialization failure or overflow retains the existing pop/truncated outcome.
GenAiAttempt serialization has no side effects, so early excess rejection changes
no published result. Usage, metric and adaptive updates remain before admission.

The existing SSE and DSL counters are private and have different unbounded or
saturating contracts; this owner-local size gate does not expose those internals
or create another serializer. It retains no scratch buffer across attempts.
Tests compare escaped/UTF-8 serialization length, reject every smaller allowance,
accept the exact length with zero counting allocations, and verify the former
Vec serialization allocates. An oversized synthetic ledger entry exercises the
independent byte cap beyond normal field bounds: prior entries survive while
charged and metric totals still include the omitted entry. This is a local
allocation result, not a throughput benchmark or removal of the byte limit.

Fresh verification passed 37 tests: 24 attempt runtime and 13 accounting finalizer
tests. Formatting, workspace check/Clippy, seven repository guards and diff
hygiene passed. Existing warnings remain outside changed code. No live transport,
Redis or throughput benchmark was run.

## Borrowed reported-usage snapshot projection

AiUsageShared::reported_usage now copies its four-u64 value under a temporary
ArcSwap guard instead of acquiring an owned Arc with load_full and immediately
dropping it. The locked arc-swap 1.9.2 implementation promotes load to an owned
pointer in load_full; the borrowed form avoids that promotion on the fast path,
while retaining the dependency's safe fallback when cheap guard slots are busy.
This is not a guarantee of zero atomic operations under all concurrent schedules.

The guard protects one immutable published generation for the complete Copy
projection and does not escape or cross an await. None, publication high-water
marks, accounting precedence and separate accounting_snapshot reads are unchanged.
Existing usage, finalizer and attempt tests cover missing/reported values,
observation-off, overflow, pricing and settlement; no implementation-mirroring
test or dependency-specific refcount instrumentation was added.

Fresh verification passed 87 tests: 50 usage, 13 accounting finalizer and
24 attempt runtime tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No concurrency stress benchmark or throughput measurement was run.

## Borrowed adaptive telemetry during attempt ledger commit

Known failure, ambiguous failure, backend timeout and final completion now pass
a borrowed adaptive telemetry state into synchronous commit_ledger instead of
cloning its Arc. The consumed active attempt remains the owner throughout the
call, including after its separate reservation field moves. Original ownership
and field drop timing remain; the borrow ends before asynchronous pending
settlement. No telemetry lookup or publication behavior changes.

Charged-token and successful-TTFT recording keep their original positions before
log byte admission. Neither recorder retains the reference or depends on Arc
counts. Missing telemetry, undispatched completion, accounting overflow and
success eligibility retain their branches. Existing settlement/adaptive and
finalizer tests are used for this mechanical ownership adjustment. This removes
four possible temporary Arc acquisitions, not the request-owned telemetry handle;
no throughput or whole-request allocation claim is made.

Fresh verification passed 59 tests: 24 attempt runtime, 13 accounting finalizer
and 22 adaptive routing tests. Formatting, workspace check/Clippy, seven
repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No live provider/Redis test or throughput benchmark was run.

## Quota internal UUID stack encoding

Credential reservation and identity/model quota admission now encode their
internal UUID into the uuid crate's stack buffer instead of constructing a
temporary String. Locked uuid 1.24.0 Simple Display uses the same lowercase
encoding, preserving the 32-byte unhyphenated representation. Random generation
stays after Redis-client health checks; each invocation still generates one ID,
shared by every target in that quota admission.

Both canonical Redis key builders synchronously hash the ID and return owned
key strings. The buffer is not retained by a lease or Redis future. Window IDs,
cluster-slot hashing, request-marker isolation, TTLs, admission and failure
policies are unchanged. This removes the intermediate formatting String in each
path; key storage and RNG work remain. Existing quota, attempt and shared Redis
core tests provide verification without duplicating the UUID library tests.

Fresh verification passed 80 network-free tests: 24 attempt runtime, 27 quota
and 29 shared Redis rate-limiter tests. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain
outside changed code. No live Redis run or throughput benchmark was performed.

## Quota window millisecond precomputation

CompiledLimit now stores the private u128 window_ms value computed once from
parsed Duration with the same as_millis().max(1) normalization. Target building
reads that value instead of repeating conversion and normalization for every
limit on every admission. Clock capture, division/remainder, u128 identity bytes,
window ID conversion and retention/retry durations are unchanged.

The existing bucket-boundary regression now obtains its limits through the real
quota compiler, verifying that 1m still rotates at 60,000 milliseconds and keeps
the one-millisecond remainder before the boundary. Other scope, identity and
reorder tests keep their original inputs with explicit millisecond fields.
This is configuration-only precomputation, not a new runtime policy, storage-size
claim or measured throughput improvement.

Fresh verification passed 56 network-free tests: 27 quota and 29 shared Redis
rate-limiter tests. Formatting, workspace check/Clippy, seven repository guards
and diff hygiene passed. Existing warnings remain outside changed code.
No live Redis test or throughput benchmark was run.


## Redis quota and reservation integer arguments

Cumulative quota admission/update and credential reservation now pass validated
i64 arguments to the same generic script executor, extending the ordinary
rate-limit argument optimization. This removes seven intermediate decimal Strings
from each quota invocation and five from each credential-reservation invocation.
Every u64 conversion follows its existing Lua exact-integer validation at
2^53 - 1, including limits, TTLs, revision and cumulative/credential amounts.
Absent optional credential limits remain zero; operation codes and final flags
retain their existing signed integer values.

Locked Fred 10.1.0 uses integer-as-bulk/blob encoding in both RESP command codecs;
redis-protocol 6.0.0 writes the same decimal argument bytes as the previous string
values. Lua bodies, keys, argument order, result parsing and exact NOSCRIPT-only
fallback are unchanged. Container cloning and command buffers still allocate;
this is not an allocation-free Redis operation or a throughput claim. Existing
boundary, TTL, quota and reservation tests cover the retained policy checks.

Fresh verification passed 80 network-free tests: 29 shared Redis core, 27 quota
and 24 attempt runtime tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live Redis, Cluster/failover or throughput test was run for this batch;
historical live-test results above are not fresh verification of this change.


## Quota worker ownership and future-layout audit

Inspected the complete quota accumulator/worker lifecycle and its Redis sink.
Context, worker and response processor retain separate accumulator owners;
notifications carry no accounting payload, so monotonic atomic watermarks remain
the source of truth. Attempted high-water is recorded before await to avoid
refunding uncertain applies. Final retry deadlines, lease refresh, request-drop
abort, timeout abort and completed-worker joining retain independent purposes.
No clone was removed from these cross-task owners.

The per-flush boxed future is a candidate, but generic direct dispatch changes
the persistent worker layout. Allocation and idle-residency consequences need
measurement before it qualifies as impact-free. The decision and acceptance
criteria are in [quota-01](quota-01-worker-future-representation.md). No runtime,
trait, test or parallel-target behavior changed in this audit.

Fresh verification passed 77 network-free tests: 27 quota and 50 usage tests.
Formatting, workspace check/Clippy, agent-document validation and diff hygiene
passed. Existing warnings remain outside this documentation-only change.
No production future-size, allocation, CPU or live Redis measurement was run;
the representation tradeoff remains open.
