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
| `early_request_filter` | Direct body and request-ID resolution inspected. Arc handles support mutable context dispatch; do not blindly replace with references. Publication-time list resolution deferred below. Trace-ID parsing/generation/reconciliation and direct preflight runtime inspected below; deeper source adapters remain open. |
| `request_filter` | Temporary metadata path copy removed in `4a976e022`. XFF snapshot preserves bounded aggregate size and owned log state; RealIp extractor borrows tokens and traverses in reverse. TLS metadata copies feed request-owned state. Routing, plugin dispatch and deeper RealIp policy dependencies remain open. |
| Upstream selection, connect, upstream request | `connected_to_upstream` direct body inspected: borrowed SAN digest, one timestamp, owned connection-establishment log. `upstream_request_filter` direct body and deferred credential nomination guard traced to shared Connection parser. Direct upstream deadline/backoff, concurrency admission and authority orchestration inspected; admission borrows its Service key below. Direct HTTP selection, sticky retry/fallback, timeout folding, policy capture, DirectEndpoint/ExternalJump and URI finalization inspected. ExternalJump borrows its domain below. Service peer dispatch and shared TLS metadata projection inspected below. Upstream request credential guards, deferred mutation ownership, request-ID reconciliation and transcoder attempt header validation inspected below; deeper endpoint algorithms, body conversion and credential publication remain open. |
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
- [Backend TLS verification failures](verification-03-backend-tls-tests.md)
- [Prebuilt read-only response-body dispatch](body-01-prebuild-readonly-dispatch.md)
- [ExtProc protobuf body representation](body-02-extproc-protobuf-body-storage.md)
- [Direct TLS listener log serialization](tls-01-direct-listener-log-serialization.md)
- [Pending response-header execution ownership](response-01-pending-header-ownership.md)
- [ResponseRewrite rename value fidelity](response-02-rename-value-fidelity.md)
- [Service selected-slot lookup cost](backend-01-selected-slot-lookup.md)
- [JWT claim condition evaluation ownership](condition-01-jwt-claim-evaluation.md)
- [GeoIP record decoding and projection](geoip-01-record-decoding.md)
- [Redis compiled-policy validation ownership](redis-01-compiled-policy-validation.md)
- [HMAC signed-header membership ownership](hmac-01-signed-header-membership.md)
- [Preinitialized HMAC key state](hmac-02-preinitialized-mac-state.md)
- [Raw URI fragment serialization](log-01-raw-uri-fragment-serialization.md)
- [Wasm header snapshot and replay ownership](wasm-01-header-snapshot-ownership.md)
- [DSL string consumer ownership](dsl-01-string-consumer-ownership.md)
- [ProxyRewrite staged template compilation](rewrite-01-staged-template-compilation.md)

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
reservation callers retain their String arguments through the generic executor.

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
