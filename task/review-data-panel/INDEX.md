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

- [Callback-local compression eligibility reuse](compression-04-response-eligibility-reuse.md)

- [Prepared response-cache Content-Length](cache-04-prepared-content-length.md)

- [Inactive downstream request-body module dispatch](context-02-request-body-module-dispatch.md)

- [Borrowed OIDC session-cookie serialization](oidc-07-borrowed-session-cookie-projection.md)

- [Incremental semantic window statistics](sse-01-incremental-window-statistics.md)

- [Guardrail decision snapshot encoding](guardrail-02-decision-snapshot-encoding.md)

- [Webhook extraction path preparation](webhook-02-extraction-path-preparation.md)

- [Webhook selective JSON projection](webhook-01-selective-json-projection.md)

- [CtxSet extraction and trim projection ownership](ctx-02-extract-trim-projection.md)

- [RegionRoute prepared endpoint projection](region-02-prepared-endpoint-projection.md)

- [Lazy HMAC missing-header diagnostics](hmac-04-lazy-missing-header-diagnostic.md)

- [JSON Patch execution ownership](rewrite-02-json-patch-execution-ownership.md)

- [Lazy dye outcome value formatting](dye-02-lazy-outcome-value-formatting.md)

- [Endpoint probe forwarded-header preparation](endpoint-04-forwarded-header-preparation.md)

- [Endpoint probe response text ownership](endpoint-03-response-text-buffer-ownership.md)

- [Claim projection path preparation](auth-03-claim-path-preparation.md)

- [HeaderCertAuth connection projection ownership](tls-04-header-cert-connection-projection.md)

- [JWE plaintext forwarding ownership](jwe-01-plaintext-header-ownership.md)

- [LDAP bind DN template preparation](ldap-01-bind-dn-template-preparation.md)

- [Generation-owned OIDC decoding keys](oidc-06-prepared-jwks-keys.md)

- [OIDC introspection claims ownership](oidc-05-introspection-claims-ownership.md)

- [OIDC cookie attribute preparation](oidc-04-cookie-attribute-formatting.md)

- [OIDC required-scope membership storage](oidc-03-required-scope-membership.md)

- [OIDC cookie Base64 output construction](oidc-02-cookie-base64-output.md)

- [Shared OIDC claims serialization](oidc-01-shared-claims-serialization.md)

- [Borrowed Connection inspection for credential injection](credential-01-connection-inspection-interface.md)

- [Selective request-guard refusal facts](ai-guard-01-refusal-facts-parser.md)

- [Validation-only raw JSON traversal](json-02-validation-only-raw-json.md)

- [Bounded endpoint probe projection](endpoint-02-bounded-endpoint-projection.md)

- [Single-line tracestate ownership](trace-01-single-line-tracestate-ownership.md)

- [Plugin execution state layout baseline failure](verification-04-plugin-state-layout.md)

- [Shared relay idle-reset coalescing](relay-01-idle-reset-coalescing.md)

- [ASCII character-count fast path](usage-01-ascii-character-count.md)

- [Prepared Service endpoint eligibility lookup](backend-02-runtime-eligibility-lookup.md)

- [Backend SAN inspection without full display projection](tls-05-backend-san-inspection.md)

- [Attached ExtProc summary rendering](log-05-attached-summary-rendering.md)

- [Typed gRPC terminal status evidence](grpc-08-typed-terminal-status-evidence.md)

- [Listener specificity lookup indexing](routing-02-listener-specificity-index.md)

- [ClientHello inspection buffer capacity](tls-04-clienthello-buffer-capacity.md)

- [Structured L4 peer-address handoff](l4-01-structured-peer-address.md)

- [HeaderCertAuth CA public-key preparation](tls-03-header-ca-public-key-preparation.md)

- [Local limiter initialization and window ownership](rate-limit-01-local-estimator-initialization.md)

- [Gateway-owned CORS Max-Age preparation](cors-01-derived-max-age-preparation.md)

- [Dynamic fallback metric label capacity](metrics-01-fallback-label-capacity.md)

- [Boundary-local response-cache Vary validation reuse](cache-03-boundary-local-vary-validation.md)

- [Terminal gRPC message header storage](grpc-07-terminal-message-header-storage.md)

- [Server header replacement without prior removal](response-04-server-header-replacement.md)

- [Prepared authentication challenge generation](auth-02-prepared-challenge-generation.md)

- [Query and Cookie stripping survivor storage](auth-01-strip-survivor-storage.md)

- [Prepared session-affinity token hash](lb-03-prepared-session-token-hash.md)

- [Rejected circuit-breaker permit owner copy](lb-02-rejected-permit-owner-copy.md)

- [gRPC timeout header storage](grpc-06-timeout-header-storage.md)

- [Transcoder terminal logging owner borrow](grpc-05-terminal-log-owner-borrow.md)

- [gRPC-Web trailer frame buffer ownership](grpc-web-01-trailer-frame-buffer.md)

- [Compression algorithm preference storage](compression-01-accept-encoding-storage.md)

- [Empty compression output buffer retention](compression-02-empty-output-buffer-retention.md)

- [Zstd exclusive encoder access](compression-03-zstd-exclusive-encoder-access.md)

- [Request dye name ownership](dye-01-request-name-ownership.md)

- [Elasticsearch header reuse across flush and retries](log-04-es-index-header-reuse.md)

- [Generation-owned OTLP SDK logger reuse](log-03-otlp-logger-generation-cache.md)

- [Empty-role terminal finalizer locks](body-06-empty-terminal-role-locks.md)

- [Response-cache Age header construction](cache-02-age-header-construction.md)

- [CtxSet regex replacement result storage](ctx-01-regex-replacement-storage.md)

- [Template JSON escape run batching](template-01-json-run-batching.md)

- [Outbound URL serialization ownership](outbound-02-url-serialization-storage.md)

- [gRPC single-record callback output](grpc-04-single-record-output.md)

- [Prepared gRPC wire-admission schema](grpc-03-prepared-wire-schema.md)

- [gRPC integer normalization storage](grpc-02-integer-normalization-storage.md)

- [gRPC query preflight decoding](grpc-01-query-preflight-decoding.md)

- [JSON value profiling worklist representation](json-01-profile-worklist-representation.md)

- [Prepared GraphQL type-condition lookup](graphql-02-prepared-type-conditions.md)

- [DynamicInternalUpstream input ownership](routing-01-dynamic-internal-input-ownership.md)

- [RegionRoute prepared lookup indexes](region-01-prepared-lookup-indexes.md)

- [Precomputed backend TLS pool-identity composition](tls-02-precomputed-pool-composition.md)
- [ExternalName DNS address-result ownership](dns-01-address-result-ownership.md)
- [Precomputed fractional Peak-EWMA decay](lb-01-precomputed-ewma-decay.md)
- [Incremental attempt-log size accounting](log-02-incremental-attempt-size.md)
- [SSE dispatch metadata representation](body-05-sse-dispatch-metadata.md)
- [Selective SSE observer input representation](body-04-selective-observer-input.md)
- [Quota admission concurrency](quota-02-admission-concurrency.md)
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
- [Incremental HMAC signing input](hmac-03-incremental-signing-input.md)
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


## Credential reservation limiter borrowing

Credential admission now borrows the limiter owned by AiAttemptAdmissionInput
through the existing Redis await. It clones the same Arc only when a Reserved
or Duplicate result constructs a settlement lease. The input already remains
alive across this operation, and RedisRateLimiter neither inspects reference
counts nor owns a custom destructor. Rejected, failed, timed-out or cancelled
operations no longer acquire and release the redundant limiter Arc. Successful
leases retain the same ownership needed for later settlement.

Client lookup, UUID/key generation, limits, TTL, timeout, Redis invocation and
result mapping are unchanged. Cancellation still leaves uncertain remote charges
to the existing conservative TTL policy. This is a reference-count optimization;
no throughput, allocation or async-frame-size improvement was measured.

Fresh verification passed 53 network-free tests: 24 attempt-runtime and 29 shared
Redis tests. Formatting, workspace check/Clippy, seven repository guards and diff
hygiene passed. Existing warnings remain outside changed code. No live Redis
cancellation, failover or throughput benchmark was run for this batch.


## Quota target identity scratch reuse

build_targets now clears and reuses one local byte buffer across quota limits
instead of allocating a fresh identity buffer per limit. Capacity is requested
lazily inside the iterator, so empty input does not allocate identity scratch.
The same length-prefixed parts are appended in the same order, and the canonical
shared key builder still produces independent owned bucket/marker strings.
The buffer is released when synchronous target construction returns, before
Redis admission awaits. Clock capture, deadlines, limits and target order remain
unchanged. No hash/key format, shared key API or response-body ownership changed.

The existing mixed-scope/reordering regression now compares each combined target
with an independent single-target build, including both bucket and marker keys.
Reversed IdentityModel/Identity ordering verifies that prior model bytes do not
leak into the next identity. This reuses temporary capacity across limits; no
fixed allocation count or measured throughput gain is claimed.

Fresh verification passed 56 network-free tests: 27 quota and 29 shared Redis
tests. Formatting, workspace check/Clippy, seven repository guards and diff
hygiene passed. Existing warnings remain outside changed code. No live Redis
or throughput benchmark was run for this batch.


## Quota admission scheduling audit

Inspected serial Redis admission, maximum-denial retry aggregation, immediate
error exits and the enclosing admission/throttle budgets. Admission Lua reads
without quota-key mutation, but parallelizing up to eight validated targets
changes observation times, failure-path command issuance and deadline outcomes.
Existing concurrent settlement does not establish admission equivalence. The
tradeoff and required acceptance are recorded in
[quota-02](quota-02-admission-concurrency.md); runtime and tests remain unchanged.

Fresh verification passed 56 network-free tests: 27 quota and 29 shared Redis
tests. Formatting, workspace check/Clippy, agent-document validation and diff
hygiene passed. Existing warnings remain outside this documentation-only batch.
No live Redis, concurrency benchmark or pool-pressure measurement was performed.


## Provider cooldown Retry-After borrowing

The raw 429 cooldown hook now borrows the bounded Retry-After value from the
upstream response instead of creating a temporary String. Attempt/outlier
handling synchronously parses it into Duration and stores only numeric cooldown
state. The borrow ends before the original header is removed and any accepted
normalized delta-seconds value is inserted.

The 64-byte bound, text validation, SystemTime sample, provider-reset fallback
evaluation, cooldown precedence and header mutation order remain unchanged.
This removes the temporary copy for eligible nonempty values; normalized output
and other downstream work can still allocate. No throughput improvement or
allocation-free cooldown path is claimed.

Fresh verification passed 44 network-free tests: 4 AI response, 24 attempt runtime
and 16 credential outlier tests. Formatting, workspace check/Clippy, seven
repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No end-to-end provider or throughput benchmark was run.


## Native Anthropic error serialization

Native Messages local errors now serialize private borrowed envelope fields
directly, following the typed OpenAI renderer pattern, instead of building an
owned JSON Value tree on the normal path. Field declaration order preserves
the existing wire order. Status and kind mapping, String return ownership and
named SSE framing are unchanged. The lazy serialization-error fallback retains
the original Value expression rather than introducing a different error reply.

A regression covers all 13 error variants against the original Value renderer:
exact JSON bytes, status, full SSE bytes including leading/trailing boundaries,
and fewer measured render allocations. Final String/event storage still allocates;
this does not claim allocation-free replies or measured throughput improvement.

Fresh verification passed 42 network-free tests: 32 native Anthropic, 1 OpenAI
error and 9 AI context tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live provider or throughput benchmark was run.


## Native quota SSE event reuse

Native Messages quota termination now borrows one of two immutable LazyLock<Bytes>
payloads, initialized by its existing canonical event renderer. This follows the
OpenAI quota-event ownership pattern and avoids repeating fixed JSON serialization
and event formatting after initialization. Only the quota-exceeded and quota-
unavailable events are retained; no request data enters these bounded owners.

EOS/Reject checks, event selection, unconditional SSE boundaries, diagnostics
and terminal handling remain unchanged. terminate_ai_stream still clones Bytes
into the bounded sink when injection is allowed. First initialization and sink
ownership may allocate; this is not a zero-allocation termination claim.
Tests compare both caches to canonical events and verify allocation-free warmed
cache reads, alongside existing native Reject and shared framing regressions.

Fresh verification passed 87 network-free tests: 33 native Anthropic, 50 usage
and 4 AI response tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live provider or throughput benchmark was run.


## Unchanged reported-usage publication

observe_reported_usage now compares the fully normalized next value with the
current optional snapshot and publishes a new Arc only when they differ. The
private ArcSwap remains the sole immutable multi-field publication owner; readers
receive copied values and no consumer depends on repeated store notifications
or Arc identity. Production writers remain sequential response processors.

High-water merging, checked optional-cache normalization and the observation-off
gate retain their order. None still differs from an initial all-zero reported
snapshot. Separate accounting_snapshot reads and quota notifications are unchanged.
The new allocation regression covers repeated zero, identical and lower snapshots,
then invalid merged cache detail clearing an existing valid detail and repeating
that normalized result. Unchanged warmed observations allocate no new snapshot;
changed observations still publish normally. No concurrency-policy or throughput
improvement is claimed.

Fresh verification passed 140 network-free tests: 51 usage, 33 native Anthropic
and 56 Chat-to-Anthropic conversion tests. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain
outside changed code. No live provider or throughput benchmark was run.


## SSE literal comparison without materialization

SliceChain now compares a validated range with a literal through its canonical
borrowed fragment walker. SseEventRef uses it for event_is and its existing
single-data-line literal check. Native Messages classifies its four recognized
event names through event_is, avoiding temporary strings when names span slices;
canonical Chat DONE checks similarly avoid materializing fragmented data.

Matching a valid string literal also guarantees valid UTF-8 for that match.
Case sensitivity, missing versus empty names, duplicate-name selection, the
single-data-line requirement and poisoned-block policies remain unchanged.
The helper rejects reversed/out-of-bounds ranges before visiting storage.
No decoding, coalescing, retained-byte bounds or event ownership was changed.

Regressions compare decoder checks with the prior materializing expressions at
every two-way split, including empty/missing, repeated, mixed-case and invalid
UTF-8 names and multiline data. Chain tests cover sliced/coalesced storage,
empty/invalid ranges and late mismatches. Comparisons allocate zero bytes in
these tests; classification may traverse fragments more than once, so no general
CPU or throughput improvement is claimed.

Fresh verification passed 118 distinct network-free tests: the SSE filter ran
85 and native Anthropic ran 33. The separate 12-test canonical SSE run overlaps
the first filter and is not counted twice. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain
outside changed code. No live provider or throughput benchmark was run.


## Converter SSE name borrowing

Chat-to-Anthropic process_sse_event now compares the SSE name with the parsed
JSON type through canonical event_is. It no longer materializes a name spanning
slices solely for this equality check. The guard stays after JSON/object/type
validation and before known-event dispatch, preserving mismatch failures and
unknown-event handling. Required type values remain valid UTF-8 strings.

Existing converter regressions cover every byte split against complete output
bytes, mismatched names, unknown events and malformed UTF-8. Shared SSE literal
tests cover missing/empty, duplicate names and fragmented comparisons. No new
parser, event classification or stream-state ownership was introduced.

Fresh verification passed 104 network-free tests: 56 converter, 44 SSE module
and 4 SliceChain tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live provider or throughput benchmark was run.


## Selective SSE observer input audit

Native Anthropic and Responses currently materialize semantic event data before
one selective-parser feed. Constructor inspection found lazy empty parser buffers;
a cross-event parser pool is not justified by constructor allocation. Fragment
feeding could remove some semantic merges, but it changes token buffering and
segmentation-dependent work accounting. Net allocation and production-limit
equivalence remain unproven. The bounded proposal and required differential tests
are recorded in [body-04](body-04-selective-observer-input.md). No runtime or test
code changed; converter and offset-based Chat parsing remain outside the proposal.

Fresh verification passed 86 network-free tests: 33 native Anthropic, 2 Responses
and 51 usage tests. Formatting, workspace check/Clippy, agent-document validation
and diff hygiene passed. Existing warnings remain outside this documentation-only
batch. No fragment-feeding prototype, allocation or CPU benchmark was run.


## SSE line classification and UTF-8 result reuse

finish_line now obtains both field kind and malformed status from one local
classification. It no longer materializes and validates the same line again
after metadata admission. Fragmented lines therefore lose one temporary copy;
contiguous lines avoid the repeated validation scan. Comments retain their
UTF-8 exemption and invalid ranges retain their prior classification behavior.

The malformed/poisoned mutation remains after all line/range/metadata admission
gates. Rejected invalid lines cannot publish that state early. parse_block still
performs its independent dispatch-time parsing and completed malformed-event
accounting; no ranges, limits, dispatch order or downstream policy changed.

Regression tests drive actual finish_line with preallocated line metadata and
two-slice valid, invalid and comment lines, verifying one materialization and
immediate flags. A separate invalid line rejected by the line-count gate checks
that flags, line start and data count stay unchanged.

Fresh verification passed 130 network-free tests: 46 SSE module, 33 native
Anthropic and 51 usage tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No throughput benchmark was run; the allocation regression measures this local
line-completion path only.


## SSE dispatch metadata audit

Inspected the line-table lifecycle from finish_line through parse_block and the
RawWindow adapter. Dispatch reparses retained lines, while admission depends on
line/data capacities and independent range/metadata budgets. Caching parsed state
or replacing the line table therefore requires a capacity and acceptance review.
The alternatives and preserved malformed timing, line_count and raw-byte contracts
are recorded in [body-05](body-05-sse-dispatch-metadata.md). No runtime or tests
changed; downstream selective-observer input remains the separate body-04 task.

Fresh verification passed 130 network-free tests: 46 SSE module, 33 native
Anthropic and 51 usage tests. Formatting, workspace check/Clippy, agent-document
validation and diff hygiene passed. Existing warnings remain outside this
documentation-only batch. No new-layout prototype or performance benchmark ran.


## Fixed-point cost fraction formatting

estimated_cost_usd now strips trailing decimal zeroes from the nonzero integer
fraction while decreasing its padding width, then formats the result directly.
This removes the intermediate twelve-digit fractional String. The loop has at
most eleven iterations; leading zeroes, integer output and all pricing arithmetic
remain unchanged. Both attempt and request finalization use this same function.

A differential test compares exact bytes against the previous formatter across
integer boundaries, all trailing-zero counts, irregular fractions and u64::MAX.
Thread-local allocation measurements show fewer allocations for these nonzero
fractions and unchanged allocation counts for integer results. Existing mixed-rate
finalization tests retain coverage of ordinary, cached and output token pricing.
No throughput or CPU improvement is claimed.

Fresh verification passed 65 network-free tests: 52 usage and 13 finalization
tests. Formatting, workspace check/Clippy, seven repository guards and diff
hygiene passed. Existing warnings remain outside changed code.


## Owned credential outlier handle during admission

AiAttemptRuntimeState::admit now takes the owned outlier handle from its input
and moves it into the Closed/Probe lease. This removes cloning its two private
Arcs and their corresponding decrements. The handle has no custom destructor
or reference-count-dependent behavior; the same captured policy generation
and state stay alive in the lease across reservation and runtime processing.

reserve does not read the outlier input field. Admission guards, circuit-breaker
acquisition, Redis failure policy and probe release on cancellation or early
return keep their existing order. The independent plan review approved this
ownership-only change. No allocation or throughput improvement is claimed.

Fresh verification passed 143 network-free tests: 24 attempt runtime, 16 credential
outlier and 103 HTTP upstream-peer tests. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No live Redis cancellation experiment or benchmark was run.


## Bounded attempt identity ownership

Admission and terminal ledger construction now consume backend and credential
identity strings. The shared local helper transfers a String only when its length
is within the existing field bound and its capacity equals its length; otherwise
it uses the existing UTF-8-safe bounded copy. This avoids retaining spare capacity
or truncating a larger allocation in place. Reservation still sees the full alias
before any log-field bounding.

All four ledger callers own a removed mutable active attempt. Only the two identity
fields are taken; snapshot evaluation stays first, and later accounting, reservation
and adaptive operations do not read these fields. The independent plan review
identified this narrow transfer as safe without a borrowed admission API or a
change to the captured-generation lifetime.

Differential tests cover empty strings, spare-capacity empty strings, short/exact/
over-limit values and multibyte boundaries. Accepted transfer cases preserve the
buffer pointer and perform zero allocations; all cases match the previous output
and requested String capacity. This does not claim physical allocator size or
measured throughput improvements.

Fresh verification passed 73 network-free tests: 25 attempt runtime, 35 dispatch
and 13 usage-finalization tests. Formatting, workspace check/Clippy, seven
repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No live provider/Redis experiment or throughput benchmark ran.


## Attempt-log repeated size scan audit

commit_ledger still serializes the retained list at each admission through the
existing zero-allocation counting writer. Eight accepted entries cause 36 entry
visits across those checks; ordinary one-attempt requests do not incur that cost.
Incremental size accounting would reduce scans but add persistent state and a
consistency invariant. The benchmark-led decision and exact wire-size, rollback
and accounting requirements are recorded in [log-02](log-02-incremental-attempt-size.md).
No runtime, schema or test code changed.

Fresh verification passed 38 network-free tests: 25 attempt runtime and 13 usage
finalization tests. Formatting, workspace check/Clippy, agent-document validation
and diff hygiene passed. Existing warnings remain outside this documentation-only
batch. No incremental-counter prototype or benchmark ran.


## Peak-EWMA fractional decay audit

Followed Service selection through compiled EWMA/P2C and score reads, and response
header sampling through the same canonical estimator. Fractional decay uses 625
possible remainders with a 33-entry interpolated Q16 table. Exact compile-time
precomputation could trade interpolation arithmetic for a larger read-only table;
constant-division optimization and cache costs require measurement. The proposal
and preserved estimator/CAS invariants are recorded in [lb-01](lb-01-precomputed-ewma-decay.md).
No runtime or tests changed, and no policy constants were retuned.

Fresh verification passed 139 network-free tests: 127 LB and 12 Service-selector
tests. Formatting, workspace check/Clippy, agent-document validation and diff
hygiene passed. Existing warnings remain outside this documentation-only batch.
No generated-table prototype, assembly inspection or benchmark ran.


## Borrowed Gateway backend TLS singleton

The Gateway backend TLS store now lives directly in LazyLock and its accessor
returns a static reference. All repository callers use borrowed store methods;
none require ownership or reference-count identity of the singleton wrapper.
This removes the wrapper allocation at initialization and one Arc increment/
decrement pair per accessor call, including peer fallback lookup and endpoint
probe freshness checks. No throughput improvement is claimed.

The ArcSwap map and entry/raw/prepared identity Arcs remain unchanged. Captured
certificate generations, same-data raw identity replacement, probe pointer
freshness, publication/withdrawal, policy precedence and pool isolation keep
their existing owners and behavior. Local independently constructed stores are
unaffected. No compatibility alias or new lookup mechanism was introduced.

Fresh verification passed 33 network-free tests: 3 store, 22 peer builder,
4 endpoint-probe transport and 4 runtime handler tests. Peer tests ran serially
because of the existing shared-fixture failures recorded in verification-03;
this does not establish that the parallel fixture issue is fixed. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed. Existing
warnings remain outside changed code. No live TLS rotation or benchmark ran.


## Borrowed BackendTLSPolicy singleton

BackendTLSPolicyStore now lives directly in LazyLock and its getter returns a
static reference. The existing generic Deref-based ConfHandler implementation
also accepts that reference, so ConfigClient handler construction needs no new
adapter or owned Arc. All other repository consumers call borrowed store methods.
The static lifetime already prevented last-owner destruction in the former Arc
representation; no destructor or reference-count-sensitive behavior is involved.

This removes the singleton wrapper allocation and temporary getter reference-count
operations on Service/L4 policy lookup, DirectEndpoint and endpoint-probe paths.
Immutable policy snapshots, prepared native TLS values, effective-policy Arcs,
publication generations and captured pool isolation remain unchanged. No heap
saving is claimed for snapshot capture and no throughput benchmark was run.

Fresh verification passed 72 network-free tests: 33 policy store/handler, 22 peer
builder, 4 endpoint-probe transport and 13 DirectEndpoint tests. Policy and peer
filters ran serially; the known parallel peer fixture issue in verification-03
remains open. Formatting, workspace check/Clippy, seven repository guards and
diff hygiene passed. Existing warnings remain outside changed code. No live
rotation or transport integration run was performed.


## Borrowed BackendTrafficPolicy singleton

The process-global BackendTrafficPolicyStore now lives directly in LazyLock and
its getter returns a static reference. HTTP peer capture, AI policy capture, L4
admission, response-cache evidence and plugin outbound consumers use borrowed
store methods. The existing Deref-based ConfHandler implementation accepts the
static reference without a new adapter or factory change.

Only the singleton wrapper allocation and getter Arc reference-count operations
are removed. The effective ArcSwap generation, retained policy snapshots, retry
budgets, outlier and circuit-breaker handles remain owned as before. Publication
still precedes derived background reconciliation; no live-policy reload replaces
a captured request generation. No throughput improvement is claimed.

Fresh verification passed 208 network-free tests: 64 backend-traffic policy,
103 upstream peer, 21 AI proxy and 20 response-cache storage tests. Policy and
peer filters ran serially. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live transport or throughput benchmark ran.


## Borrowed discovery coordinator accessor

The Service discovery getter now borrows the process-global Arc. ServiceStore,
EpSliceStore and ConfigClient handler construction explicitly clone it at their
existing ownership boundaries. The singleton Arc remains because these owners
also support independently constructed coordinators. Ordinary selection, policy
lookup, affinity and completion callers no longer create a temporary coordinator
Arc solely to call a borrowed method.

Generation loads, runtime-cell and endpoint ownership, completion locking and
retirement remain unchanged. This does not remove snapshot ownership or the
accepted recovery/rejection costs documented in the Service runtime selection
guard. Unit-test edits only remove redundant references after the getter type
change; no fixture, assertion or integration test changed. No throughput claim
is made.

Fresh verification passed 315 network-free tests: 71 discovery, 12 Service
selector, 127 LB, 103 HTTP upstream-peer and 2 ConfigClient tests. Discovery
and peer filters ran serially. Formatting, workspace check/Clippy, seven
repository guards and diff hygiene passed. Existing warnings remain outside
changed code. No live transport or throughput benchmark ran.


## Borrowed numeric endpoint facade

The global RoundRobin EpSliceStore facade now lives directly in LazyLock and
its getter returns a static reference. Outbound target resolution, pinned
resolver calls and health-adjacent inventory use borrowed methods; no consumer
needs an owned facade. This removes the outer singleton allocation and temporary
Arc reference-count operations without changing the facade's coordinator Arc.

Each lookup still loads its generation and returns owned sockets or backend
handles. Service-port validation, actual serving ports, readiness order, fail-closed
non-inet handling, resolver safety filtering and cursor behavior remain unchanged.
Local generic constructors and default ownership remain unchanged. The Service
facade getter has no current production consumer and was left outside this batch.
No general allocation-free discovery or throughput claim is made.

Fresh verification passed 45 network-free tests: 4 endpoint-facade, 4 pinned
resolver, 2 outbound target, 13 DirectEndpoint and 22 session-persistence tests.
Formatting, workspace check/Clippy, seven repository guards and diff hygiene
passed. Existing warnings remain outside changed code. No live transport or
throughput benchmark ran.

## Borrowed outlier-detection singleton

The global OutlierDetectionStore now lives directly in LazyLock and its getter
returns a static reference. HTTP response, connect-failure and proxy-error
callbacks, backend completion-token capture and TCP/TLS connection paths borrow
the same store. The static reference also spans existing L4 awaits. No consumer
requires an owned outer store handle.

This removes the outer singleton allocation and getter Arc reference-count
operations. Internal Service and endpoint Arcs, policy tokens, generation checks,
retirement, cooldown recovery, ejection arbitration and per-attempt classification
remain unchanged. The accepted completion-lock and metric-label costs remain;
this is not a lock-free completion or throughput claim.

Fresh verification passed 244 network-free tests: 36 outlier-detection,
41 discovery-coordinator, 64 backend-traffic policy and 103 HTTP upstream-peer
tests, all with serial test execution. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed. Existing warnings remain
outside changed code. No live transport or throughput benchmark ran.

## Borrowed ExternalName DNS cache singleton

The process-global DnsCache now lives directly in LazyLock and its getter
returns a static reference. ExternalName self-resolution passes that reference
through the existing async resolution owner; Service removal still invalidates
the same shared cache. Local independently constructed caches and their test
Arc owners are unchanged.

This removes the outer singleton allocation and temporary getter Arc operations.
Cache identity, capacity, shard floor, recency, TTL, owned address results and
best-effort removal invalidation remain unchanged. Concurrent misses and failure
handling retain their existing semantics; no coalescing or negative cache was
added. No allocation-free lookup or throughput claim is made.

Fresh verification passed 176 network-free tests: 71 discovery, 103 HTTP
upstream-peer, and the two native DNS tests for captured Service geometry across
await and cache identity/modes/TTL/failure boundaries. All ran serially.
Formatting, workspace check/Clippy, seven repository guards and diff hygiene
passed. Existing warnings remain outside changed code. The separately recorded
TLS admission fixture failure remains outside this batch; no full backend suite,
live transport or throughput benchmark ran.

## Borrowed ExternalName DNS lookup identity

DnsCache now uses a private Cow query representation shared with an owned
entry-key wrapper. Borrowed Service, hostname and resolver-fingerprint components
flow through the canonical ShardedLruCache Borrow interface without constructing
three temporary Strings. Inserted keys still own all components. No shared cache
API, dependency or resolver behavior changed.

The new regression compares previous three-String hashes against owned and
borrowed representations, including empty fields, case differences, component
boundaries and permitted UTF-8. Allocation counting verifies misses allocate
nothing inside DnsCache::get and hits allocate only the independent address
result. Resolver fingerprint construction and address cloning remain separate
costs. A local type-layout probe found both key representations occupy 72 bytes
on the current target; this is not a portable layout guarantee or a throughput
measurement. Canonical recency, shard routing, TTL and removal semantics remain.

Fresh verification passed 225 network-free tests: 47 canonical-cache,
73 discovery, 103 HTTP upstream-peer and two native DNS await/cache-boundary
tests. Gateway filters ran serially. No live transport or throughput benchmark
ran; the previously recorded TLS admission fixture issue remains separate.
Formatting, workspace check/Clippy, seven repository guards and diff hygiene
passed. Existing warnings remain outside changed code.

## Borrowed resolver address fingerprints

ResolverSpec::fingerprint now returns Cow<str>: System borrows its fixed text,
HTTPDNS borrows the prepared generation's redacted address identity, and standard
DNS retains the exact owned formatting. Gateway's existing resolver owner keeps
the borrowed value valid through DNS await and cache insertion. This removes
one temporary String allocation for System and HTTPDNS without changing cache
identity or the private Secret-sensitive preparation identity.

The supporting Common change is limited to this method and its ownership tests.
Existing tests that intentionally retain a temporary resolver's fingerprint now
explicitly own that value; their assertions and fixtures are unchanged. A new
regression checks borrowed variants and prepared-buffer identity, plus unchanged
standard-DNS bytes. No cache API, client preparation, Secret-rotation, fallback,
TTL or provider publication behavior changed. No throughput claim is made.

Fresh verification passed 120 network-free tests: 43 Common DNS tests,
73 Gateway discovery tests and four native DNS/cache/precedence/HTTPDNS rotation
tests, all serially. The existing real localhost resolution test was excluded
from this network-free run. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed. Existing warnings remain outside changed code.
No live transport or throughput benchmark ran.

## DNS address-result ownership audit

Traced cache-hit Vec cloning under the canonical shard mutex, miss insertion
copying and post-lock first-safe-address selection. Shared Arc results and a
scoped projection have different allocation, retention and contention costs;
[dns-01](dns-01-address-result-ownership.md) records the alternatives and preserved
identity, TTL, mode, SSRF and diagnostic contracts for user decision. Production
code and tests are unchanged. Fresh verification passed 73 network-free tests:
47 canonical-cache, 20 DNS-cache, four address-safety and two native DNS-boundary
tests. Formatting, workspace check/Clippy and document/diff validation passed.
No result-representation prototype or throughput benchmark ran.

## Backend TLS pool-composition audit

Peer construction retains necessary native certificate/CA/hook ownership and
already-prepared policy/Gateway fingerprints. Only selected Gateway fallback
identity adds a fixed 102-byte SHA-256 composition per peer; the no-fallback path
reads the prepared policy group key. [tls-02](tls-02-precomputed-pool-composition.md)
records prefix-state and complete-combination precomputation tradeoffs, including
live Gateway rotation beside captured policy generations. Production code and
tests are unchanged. Fresh serial verification passed 58 network-free tests:
22 peer-builder, 33 policy and three Gateway TLS store tests. Formatting,
workspace check/Clippy and document/diff validation passed. No prototype,
throughput benchmark or live TLS/pool verification ran.

## Borrowed adaptive telemetry store accessor

AI request planning now borrows the process-global AdaptiveTelemetryStore Arc
instead of creating a temporary owned handle. The auxiliary ticker explicitly
clones that same Arc at its existing ownership boundary; the global Arc and local
independent store construction remain available. Admin and cold reconciliation
callers continue using borrowed methods.

Only redundant outer reference-count operations are removed. Request-owned
telemetry-state Arcs, immutable publications, selection sequence, generation
cleanup, ticker cadence and shutdown remain unchanged. This does not remove a
heap allocation or establish a throughput improvement.

Fresh verification passed 72 network-free tests: 22 adaptive telemetry,
21 AI HTTP proxy and 29 AI backend-store tests, all serially. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed.
Existing warnings remain outside changed code. No live transport or throughput
benchmark ran.

## Borrowed EdgionConfigData store accessor

Canary and RegionRoute now borrow the process-global EdgionConfigDataStore
instead of cloning an outer Arc solely to call borrowed methods. The direct
LazyLock retains process lifetime, and the existing ConfHandler Deref blanket
supports the factory's boxed static reference. One test call drops its redundant
reference/dereference; its fixture and assertions are unchanged.

Owned snapshot handles, retained resource Arcs, compiled IP matchers and paired
region-table proofs remain unchanged. Partial publication, generation consistency,
fallback and configuration validation retain their existing owners and behavior.
This removes redundant outer reference-count operations without establishing a
throughput improvement.

Fresh serial verification passed 96 network-free tests: 22 configuration-data,
15 Canary, 23 RegionRoute, 11 stream GeoIP, 17 HTTP generation and eight stream
generation tests. Formatting, workspace check/Clippy, seven repository guards and
diff hygiene passed. Existing warnings remain outside changed code. No live
transport or throughput benchmark ran.

## RegionRoute sensitive integer log arguments

Four RegionRoute log sites now pass ports and hash values to the canonical
PluginLog sensitive unsigned-integer formatter. Decimal conversion uses the
existing bounded stack buffer, then the same sensitive-text masking function.
The temporary decimal Strings are removed; masking, field order, entry limits
and rejection remain unchanged. Request writeback Strings, source descriptors,
selected endpoint/SNI ownership and routing behavior retain their existing paths.
The access-log canon documents the explicit helper without expanding permission
to log identities, credentials, arbitrary Display values or additional events.

Regression coverage compares numeric and prior sensitive-text forms across
Off/Replace/Wrap, decimal boundaries through u64::MAX, rendered lengths around
128 and 512 bytes, rejection followed by another entry, JSON and log_full.
Warmed inline scratch formatting allocates zero. Fresh serial verification
passed 57 tests: 34 canonical PluginLog and 23 RegionRoute tests. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed; existing
warnings remain outside the diff. No live transport or throughput benchmark ran.

## Borrowed fixed KeyGet log labels

RegionRoute source diagnostics now obtain borrowed literal labels for the eight
fixed KeyGet variants through the canonical resources-layer as_log_str method.
Its Cow return retains owned formatting for Header, Cookie, Query and Ctx names;
all existing callers accept the result without changes. Condition descriptors
still own their final combined text. KeySet formatting, source extraction,
selection policy, logging masks and schema/transport representation are unchanged.
No resident descriptor cache or parallel source-label implementation was added.

The exact supporting cross-crate scope is the KeyGet method and its unit tests.
New tests cover every variant's bytes and ownership, including empty, mixed-case
and UTF-8 dynamic names whose output survives destruction of the source enum.
Fresh serial verification passed 134 tests: 17 key-accessor, 27 resource-condition,
23 RegionRoute and 67 Gateway-condition tests. Formatting, workspace check/Clippy,
seven repository guards and diff hygiene passed; existing warnings remain outside
the diff. The claim is removal of temporary String allocation for fixed labels;
no live transport or throughput benchmark ran.

## RegionRoute lookup-index audit

Traced direct/map/hash selection and one-hop failover through the existing linear
name/range scans. Validation sorts a temporary range vector and retains only a
compact maximum-bound proof; authored tables remain unsorted. Independent sorted
indexes could reduce scans, but introduce retained-generation and publication
costs. [region-01](region-01-prepared-lookup-indexes.md) records the alternatives
and same-generation ownership, fallback, ordering and measurement acceptance for
user decision. Production code and tests are unchanged. Fresh serial verification
passed 61 network-free tests: 23 Gateway RegionRoute, 22 ConfigData and 16 resource
region tests. Formatting, workspace check/Clippy and document/diff validation
passed. No index prototype, live transport or throughput benchmark ran.

## Borrowed named-backend admission

Canary, DynamicInternalUpstream and GraphqlOperationRouter now validate named
targets without constructing a temporary LogicalBackendHandle. Validation and
owned upstream selection share one private name/index matcher. The original
constructor slot/index predicate is shared with that matcher; denial still wins
before slot inconsistency, and the owned constructor retains its defensive check.
The actual upstream selection continues to pin its compiled generation.

Service classification, optional namespace matching, first-match denial shadowing,
weight-zero selection, plugin-specific onInvalid/logging behavior and jump-lock
ordering are unchanged. No schema scan or second matcher was introduced. This
removes temporary generation Arc reference-count operations, not a heap allocation.
The architecture and named-jump review canon describe both entry points and their
single matching implementation.

New regressions assert explicit outcomes for both APIs across allowed/hidden,
missing/empty, namespace, non-Service, denied-first duplicate and deliberately
misordered compiled slots, including denial before slot failure. The first test
compile exposed a missing test-only import, which was fixed. Fresh serial
verification then passed 147 tests: 15 Canary, 10 DynamicInternalUpstream, four
GraphQL router, 104 upstream-peer and 14 backend-runtime tests. Formatting,
workspace check/Clippy, seven repository guards and document/diff validation
passed; existing warnings remain outside the diff. No live transport or throughput
benchmark ran.

## DynamicInternalUpstream input-ownership audit

Traced owned source extraction, borrowed regex capture, direct/configured target
lifetimes, canonical admission, jump publication and success receipt. Rules input
can potentially stay borrowed, while direct targets currently retain their input
owner across mutable publication. [routing-01](routing-01-dynamic-internal-input-ownership.md)
records staged ownership alternatives and mode/source-specific copy tradeoffs;
the current borrow conflict does not establish that optimization is impossible.
Production code and tests are unchanged. Fresh serial verification passed 83 tests:
10 plugin, six resource configuration and 67 shared condition tests. Formatting,
workspace check/Clippy and document/diff validation passed. No prototype, live
transport or throughput benchmark ran.

## Form encoding of contiguous plain bytes

FormJsonTransform's JSON-to-form encoder writes contiguous safe ASCII runs as
borrowed slices instead of issuing one capped write for every plain byte. Spaces
still encode as plus and other bytes as atomic uppercase percent triplets. The
canonical CappedWriter exposes its remaining output budget so run discovery scans
at most that budget plus one byte before refusing an oversized run. No alternate
writer, temporary encoding buffer or request-body owner was introduced.

Successful bytes and output refusal boundaries remain unchanged. A failed bulk
write can leave a shorter private partial buffer than byte-at-a-time writing;
write_json_pair propagates failure and json_to_form drops that writer, so those
bytes never reach request publication. Pair/error ordering, body diagnostics,
physical caps and header-failure rollback retain their existing paths.

An independent original byte encoder verifies all 256 byte values, mixed escapes,
permitted UTF-8, long plain inputs, prefilled writers and exact/insufficient caps.
Budget tests cover zero/exact/usize::MAX, unchanged budget after refusal and later
fitting writes. Fresh serial verification passed 25 tests: 23 transformer and two
writer tests. Formatting, workspace check/Clippy, seven repository guards and
document/diff validation passed; existing warnings remain outside the diff.
The claim is fewer writer calls for plain runs, not lower CPU cost on every input
or measured throughput. No live transport or performance benchmark ran.

## Bare request media-type validation

The shared JSON and form header validators now recognize their exact bare media
types with borrowed, ASCII-case-insensitive comparisons after the existing unique
text-header gate. Every accepted fast path still checks identity encoding.
Parameterized, suffixed, malformed and other media values retain the original
MIME parser and policy. No header cache, body owner or caller policy changed.

The pinned mime 0.3.17 parser allocates a lowercase owned string for these bare
types. Differential tests compare both validators with the original parser across
case, whitespace, parameters, suffixes, duplicates, missing/non-text headers and
encoding gates. Allocation tests with prebuilt headers verify positive allocation
for the original parser and zero for the new bare-type checks.

Fresh serial verification passed 102 tests: seven shared media, nine JSON schema,
23 form transform, 21 request rewrite, 26 gRPC transcoder, nine REST-to-GraphQL and
seven shared GraphQL tests. Formatting, workspace check/Clippy and seven repository
guards passed; existing warnings remain outside the diff. The claim is removal
of MIME allocation for the two exact bare types, not measured throughput. No live
transport or performance benchmark ran.

## Borrowed GraphQL inline-fragment type context

Traced Gateway GraphqlGuard/GraphqlRateLimit evaluation into the shared Resources
cost visitor. Untyped inline fragments now borrow their current static type name
in both static validation and concrete-object evaluation instead of allocating a
temporary String. Typed fragments retain their existing owned name extraction.
The exact supporting cross-crate scope is graphql_cost/request.rs and its existing
cost-model test module. No schema representation, parser ownership, admission,
Work charge, directive, recursion or error ordering changed.

A new regression first passed against the original implementation, then against
the borrowed implementation. Plain, typed, untyped and mixed nested fragments
retain total cost two and root charge one; exact remaining Work thresholds are
five, eleven and seventeen, with rejection one step below each. Existing tests
also cover covariance, abstract types, malformed fragments and bounded traversal.
Fresh serial verification passed 53 tests: 24 cost-model, 13 GraphQL frontend,
four Gateway guard, five Gateway rate-limit and seven shared evaluation tests.
Formatting, workspace check/Clippy, seven repository guards and diff hygiene
passed; existing warnings remain outside the diff. This removes two allocation
sites for untyped-fragment visits, not all name allocations or repeated frontend
parsing. No allocation benchmark, live Redis or throughput test ran.

## Borrowed GraphQL fragment graph-state keys

The canonical frontend's temporary cycle-detection state map now borrows keys
from the immutable fragment-definition map. graph_fragment obtains the canonical
key through get_key_value only after the existing Work, depth and state checks;
active/completed marks reuse that key. This removes both temporary name copies
for each first-visited successfully completed fragment. Short-lived spread-name
strings remain lookup inputs only, and all state-map borrows end before the
fragment owner moves into ParsedGraphqlRequest.

The supporting cross-crate scope is graphql_request/selection.rs and its existing
frontend test module. Fragment ownership, graph iteration, recursion and Work
charges, cycle/missing-name rejection and selected-operation semantics remain
unchanged. No retained AST cache, second body owner or admission change is added.
New regressions first passed against the original implementation and then the
borrowed implementation: repeated/diamond graphs retain exact Work counts 12/22,
an unused valid fragment retains count five, and unused/unselected cycles,
missing references and duplicate fragment names still reject.

Fresh serial verification passed 60 tests: 15 Resources frontend, 24 cost-model,
four Gateway guard, five rate-limit, one safelist, four router and seven shared
evaluation tests. Formatting, workspace check/Clippy, seven repository guards
and diff hygiene passed; existing warnings remain outside the diff. The benefit
is removal of the graph-state name allocations, not all frontend allocations
or measured throughput. No live transport, Redis or performance benchmark ran.

## GraphQL directive comparisons without owned name copies

Shared frontend/cost directive validation now compares Apollo TokenText directly
for directive names and the if argument instead of converting that same token
text into a temporary String. The pinned Apollo 0.8.6 implementation keeps a
GreenToken handle; the original name helper used the same text extraction before
to_string. The general owned-name helper and all other consumers remain unchanged.
The supporting cross-crate scope is graphql_request/selection.rs and its existing
frontend test module. No token survives validation, and Work, short-circuit/error
order, parser ownership, admission and directive acceptance are unchanged.

A direct directive regression first passed against the original implementation,
then against the optimized version. It covers absent directives, skip/include,
boolean/variable values, repeated directives, case sensitivity, unknown names,
missing arguments, invalid values and duplicate if arguments. Repeated directives
remain accepted; duplicate arguments reject. Exact Work counts and success at the
remaining budget boundary, with failure one step short, are asserted.

Fresh serial verification passed 61 tests: 16 Resources frontend, 24 cost-model,
four Gateway guard, five rate-limit, one safelist, four router and seven shared
evaluation tests. Formatting, workspace check/Clippy, seven repository guards
and diff hygiene passed; existing warnings remain outside the diff. The claim
is removal of two owned-name copy sites, not allocation-free validation or measured
throughput. No live transport, Redis or performance benchmark ran.

## GraphQL cost lookup token text

The shared cost visitor now uses Apollo TokenText for six transient name sites:
static field and argument checks, static and evaluated fragment-spread lookup,
evaluated field lookup and multiplier-variable lookup. These names only feed
comparisons and borrowed string-key lookups; the prior helper extracted the same
token text and then allocated a String. Owned argument-map keys, type-context
names, the general name helper and schema representation remain unchanged.
The supporting cross-crate scope is graphql_cost/request.rs and its existing
cost-model test module.

Lookup counts and indexing versus checked-get behavior are retained, along with
short-circuit order, candidate order, Work, recursion, static/runtime type context
and root accounting. No token escapes the synchronous evaluation owner; Gateway
admission, BodyStorage and Redis permit-release boundaries are unchanged.
A new baseline-first regression verifies aliases, reused fragments, supplied and
default variable values, case-sensitive field/argument/fragment/variable names,
explicit total costs nine/fifteen and their root charges. Existing covariance,
rule-binding and exact Work-boundary regressions also pass.

Fresh serial verification passed 57 tests: 25 Resources cost-model, 16 frontend,
four Gateway guard, five rate-limit and seven shared evaluation tests. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed; existing
warnings remain outside the diff. This removes six temporary String conversion
sites, not all evaluation allocations. No live transport, Redis, allocation or
throughput benchmark ran.

## GraphQL multiplier scalar type projection

Variable multipliers previously built a full owned TypeRef, then consumed only
its scalar-Int predicate and required bit. A dedicated query on the canonical
TypeRef owner now returns that bit directly for Int/Int! and rejects other named
or list types. The cost visitor calls it at the original evaluation point; general
schema/REST type parsing retains its owned representation. The exact supporting
cross-crate scope is graphql_request/input_type.rs, graphql_cost/request.rs and
their unit tests. No eager variable validation, retained cache or plugin-local
type parser is introduced.

The projection avoids constructing the transient name String and wrapper Vec.
Default validation still precedes supplied-value lookup, absent nullable values
still reach existing fallback, and unused incompatible variables are unchanged.
Work, recursion, numeric bounds, admission and request-body ownership are retained.
A baseline-first runtime matrix covers nullable/required Int, case-sensitive
non-Int names, list combinations, missing/supplied/default values and unused
variables. Its invalid-default fixture was corrected to use valid JSON before
the final baseline run. A new mounted type-owner test compares the projection
with parse/scalar_int/required across seven names and nullable/non-null nested
list combinations through eight layers.

Fresh serial verification passed 59 tests: 26 Resources cost-model, 17 frontend,
four Gateway guard, five rate-limit and seven shared evaluation tests. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed; existing
warnings remain outside the diff. This eliminates the temporary owned type
representation on the multiplier path; no live Redis, allocation or throughput
benchmark ran.

## Borrowed GraphQL cost argument keys

The cost visitor's temporary supplied-argument map now borrows canonical names
from the current immutable concrete field's schema argument map. One
get_key_value replaces the existing contains_key lookup, preserving unknown-name
rejection before value extraction. CST values remain owned by the temporary map;
case-sensitive duplicate insertion and multiplier lookup retain their original
semantics. The supporting cross-crate scope is graphql_cost/request.rs and its
existing cost-model test module.

No schema or request owner is retained beyond evaluation. The change removes one
temporary name String per recognized argument while retaining the argument map,
value handles, length guard, Work steps, concrete defaults, interface candidate
order, recursive child evaluation and root accounting. It introduces no request
cache or cross-request state.

A baseline-first regression checks reversed arguments, two/one/no supplied values,
variable defaults, explicit total/root charges, and exact remaining Work thresholds
15/13/11 with rejection one step below. Duplicate, unknown, case-mismatched and
null arguments retain rejection. Fresh serial verification passed 60 tests:
27 Resources cost-model, 17 frontend, four Gateway guard, five rate-limit and
seven shared evaluation tests. Formatting, workspace check/Clippy, seven repository
guards and diff hygiene passed; existing warnings remain outside the diff.
No live transport, Redis, allocation or throughput benchmark ran.

## GraphQL type-condition precomputation audit

Traced static overlap and per-object applicability in the shared cost visitor.
Repeated immutable pair scans charge every target visit and source comparison;
interface possibility order depends on schema HashMap traversal. A faster lookup
must preserve the carried Work acceptance boundary and vector order, while any
prepared index adds bounded compilation and retained-generation costs.
[graphql-02](graphql-02-prepared-type-conditions.md) records measurement-first,
indexed and pair-table alternatives, exact accounting requirements and same-model
ownership for user decision. No runtime or test code changed.

Fresh serial baseline verification passed 60 tests: 27 Resources cost-model,
17 frontend, four Gateway guard, five rate-limit and seven shared evaluation tests.
Formatting, workspace check/Clippy and document/diff validation passed; existing
warnings remain outside the scope. These checks do not establish index performance
or memory benefit. No prototype, live Redis or benchmark ran.

## Borrowed plain GraphQL query components

The canonical GraphQL/REST query decoder now returns Cow text. Components without
percent or plus escapes borrow their original str; escaped components retain the
original byte decoder and owned UTF-8 result. The byte-only escape precheck scans
at most remaining budget plus one, with saturating arithmetic. Plain over-budget
input returns TooLarge and leaves the remaining budget at zero exactly as before;
no str slice is taken at a possible interior UTF-8 boundary.

GraphQL GET retains owned final Map keys and query/operation-name strings, while
unescaped JSON parameters can be parsed directly from borrowed text. REST query
preflight can retain borrowed names and discard validated borrowed values without
constructing temporary decoded strings. Mixed owned/borrowed duplicate names still
compare by content. No body, URI, parser, budget or error ownership changes.
The supporting cross-crate scope is graphql_request/envelope.rs, its new mounted
test module and the existing REST configuration tests.

An independent original-decoder oracle first passed against the baseline, then
against the new implementation. It compares output, error and exact remaining
budget for plain/multibyte/escaped/malformed inputs, budget boundaries inside
UTF-8 and around percent escapes, long inputs, zero and usize::MAX. Ownership tests
verify plain source-pointer identity and owned escaped results. REST duplicate
coverage now exercises raw/escaped names in both orders.

Fresh serial verification passed 85 tests: 19 Resources frontend, 27 cost-model,
nine REST configuration, seven Gateway shared evaluation, four guard, five
rate-limit, one safelist, four router and nine REST adapter tests. Formatting,
workspace check/Clippy, seven repository guards and diff hygiene passed; existing
warnings remain outside the diff. Final JSON ownership still allocates as needed,
and the additional bounded precheck is not claimed to reduce CPU on every escaped
input. No live transport, allocation or throughput benchmark ran.

## GraphQL fragment type text borrowing

The shared cost visitor now retains local Apollo TokenText owners for declared
fragment types instead of copying their bytes into temporary Strings at five
source sites. Optional inline type conditions borrow their text through
Option::as_deref; absent conditions retain the current static context. A present
condition with a missing name still rejects before fallback. No CST owner escapes
evaluation and no cache or prepared type-condition index is introduced.

The baseline-first fragment regression covers named chains, mixed typed/untyped
inline and named fragments, exact remaining Work thresholds 5/11/17 and rejection
one step below, total/root charges, unknown/scalar/case-mismatched types and an
invalid unused fragment. Existing covariance tests retain the independent static
validity and runtime applicability contract. Traversal order, directive checks,
recursion, condition scans and quota accounting are unchanged. This removes the
five explicit String conversion sites; no latency, allocation-count or RSS
benchmark claim follows.

Fresh serial verification passed 62 tests: 27 Resources cost-model, 19 frontend,
four Gateway guard, five rate-limit and seven shared evaluation tests. Formatting,
workspace check/Clippy and seven repository guards passed; existing unrelated
warnings remain outside the diff. No live transport or Redis integration ran.

## GraphQL graph spread lookup text

The canonical frontend graph walk now passes a local Apollo TokenText borrow to
graph_fragment instead of constructing a temporary String for each spread lookup,
including references to already visited fragments. Cycle-state keys still borrow
from the immutable fragment-definition map; definition/variable ownership and
argument/operation duplicate-detection sets are unchanged. This supersedes the
remaining spread-input String observation in the earlier graph-state-key audit.

The baseline-first regression extends existing shared/diamond graph Work checks
with mixed-case fragment names and case-mismatched references in selected,
unselected and unused definitions. Directive validation remains before name
extraction, and graph Work/depth checks, missing-reference/cycle rejection and
whole-document validation retain their original order. No request cache or
retained CST state is added. The benefit is removal of one source-level String
copy site, without measured throughput, heap or RSS claims.

Fresh serial verification passed 67 tests: 19 Resources frontend, 27 cost-model,
four Gateway guard, five rate-limit, seven shared evaluation, four router and one
safelist tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing unrelated warnings remain outside the diff. No live transport,
Redis integration or performance benchmark ran.

## REST-to-GraphQL borrowed variable names

The adapter's synchronous construct method now accumulates owned bound values in
a BTreeMap with keys borrowed from its immutable compiled bindings. The previous
serde_json::Map cloned each present variable name before immediately serializing
the envelope. Values, binding evaluation order, omissions, header preflight,
CappedWriter and complete final-envelope validation remain unchanged; no borrowed
key escapes serialization or reaches publication.

The current Gateway serde_json feature graph has no preserve_order feature, and
its Map uses BTreeMap internally. String and borrowed str keys retain the same
lexical ordering and JSON serialization. Re-evaluate byte-order expectations if
that dependency feature changes; this batch adds no alternate feature mode.
A baseline-first regression uses an independent owned-map envelope oracle for
both selector presence states, nonlexical binding order, absent nullable/defaulted
values, explicit null, nested values/escaping and exact/one-short output caps.
The benefit is removal of one variable-name String copy per inserted binding,
not allocation-free binding/serialization or measured latency/heap/RSS gains.

Fresh serial verification passed 81 tests: ten Gateway REST adapter, seven shared
GraphQL evaluation, four guard, five rate-limit, nine Resources REST configuration,
19 frontend and 27 cost-model tests. Formatting, workspace check/Clippy and seven
repository guards passed; existing unrelated warnings remain outside the diff.
No live integration or performance benchmark ran.

## JSON value profile number length counting

Tracing request JSON processing into the shared Resources value profiler found
Number::to_string used only to measure its length. profile_json_value now sends
the same Number Display output into a private saturating byte counter, removing
one temporary String per numeric node. Gateway ProxyRewrite, REST bindings and
gRPC JSON conversion retain this canonical profiler; cold validation consumers
receive the same change without a second implementation.

Traversal order, saturating totals and depth/node rejection are unchanged. The
strict parser's separately collected accounting is deliberately untouched and is
not an oracle for already-parsed value profiling. Number Display in the current
serde_json dependency delegates to write_str after stack formatting; the counter
always succeeds. A defensive formatting error maps to Malformed rather than
panicking. The traversal worklist still allocates, and no whole-profile
allocation-free or measured latency/heap/RSS claim is made.

Baseline-first tests compare lengths with Number::to_string across signed/unsigned
extremes, signed zero, finite float extremes, subnormals, exponents and deterministic
finite bit-pattern samples. Mixed-tree exact accounting and exact/over-limit
node/depth tests preserve the profiler's admission boundaries.

Fresh serial verification passed 172 tests: 25 Resources schema/profile, nine REST
configuration, 21 rewrite configuration and 28 gRPC codec tests; nine Gateway
schema, 21 rewrite, ten REST adapter, 26 transcoder and 23 form transformer tests.
Formatting, workspace check/Clippy and seven repository guards passed, with existing
warnings outside the diff. No full integration or performance benchmark ran.


## JSON value profiling worklist audit

Traced the shared profiler through REST input/output binding, ProxyRewrite copy,
patch-value and post-operation checks, and gRPC merged request encoding. Scalar
roots still allocate the initial traversal Vec. Eager child population and LIFO
visits make temporary storage shape-dependent; object key counting happens before
child traversal, and depth rejection precedes node rejection. Existing caller
bounds prevent treating this observation as proof of unbounded gateway allocation.

[json-01](json-01-profile-worklist-representation.md) records root-outside-Vec,
inline storage and iterator-frame alternatives with their allocation/stack/order
tradeoffs, required simultaneous-limit error oracles and measurement acceptance.
No runtime change, replacement algorithm or benchmark was made. Strict parser
accounting remains separate from already-parsed value profiling.

Fresh serial baseline verification passed 172 existing tests: 25 Resources
schema/profile, nine REST configuration, 21 rewrite configuration, 28 gRPC codec;
nine Gateway schema, 21 rewrite, ten REST adapter, 26 transcoder and 23 form.
Formatting check, workspace check/Clippy and documentation/diff checks passed;
existing warnings remain outside this documentation-only batch. These results
validate the current baseline, not any proposed traversal representation.

## gRPC field selector root descriptor borrowing

The shared transcoder field resolver now starts with Cow::Borrowed on its supplied
MessageDescriptor and switches to Cow::Owned only when the existing nested-message
descent returns a new descriptor. This avoids the initial descriptor handle clone
and drop for each selector lookup, including query binding and cold path/body
compilation. prost-reflect's MessageDescriptor clone copies an index and an
Arc-backed DescriptorPool handle; this is not a descriptor-byte or heap-allocation
removal claim. Returned FieldDescriptors retain their own existing ownership.

The baseline-first resolver regression covers proto/JSON aliases, flat/nested
paths, terminal list/map/message acceptance, prohibited traversal through list,
map or scalar fields, malformed/unknown/case-mismatched names, the 32/33-segment
boundary and overlong selectors. Returned descriptors remain usable after the
input descriptor and pool leave scope. Lookup priority, all failure gates, path
vectors, binding overlap rules, normalization, admission and output are unchanged.
No new descriptor cache or compiled selector index is introduced.

Fresh serial verification passed 55 tests: 29 Resources transcoding and 26 Gateway
transcoder tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing unrelated warnings remain outside the diff. No real-socket gRPC
integration, allocation benchmark or throughput measurement ran.

## gRPC selected binding handle borrowing

CompiledGrpcTranscoder::bind now borrows the selected Arc from its immutable
binding collection through the existing second-match ambiguity check and the
synchronous inner bind. The inner CompiledBinding::bind still clones its Arc
into BoundRequest only on success. This removes one redundant transient Arc
increment/decrement without weakening returned-generation ownership or changing
method/path matching, length admission, query decoding or error ordering.

A baseline-first regression drops the compiled owner before encoding the returned
bound request and verifies its field payload. It also exercises wrong-method and
missing-path NotFound, overlong-path Invalid and runtime duplicate-match Invalid;
the latter deliberately bypasses cold disjointness admission only inside the test.
No payload-copy, heap-allocation or measured throughput improvement is claimed.

Fresh serial verification passed 56 tests: 30 Resources transcoding and 26 Gateway
transcoder tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing warnings remain outside the diff. No real-socket integration or
performance benchmark ran.


## gRPC query preflight decoding audit

Traced query binding from the aggregate encoded-byte cap through path capture,
strict whole-query preflight and per-component form decoding. Queries without
percent signs already borrow during preflight; escaped queries materialize a
strict decoded String that is discarded before the form parser decodes again.
The dependency form parser is permissive, so removing preflight would change
malformed and ignored-unknown acceptance. Raw separators and encoded plus prevent
splitting/re-decoding the preflight output as an equivalent shortcut.

[grpc-01](grpc-01-query-preflight-decoding.md) records validation-only, ASCII-fast-path
and strict-component alternatives, their validation-order/ownership constraints and
CPU/temporary-buffer measurement requirements. It also preserves the existing
64-accumulated-selector rule rather than inventing a raw-pair cap. No runtime code,
replacement parser or benchmark was added.

Fresh serial baseline verification passed 56 tests: 30 Resources transcoding and
26 Gateway transcoder tests. Formatting check, workspace check/Clippy and
agent-doc/diff checks passed; existing warnings remain outside this documentation
batch. These checks do not establish performance or equivalence of the proposals.

## gRPC decoded scalar ownership transfer

Path and form-query binding now pass their decoded Cow<str> into scalar conversion.
String, bytes, enum and validated 64-bit integer text move an existing owned
String into the bound JSON value; borrowed text still receives an owned copy.
Numeric parsing, error ordering and original integer spelling are preserved.
BoundRequest retains independent ownership after input buffers leave scope.

The baseline-first scalar regression covers borrowed and owned forms across all
scalar families, integer boundaries and malformed values. A separate escaped-path
regression verifies pointer and capacity identity for the moved string-producing
families. This establishes buffer transfer, not measured throughput or RSS gains.
Decoder spare capacity remains retained; the owning conversion memory ledger now
explicitly accounts for it under the unchanged encoded input cap and existing
selector/scratch allowance. No exact-capacity equivalence is claimed. Strict query
preflight remains unchanged; grpc-01 still requires a separate decision.

Fresh serial verification passed 58 tests: 32 Resources transcoding and 26 Gateway
transcoder tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing unrelated warnings remain outside the diff. No real-socket
integration or performance benchmark ran.


## gRPC integer normalization storage audit

Traced RequestFilter conversion through binding/body merge, JSON profiling, exact
integer normalization, capped serialization and tree release before reflection.
Every accepted integer currently receives a fresh canonical decimal String,
including already canonical text. Reusing the old allocation can avoid formatting
storage but also retain oversized buffers after long zero/exponent inputs collapse.
Canonical-only reuse still needs a capacity policy for decoder/parser spare space.

[grpc-02](grpc-02-integer-normalization-storage.md) records alternatives and their
required semantic, capacity and peak-overlap evidence. No implementation or new
threshold is selected, and no breach of the current memory ledger is claimed.

Fresh serial baseline verification passed 58 tests: 32 Resources transcoding and
26 Gateway transcoder tests. Formatting check, workspace check/Clippy and agent-doc
and diff checks passed; existing warnings remain outside this documentation batch.
No allocation or throughput benchmark ran; tests do not prove proposed reuse safe.

## gRPC non-null retention lookup

Ordinary-message normalization now retains non-null fields immediately during
its null-removal pass. The preceding full name/alias validation and following
field normalization remain unchanged. Null values still resolve their descriptor
and use the same list/map and Value/NullValue sentinel rules. This removes one
redundant descriptor lookup and handle construction for each non-null field in
this pass; it does not introduce a prepared schema cache or merge validation passes.

A baseline-first regression verifies unknown non-null fields and mixed null/non-null
alias duplicates fail before any null removal or integer rewriting. A valid mixed
object still retains text, removes an ordinary null and canonicalizes its integer.
Existing collection sentinel, nested validation and wire regressions remain intact.
No allocation-count, throughput or memory-budget improvement is claimed.

Fresh serial verification passed 59 tests: 33 Resources transcoding and 26 Gateway
transcoder tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing unrelated warnings remain outside the diff. No real-socket
integration or performance benchmark ran.

## gRPC codec descriptor handoff

Request encoding now borrows one method input descriptor for normalization and
moves that same handle into DynamicMessage deserialization. Response decoding
likewise borrows one output descriptor for wire admission and moves it into
DynamicMessage decoding. Descriptor acquisition remains after the existing
profile or response-byte checks, and admission still precedes reflection.

The locked prost-reflect implementation builds these handles from a pool Arc and
an index. Each path reaching reflection avoids one redundant descriptor handle
clone/drop; this is not a descriptor-data copy, heap-allocation or throughput
claim. The binding already pins the pool across the request serialization interval.
JSON-tree release, capped writers, panic containment, output envelopes and memory
reservations remain unchanged. Existing ownership, malformed-input/wire, limits,
WKT and round-trip regressions cover this local handoff without new mirrored tests.

Fresh serial verification passed 59 tests: 33 Resources transcoding and 26 Gateway
transcoder tests. Formatting, workspace check/Clippy and seven repository guards
passed; existing unrelated warnings remain outside the diff. No real-socket
integration or performance benchmark ran.


## gRPC wire-admission schema audit

Traced unary and streaming response payloads through canonical wire admission,
reflection and borrowed ProtoJson serialization. The dependency already indexes
field numbers with BTreeMap; per-tag shape extraction still constructs Arc-backed
handles and derives kind/list metadata. A prepared schema graph could reduce that
work but duplicates cold state and requires bounded recursive/shared graph ownership.

[grpc-03](grpc-03-prepared-wire-schema.md) records that tradeoff, sparse-tag handling,
runtime unknown-field semantics, stricter descriptor mode and exact budget/order
acceptance. No replacement walker or cache was implemented. The existing recursive
serializer already borrows values and forwards collect_str without an extra JSON tree.

Fresh serial baseline verification passed 59 tests: 33 Resources transcoding and
26 Gateway transcoder tests. Formatting check, workspace check/Clippy and agent-doc
and diff checks passed; existing warnings remain outside this documentation batch.
No performance benchmark ran; these results do not validate a proposed schema graph.


## gRPC callback output ownership audit

Traced streaming feed through message decode, RawWindow replacement normalization,
callback aggregation and capacity-charged output ownership. Lazy callback writers
already avoid empty/incomplete work, but one completed record still uses the extra
aggregate writer. [grpc-04](grpc-04-single-record-output.md) records a possible
first-record handoff and its fallback, capacity and generation-lifecycle tradeoffs.
RawWindow may copy replacements; Bytes-to-Vec transfer is not assumed allocation-free.

Terminal static bytes still need the output owner that pins admission. Content-Length
formatting was also inspected: the locked header API accepts owned strings, so a
numeric HeaderValue substitution is not treated as an established allocation removal.
No output-owner API, writer policy or runtime behavior changed in this audit.

Fresh serial baseline verification passed 59 tests: 33 Resources transcoding and
26 Gateway transcoder tests. Formatting check, workspace check/Clippy and agent-doc
and diff checks passed; existing warnings remain outside this documentation batch.
No allocation or throughput benchmark ran.

## ExtProc explicit empty trailer mutation ownership

The response-trailer callback now treats a HeaderMutation whose set_headers and
remove_headers lists are both empty like an absent mutation for candidate storage.
It validates the original map without cloning/replacing it. Every nonempty list
still uses the existing atomic candidate path, even if its operations later skip
or make no effective change. Final map validation, representability, failure policy,
phase matching and terminal sequencing remain unchanged.

This extends the earlier unchanged-trailer-map optimization, superseding only its
explicit-empty-mutation exclusion. The owning header-fidelity knowledge now states
the current scope. Baseline-first callback cases cover absent/empty forms across
HTTP/gRPC and both failure policies, preserving repeated opaque values, sensitivity,
projection and outcomes. The optimized forms both retain original HeaderValue
storage. Unrepresentable trailers still follow both configured failure policies.
This removes the empty-mutation candidate map clone, not protobuf projection or
all callback allocations; no throughput improvement is claimed.

Fresh serial verification passed 153 tests: 142 ExtProc and 11 upstream response
trailer-driver tests. Formatting, workspace check/Clippy and seven repository
guards passed; existing unrelated warnings remain outside the diff. No live
ExtProc interoperability or performance benchmark ran.

## ExtProc terminal summary lock scope

Logging terminalization now acquires the summary mutex once after releasing the
state mutex. It preserves the guarded pending-to-cancelled/logging-fallback update
and drops the redundant preliminary summary read. Finished/Failed owners with a
pending summary previously acquired that mutex twice; other paths already acquired
it once. Lazy/Open still become Finished, and no state/summary guards overlap.

A baseline-first state/outcome table covers pending and settled summaries across
Lazy/Finished/Failed states, including previously emitted summaries, repeated
terminalization, retained counters/reasons and single emission. The final test also
explicitly checks Finished state after lazy retirement. Existing stream cleanup
and logging integration coverage remains. This preserves the conditional transition,
not identical thread scheduling or an atomic combined state/summary snapshot.
No new tracing, metrics, log bytes or allocation/throughput claim is introduced.

Fresh serial verification passed 152 tests: 143 ExtProc and nine terminal logging
tests. Formatting, workspace check/Clippy and seven repository guards passed;
existing unrelated warnings remain outside the diff. No live integration,
contention benchmark or performance measurement ran.

## ExtProc TLS server-name ownership

The TLS dial connector now uses the owned ServerName returned by TryFrom<String>
directly, with an explicit static lifetime annotation. Locked rustls-pki-types
1.15.1 already consumes the supplied String; its former trailing to_owned cloned
that owned DNS string again. Removing it avoids one DNS-name string clone per
successful name conversion. IP server names only copied their address value in
that operation, so no IP string-allocation saving is claimed.

The separate host clone feeding name conversion remains because the async dial
also borrows its host for canonical egress resolution. Name parsing, original
case, invalid-name errors and their placement before connect_admitted are unchanged.
SNI, CA/mTLS, hostname/SAN capability rejection and target-runtime identity retain
their owners. This is the permitted backend-independent outbound rustls client,
not Gateway TLS-backend expansion. No new cache or parser was introduced.

Fresh serial verification passed all 143 ExtProc tests. Formatting, workspace
check/Clippy and seven repository guards passed; existing unrelated warnings
remain outside the diff. Existing constructor/identity/admission tests and the
static lifetime check cover this local ownership change. No live TLS handshake,
allocation benchmark or throughput measurement ran.

## ExtProc dial host sharing

Plain and TLS connector construction now moves the existing host String into one
Arc<String>. Service clones and newly created dial futures clone that shared handle
rather than copy host text. The original String capacity stays owned until the last
connector/future releases it. This adds one cold Arc allocation and atomic handle
operations; it is not a claim that every traffic pattern allocates less overall.

TLS still copies the host once into its independent owned ServerName inside the
polled future, preserving validation/error timing before connect_admitted. The
canonical resolver receives the same borrowed host bytes and performs its existing
DNS and egress checks for each dial. Port, address fallback, timeouts, TLS policy,
cache identity and cancellation ownership are unchanged. No process-wide cache,
prepared name parser or additional configuration state was introduced.

Fresh serial verification passed all 143 ExtProc tests. Formatting, workspace
check/Clippy and seven repository guards passed; existing unrelated warnings
remain outside the diff. No live DNS/TLS integration or allocation/throughput
benchmark ran; lazy channel-construction tests are not handshake evidence.

## ForwardAuth prepared client-header selection

ForwardAuth construction now prepares case-insensitive client-header selection
once, retaining the first original spelling and config order. A temporary cold
HashSet of ASCII-lowercased names is dropped after preparation. Denial responses
iterate that list instead of repeatedly scanning each configured prefix. The
private response writer receives only the auth HeaderMap and uses its owner's
prepared list; source configuration and success-side upstreamHeaders are unchanged.

Dynamic response protection and Connection nominations remain per-response checks.
Repeated source field lines, first-value replacement then append, invalid-text
filtering, content type, body ownership and failures retain their existing behavior.
Baseline-first denial tests now include nonadjacent mixed-case names and duplicate
protected/nominated names; helpers exercise the production initialization path.
A constructor regression verifies first spelling/order, config immutability and
unchanged upstream-header duplicates. This removes repeated name-selection work,
not header copying or all denial-path allocations; no throughput gain is measured.

Fresh serial verification passed 13 Gateway ForwardAuth tests and 13 Resources
ForwardAuth tests. Formatting, workspace check/Clippy and seven repository guards
passed. Existing unrelated Clippy warnings remain outside this diff. The expanded
12-test denial baseline passed before the production change; no network or
allocation/throughput benchmark ran.

## Outbound URL serialization ownership inspection

Traced dynamic Service authority replacement from shared outbound request dispatch
through replace_url_host and final request URL construction, including the shared
active-health caller. Consuming the parsed Url would avoid its successful text
copy but transfer its spare String capacity. A locked url 2.5.8 probe found equal
bytes with copied/moved capacities of 20/54 and 45/84 for two host replacements.
These requested capacities do not establish physical heap usage or async retention.
The tradeoff and acceptance boundaries are deferred in outbound-02; no runtime,
resolver, authority policy or URL assembly behavior changed.

Fresh serial verification passed all 120 shared outbound tests. Formatting,
workspace check/Clippy, agent-doc validation and diff whitespace checks passed.
Existing unrelated Clippy warnings remain. The local storage probe is separate
from those regression tests; no live DNS/TLS or throughput benchmark ran.

## Outbound origin Header and Query borrowing

The shared request field collector now borrows live first-value Header text and
frozen decoded Query text through canonical session accessors. The prior owned
accessors copied those same values. Collection and final request assembly are
synchronous; Cookie and template values remain owned. Presence order, bounded
warnings, sensitive flags, invalid Header text filtering and output encoding stay
unchanged. Coherent fixtures preserve explicit Expect forwarding without making
owned and borrowed views disagree. The expanded 121-test baseline passed before
the source change; added ownership assertions check actual collector pointers and
retain owned Cookie extraction. No end-to-end allocation or throughput claim.

A separate documentation discrepancy remains unresolved: the origin/template
comparison in skills/01-architecture/03-controller-gateway-link/webhook.md describes
origin Header reads as frozen and immune to earlier rewrites, while both the old
header_value implementation and this borrowed path read the live map. The owning
borrowed-header guard also describes live access. Reconcile the intended contract
and that architecture wording separately; this optimization neither changes
header timing nor establishes that the frozen-header description is satisfied.

Fresh serial verification passed 122 shared outbound tests, 13 ForwardAuth tests
and 45 Webhook provider tests. Formatting, workspace check/Clippy and seven
repository guards passed. Existing unrelated Clippy warnings remain outside the
diff; no live network integration or throughput benchmark ran.

## Canonical template variable borrowing

The canonical resolver now uses private Cow values: Header, decoded Query,
current/entry path, method, remote IP and resolved Secret text borrow their
existing owners until the synchronous output append. Cookie, ctx, URI and
matched-route results remain owned. Final output still owns independent text;
no consumer-local walker, new cache or Secret lookup was added. Typed Header
sensitivity is recorded before invalid-text filtering as before, including
opaque sensitive fields. Missing/empty distinctions, Request/Trusted capability
errors, Secret denial, escaping and bounded output growth are unchanged.

Coherent query mocks and the opaque-sensitive Header assertion first passed on
the old resolver. New pointer/Cow assertions cover borrowed sources and retained
owned cases; a rendered result remains valid after dropping session and Secret
owners. This eliminates selected intermediate String copies, not final rendered
storage or all template allocations; no throughput gain is measured.

Fresh serial verification passed 361 tests: 23 template, 122 shared outbound,
13 ForwardAuth, 45 Webhook provider, 114 canonical AI guard, 21 ProxyRewrite,
12 CtxSet, 4 dye, 6 authority and one real-adapter sensitive-source case. The old
plugin-local AI guard filters matched zero tests; the replacement ai::guards
filter verified both current request and semantic owners. Formatting, workspace
check/Clippy and seven repository guards passed. Existing unrelated warnings
remain; no live network integration or throughput benchmark ran.

## Template JSON append-run inspection

Traced canonical append_json through the outbound JSON consumer. Safe-span
batching could reduce per-character append work but changes growth requests.
An isolated writer probe with initial capacity 32 and 70 plain bytes plus an
escaped quote produced equal 72-byte output with scalar/batched capacities
128/140, for both uncapped and 256-byte-capped modes. Current outbound JSON uses
uncapped rendering; bounded JSON remains a supported tested writer capability.
The storage tradeoff is deferred in template-01, with no production edits or
performance claim. Private partial output on failure is discarded, so its shape
alone is not an external regression.

Fresh serial verification passed 23 template and 122 shared outbound tests.
Formatting, workspace check/Clippy, agent-doc validation and diff checks passed;
existing unrelated Clippy warnings remain. The isolated capacity probe is not
a full-template allocation, live network or throughput benchmark.

## Dye outcome membership preparation

Compiled dye rules replace their cloned outcome Vec with a private u8 mask built
once through an exhaustive five-variant mapping. Both real-adapter rendering loops
use that prepared membership before their unchanged conditions. This removes a
linear membership scan per visited rule and the compiled subscription Vec storage;
no measured throughput gain is claimed. Rule/direction order, template rendering,
anti-spoof stripping, success/failure header operations and queue timing remain
unchanged. Source configuration, validation and capabilities are untouched.

A baseline-first regression compares every one of 32 subsets and reversed/duplicate
lists against source Vec membership for all five outcomes, including the empty set.
The default-plan assertion and OIDC refresh-timeout producer assertion now use the
compiled predicate. No other runtime consumers of the old list were found.

Fresh serial verification passed 23 Gateway dye tests, 176 full Session adapter
tests, the focused OIDC refresh-timeout case, and 29 Resources dye tests. These
selections overlap and are not a unique aggregate count. The old-plan baseline
passed five plan tests. Formatting, workspace check/Clippy and seven repository
guards passed. Existing unrelated Clippy warnings remain; no live network or
throughput benchmark ran.

## Borrowed valid dye field values

The canonical Resources sanitizer now returns Cow, borrowing text accepted by its
existing field-value predicate and retaining the same character filter/collection
for invalid input. This supporting cross-crate edit is limited to that helper and
its tests. The Gateway dye renderer borrows clean recorded Success text through
its ordinary Header setter, and avoids a temporary sanitized String when formatting
clean failure causes. Template results still become independent owned sanitized
text; their original spare capacity is not transferred. Prefix handling, template
precedence, invalid-control removal, header errors and queued ownership are unchanged.

An ASCII/control and permitted multibyte differential first passed with the old
sanitizer. New Cow/pointer checks cover valid empty/tab/multibyte text and owned
filtering. A real-adapter test retains a raw-value borrow across a Header write
and after adapter release while checking owned template/failure results. Invalid
text adds a precheck before filtering; no universal CPU or throughput gain is
claimed. Final headers, queued operations and rendered templates still allocate.

Fresh serial verification passed 31 Resources dye tests, 24 Gateway dye tests and
177 full Session adapter tests (overlapping selections). The initial Gateway
compile caught a shadowed method lifetime; renaming it to an independent raw-value
lifetime resolved the error. On corrected source, formatting and workspace
check/Clippy passed. Seven repository guards passed, with documentation, tracing
and unit layout checked again after correction. Existing unrelated warnings
remain; no live network integration or throughput benchmark ran.

## Borrowed already-folded MatchCore lookup

The canonical Resources MatchCore insensitive HashSet branch now borrows input
without ASCII uppercase bytes instead of unconditionally allocating its lowercase
copy. Inputs with uppercase bytes retain the same owned ASCII conversion. This
supporting cross-crate change is limited to the hot matcher and its tests; compiled
set contents, the 16-value threshold, case-sensitive and linear paths, original-text
regex fallback and shared payload validation remain unchanged. Non-ASCII UTF-8
bytes are never Unicode-folded by either path.

A baseline-first differential compares hashed and linear results against the
source values, including empty/missing values, ASCII cases, permitted multibyte
text and uppercase case-sensitive regex fallback. A real compiled Gateway condition
uses 17 values and borrowed query input; warmed lowercase/empty/multibyte hit/miss
cases allocate zero calls/bytes. Uppercase lookup remains covered without a zero
allocation claim. Its additional uppercase precheck is not a universal CPU win;
no throughput benchmark ran.

Fresh serial verification passed 132 tests: 28 Resources conditions, 68 Gateway
conditions, 18 RequestRestriction and 18 RequestAccessPolicy. Formatting, workspace
check/Clippy and seven repository guards passed. Existing unrelated Clippy warnings
remain; no live network integration or end-to-end throughput benchmark ran.

## CtxSet regex replacement ownership inspection

Rechecked shared condition async dispatch: direct futures immediately return for
synchronous predicates, with no per-atom boxed-future allocation at those sites.
Then traced CtxSet replacement output through mapping and the canonical context
setter. Moving replace_all's owned Cow could eliminate a copy, but locked regex
1.13.1 initially reserves the input length. A 65,536-byte-to-one-byte replacement
probe retained capacity 65,536 when moved versus 1 when copied, with equal text.
The buffer only crosses synchronous mapping/set work; the current setter copies
into independent context storage. Higher peak memory, long-lived context growth
and throughput gains are not established. The tradeoff is deferred in ctx-01;
no production behavior changed.

Fresh serial verification passed 12 Gateway CtxSet and 34 Resources CtxSet tests.
Formatting, workspace check/Clippy, agent-doc validation and diff checks passed.
Existing unrelated Clippy warnings remain. No live network or throughput benchmark
ran; the storage probe is separate from the regression tests.

## ProxyRewrite initial template copy

The URI/header variable resolver now calls its first `str::replace` directly on
the borrowed template. Previously it copied the template into a temporary String
only to call that same replacement and discard the copy. Receiver bytes, pattern,
escaped replacement and the newly constructed result remain identical. This
removes the initial temporary text copy for nonempty templates without transferring
input capacity into the result. Later argument, numeric-capture, named-path and
dollar-restoration stages retain their order and existing boundaries.

Independent plan review approved the two-file scope. The existing 21 ProxyRewrite
tests passed both before and after the change. Formatting, workspace check/Clippy
and seven repository guards passed; existing unrelated Clippy warnings remain.
No live integration, whole-path allocation or throughput measurement ran.

## ResponseRewrite prepared rename protection

The response-header callback's rename operations now carry a protected-name
decision computed by the canonical predicate during immutable plugin construction.
Both names, entry order and duplicate operations remain owned by that generation.
Runtime rejection still precedes the dynamic source read and preserves static
failure logs and continued processing. Validation runs before preparation, and
session writer protection remains unchanged. The dynamic value fidelity decision
in response-02 is unaffected.

This removes repeated name-policy scans at the cost of one boolean and possible
layout padding per configured rename. No total-memory or throughput improvement
is claimed. The independent plan review approved four files. The original ten
ResponseRewrite tests passed before the change. Fresh verification passed ten
Gateway ResponseRewrite, six Resources response-policy and 15 Resources
ResponseRewrite tests. Coverage includes both protected sides, mixed case,
Proxy-Connection, pseudo-headers and caller-configuration mutation. Formatting,
workspace check/Clippy and seven repository guards passed; existing unrelated
Clippy warnings remain. No live integration or throughput benchmark ran.

## WAF response status formatting

Traced the Gateway WAF response-header callback through publish_response_head to
the leaf transaction's ResponseState::build. Status bytes now use the existing
stack-backed decimal_bytes helper before the same fallible owned_copy. This
removes an intermediate String; final transaction ownership, retained-byte
accounting, status/header validation and entry-reservation order remain unchanged.
The Gateway remains the anchor; the leaf source and unit-test file are the exact
supporting edit scope. The borrowed response-header projection vector remains:
the current owner validates count and all headers before constructing entries,
then traverses them again. No iterator/API redesign is part of this batch.

Independent plan review approved the three-file scope. A new test passed against
the original implementation for all 900 accepted statuses, final collection bytes
and accounting, and invalid-status precedence over malformed headers. Fresh final
verification passed four leaf response tests and 50 Gateway WAF tests. Formatting,
workspace check/Clippy and seven repository guards passed; existing unrelated
Clippy warnings remain. No live integration, throughput or whole-path allocation
measurement ran.

## WAF response completion reachability

The only Gateway call to execute_response_window is WafPlugin's completion hook,
which passes an empty terminal window. There is no Gateway response-chunk feed
to that leaf API. Positive/non-excluded RESPONSE_BODY targets are classified as
unavailable FullResponseBody and rejected by strict compilation; support reports
do not authorize publication. The leaf API's nonempty-window capability is not
evidence that the Gateway uses it on streamed response chunks.

For the current empty window, owned_copy creates an empty Vec, reserves zero and
extends an empty slice. There is no payload allocation to remove at that site.
This does not imply that phase execution, matchers or outcome summaries allocate
nothing. Completion claims the selected owner's phase, moves its transaction out,
executes phase four and then logging, and does not reinstall the transaction.
Logging still executes after a successful phase-four intervention; only a runtime
error returns early. The final intervention prefers phase four via Option::or.
Owner mismatch and duplicate claims skip execution. These ownership and ordering
boundaries were retained, not optimized away.

Direct WAF completion wiring is inspected; deeper engine operators and full
response-chain coverage remain open. This documentation-only batch passed
independent plan review and fresh serial tests: 50 Gateway WAF, four leaf response
and 33 Resources WAF tests. Formatting and workspace check/Clippy passed with
existing unrelated warnings. No live transport or performance benchmark ran.

## Wasm failure-cause borrowing

The shared Wasm failure-policy path borrows causes that contain no spaces instead
of allocating an unchanged replacement String. Causes containing spaces retain
the original space-to-hyphen replacement and gain a preliminary scan. Metrics
and PluginLog still receive the original reason; dye receives the same normalized
text before the same fail-open continuation or fixed fail-close response.
The borrow ends during synchronous outcome recording. Metric, log and downstream
outcome storage may still allocate; this is not a whole-path allocation claim.

Independent plan review approved the three-file scope. A semantic regression
passed on the original implementation for all 13 current plugin/attached reasons
in both failure modes, checking outcome, exact log and response body. The updated
test additionally checks original pointer reuse at the outcome call for spaceless
reasons. Fresh serial verification passed all 264 Gateway Wasm tests. Formatting,
workspace check/Clippy and seven repository guards passed; existing unrelated
Clippy warnings remain. No live transport or throughput benchmark ran.

## Wasm trailer companion binding transfer

Attached response setup moves its last local WasmRequestBinding Arc into the
trailer companion instead of cloning and dropping the local handle. The earlier
body companion clone remains necessary. ExecutionLoan already owns an independent
Arc and still completes or aborts through that owner; rejected installation can
drop the offered observer before fail_mode without releasing the live execution.
No-hooks, header EOS, callback ordering, cancellation and terminal cleanup retain
their existing branches. This removes one Arc increment/decrement when trailer
installation is attempted, not a heap allocation.

Independent plan review approved the two-file scope. All four existing attached
SDK lifecycle tests passed before the change. Fresh serial verification passed
all 264 Gateway Wasm tests. Formatting, workspace check/Clippy and seven repository
guards passed; existing unrelated Clippy warnings remain. No live transport or
throughput benchmark ran.

## Wasm local-response deduplication keys

Local-response replay now borrows header names without ASCII uppercase letters
as HashSet Cow keys. Names requiring case folding retain the same lowercase
String construction after a preliminary scan. The original local header vector
already lives across the entire synchronous loop, so borrowing adds no payload
lifetime extension. Original spelling reaches the same first-set/later-append
writers in the same order, including best-effort failure handling.

This removes temporary normalized-name copies for unchanged names, not the set
allocation or response-map storage. ReplaceAll's consuming pair loop, guest ABI
admission and deferred snapshot sharing remain outside this two-file scope.
Independent plan review passed. Three existing local-response regressions passed
on the original implementation, including mixed-case repeated cookies applied to
a real ResponseHeader, empty bodies and non-UTF-8 bodies. Fresh serial verification
passed all 264 Gateway Wasm tests. Formatting, workspace check/Clippy and seven
repository guards passed; existing unrelated Clippy warnings remain. No live
transport or throughput benchmark ran.

## Response-cache terminal ownership and Age construction

Rechecked logging-to-cache finalization: pending publication moves out before
awaited finish_body so cancellation releases the writer even if the context
survives. Publication retains its epoch/freshness fence. No ownership change was
selected. Replay Age construction transfers its decimal String into HeaderValue;
direct u64 insertion has different allocation behavior in locked http 1.5.0 and
bytes 1.12.1. The short-age probe observed 1 allocation versus 2, with equal bytes
and sensitivity; cache-02 records the decision and measurement limits. No runtime
code changed and no throughput improvement is established.

Independent plan review passed. Fresh serial response_cache tests produced
55 passes and one existing backend-policy admission fixture failure, matching
verification-01-cache-policy-fixture.md. The separate lifecycle selection passed
all seven tests (overlapping the broad selection). Formatting and workspace
check/Clippy passed with existing unrelated warnings. No live integration ran.

## Transcoder terminal outcome lock

GrpcTranscodingPlan::finalize_log now normalizes Pending to Interrupted and
publishes the existing terminal log under one outcome lock. The shared private
transition also serves response cleanup. Normalization still precedes the logged
gate, and the guard still spans status publication and log traversal. First
validated status, decisive failures and repeated-finalization behavior remain.

The sole production caller in pg_logging awaits finalize_ordinary_incomplete
before terminal logging. Response settlement and cleanup share the driver mutex,
and cleanup marks the processor finished. This lifecycle makes the removed
inter-lock window unnecessary; arbitrary concurrent call traces are not claimed
equivalent. No body owner, deadline, log contents or observability surface changes.
The benefit is one fewer mutex acquisition, without a measured throughput claim.

Independent plan review passed. The new outcome/status/idempotence matrix passed
against unchanged production. Fresh serial verification passed all 27 Gateway
transcoder tests and nine pg_logging tests. No live integration benchmark ran.
Formatting, workspace check/Clippy and seven repository guards passed; existing
unrelated Clippy warnings remain.

## Terminal processor role and guardrail summary audit

Logging can enter semantic/trailer finalizers on a nonempty driver lacking that
particular role. These methods still acquire the shared processor mutex. Body-06
records potential presence gating, the removed synchronization boundary and the
membership invariant requiring review; no runtime fast path was introduced.
Current consumers inspected do not change role membership, although lease APIs
expose mutable containers. No reachable race or measured net benefit is claimed.

GuardrailSummary contains only counters and static string references. Its clone
is not a heap-payload copy; published summaries remain first-wins through OnceLock
and emission remains once-only through the atomic gate. No speculative borrowing
change was selected there. Independent plan review approved this docs-only scope.
Fresh serial verification passed 72 tests: three response-driver, 34 AI HTTP and
35 semantic-guard tests. Formatting, workspace check/Clippy and agent-doc checks
passed with existing unrelated Clippy warnings. These tests do not prove the
proposed lock elision safe; no new concurrency probe or live transport test ran.

## OTLP terminal logger construction audit

AccessLogger moves its serialized payload to the first healthy sender; OTLP keeps
method/path metadata borrowed until constructing the queued record. The SDK
logger is currently acquired for every emit. Log-03 records potential per-pipeline
reuse and its shutdown equivalence boundary: new logger acquisition can select a
no-op provider after shutdown starts, while an existing logger visits its retained
provider processors. No live race or throughput benefit is asserted.

The static scope name is borrowed, provider acquisition clones an Arc, and SDK
batch enqueue independently clones its record into a boxed item. Logger caching
must not be advertised as eliminating that payload copy. No runtime change was
selected. Independent plan review passed. Fresh serial verification passed 27
tests: 19 OTLP provider and eight access-logger tests. Formatting, workspace
check/Clippy and agent-doc validation passed with existing unrelated warnings.
No live OTLP transport test or concurrent shutdown experiment ran in this batch.

## Kafka existing-topic sender lookup

The access-log Kafka path now looks up an existing topic with a borrowed name
and shared DashMap guard before falling back to the original entry path. A hit
clones the same channel sender without constructing a temporary topic String
or taking a shard write lock. The read guard is released before returning the
sender; a miss rechecks through entry, preserving concurrent creation dedup.
Misses now pay an additional read lookup; no net throughput claim is made.

Private topic_sender extraction preserves the caller's initialized-client
snapshot through enqueue on hits, and takes it only for a newly spawned producer.
Queue reservation still precedes lookup; topic admission, retirement recheck,
task ownership, full/closed errors and completion-gated eviction remain unchanged.
Independent plan review passed. The new network-free helper regression checks
channel identity, open/closed state, one topic slot and shutdown release. With
the extracted original entry path it failed only at the zero-allocation assertion
(observed one allocation). With borrowed lookup both measured hit calls allocate
zero; this is not a whole-send or broker-delivery claim.

Fresh serial verification passed 38 tests: 30 Kafka provider and eight access-
logger tests. No live Kafka broker, integration or throughput benchmark ran.
Formatting, workspace check/Clippy and seven repository guards passed with
existing unrelated Clippy warnings.

## Elasticsearch borrowed index-action serialization

The background bulk worker's canonical ndjson_header_line now serializes two
single-field borrowed structs instead of first building a serde_json Value tree.
This removes temporary map/key/index-name storage while retaining the same serde
escaping and owned output String. Partition overhead and body construction still
share the helper; body capacity, retry selection, date failure policy and health
remain unchanged. This is header construction work, not a per-document or
whole-request allocation/throughput claim. The owning security knowledge page
now describes the typed serializer and retains historical escaping evidence.

Independent plan review approved the four-file scope. Existing 60 Elasticsearch
tests passed before modification. The new six-input matrix passed exact-byte and
round-trip assertions on unchanged production, then failed its allocation
reduction assertion (first case: header=5, reference=5). After the change all six
inputs allocate less than their original Value-based reference, including empty,
quoted/escaped, control-character, permitted UTF-8 and long names.
Fresh serial verification passed 69 tests: 61 Elasticsearch and eight access-
logger tests. Formatting, workspace check/Clippy and seven repository guards
passed with existing unrelated warnings. No live Elasticsearch delivery or
throughput benchmark ran.

## Elasticsearch repeated header lifetime audit

The flush overhead calculation and every retry body construction still serialize
the same once-generated index. Log-04 records flush-level reuse as a potential
CPU/allocation saving, together with its longer encoded-header lifetime across
awaits. Dropping the raw generated name is an alternative; escaping expansion
and actual capacity must be included. No dedicated index-length cap was found
in provider schema/validation, and the transmitted-body cap does not bound the
initial header construction. All-oversized batches have no send await.

No runtime change was selected. Independent plan review approved the two-file
documentation scope. Fresh serial verification passed all 61 Elasticsearch tests.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No lifetime probe, live delivery or throughput benchmark ran.

## Downstream compression preference storage audit

Traced registration and captured-switch application into the locked fork's
request parser and response decision. Exact gzip pushes a nonzero-sized enum into
an initially empty Vec; compression-01 records inline storage as a fork-scoped
candidate with layout, overflow and parser-semantics tradeoffs. The general sfv
parser remains a separate owner. Seven enum variants do not bound repeated input
list length. No negotiation correction or dependency mutation was made.

Independent plan review approved the documentation scope. Fresh serial tests
passed 45 cases: two module-registration, 17 early-request and 26 response-filter
tests. Formatting, workspace check/Clippy and agent-doc validation passed with
existing unrelated warnings. These are Edgion gate tests, not a fresh allocation
probe, complete fork parser suite or compressed-wire delivery benchmark.

## gRPC-Web downstream bridge buffer audit

Traced the locked bridge's request/response header normalization and trailer
framing. Header rewriting uses global replacement, so no prefix-only shortcut was
selected. Trailer split/unsplit ownership is deferred in grpc-web-01: same-owner
adjacent buffers rejoin without payload copy, while growth can force copying.
A single-buffer alternative must include freeze metadata and retained capacities
in its measurements; no net allocation saving is established by source alone.

Independent plan review approved the two-file documentation scope. Fresh serial
verification passed 30 tests: two module-registration, 17 early-request and 11
upstream-trailer tests. Formatting, workspace check/Clippy and agent-doc validation
passed with existing unrelated warnings. No fork mutation, dependency update,
allocation probe or live gRPC-Web transport run occurred in this batch.

## Unary proxy-error owner borrow

The selected unary branch in `write_proxy_error_reply` now borrows the existing
`UnaryHead` handle instead of cloning its Arc. `error_reply` synchronously returns
owned headers and bytes before mutable final-response claiming and the session
write. Claiming changes only terminal bookkeeping; the context retains the owner
and reservation throughout the write and cancellation. No owner strong-count or
uniqueness-dependent consumer was found. This removes one Arc increment/decrement
on the selected error branch; no allocation or throughput improvement is claimed.

Independent plan review approved the three-file scope, including correction of a
stale Task 03 availability sentence in the related usage knowledge page. The
historical task is absent; its old acceptance is not fresh verification.
All 16 failure-reply tests passed before and after the change. Fresh serial
verification also passed 56 Chat-to-Anthropic converter tests (72 total after the
change). Formatting, workspace check/Clippy and seven repository guards passed
with existing unrelated warnings. No live provider or throughput benchmark ran.

## Transcoder terminal logging owner borrow audit

The terminal callback clones its plan to call a finalizer requiring the whole
mutable context. Grpc-05 records a narrower disjoint-borrow interface as a value
decision: one Arc increment/decrement versus changes across context, transport
and logging. A take/restore shortcut needs separate unwind ownership analysis.
No runtime change was made. Existing lock scope, first-match logging, validated
status publication and output cleanup remain intact.

Independent plan review approved the two-document scope. Fresh serial baseline
verification passed 36 tests: 27 transcoder and nine logging tests. Formatting
check, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No candidate implementation or performance measurement ran.

## gRPC timeout header storage audit

A proposed String move at upstream header insertion preserves bytes but has an
input-dependent storage tradeoff. A temporary probe using the actual formatter
and locked http/bytes versions confirmed pointer reuse; spare-capacity outputs
allocate 24 bytes of shared metadata during conversion, versus copying 2–9 bytes
for the sampled borrowed inputs. Exact-capacity samples avoid that allocation.
Grpc-06 records the measurements and broader evaluation requirements. No runtime
change or unconditional allocation/throughput improvement is claimed.

Independent plan review required this evidence before approving the revised
two-document deferral. Fresh serial baseline tests passed 30 cases: 15 upstream
request-filter and 15 timeout tests. Formatting check, workspace check/Clippy and
agent-doc validation passed with existing unrelated warnings. No wire suite ran.

## ExternalJump SNI copy deferral

External peer construction now borrows its request-owned preset SNI through
resolution and address validation, cloning only when constructing the HttpPeer.
The nonempty SNI no longer has an extra owned copy while DNS is pending, and
resolution failure or unsafe-address rejection avoids that copy entirely.
Successful peer construction still makes one owned SNI copy; no throughput or
successful-path allocation-count improvement is claimed.

The preset remains in ctx.jump, backend-context seeding touches a separate field,
and rejection branches return before later SNI use. DNS selection, SSRF checks,
failure attribution, source tagging, timeout application and peer ownership keep
their ordering. Cancellation drops a borrow instead of a temporary plain String.

Independent plan review approved the two-file scope. All 104 upstream-peer tests
passed before and after modification; fresh serial network-helper verification
passed 14 tests (118 total after modification). Formatting, workspace check/Clippy
and seven repository guards passed with existing unrelated warnings. Existing
resolver tests include IP literals, localhost and an empty host; no external DNS
integration, live upstream transport or performance benchmark ran.

## Prepared Service application protocol

Runtime Service section materialization now resolves the canonical appProtocol
enum once. Ordinary LB and ExternalName peer construction read that prepared
value; DirectEndpoint validation copies it into the retry preset instead of
cloning a raw protocol Arc. The parser and execution-time ALPN/WebSocket behavior
are unchanged. Raw ServicePortIdentity values remain intact for equality, proof
and existing response-cache key semantics, including distinct unknown strings.
No live store read or new request state is introduced; the immutable section
carries a small derived enum. On the tested 64-bit target, UpstreamJumpTarget
shrinks from 80 to 72 bytes while its boxed base-context slot remains eight bytes.
No throughput result or overall heap-reduction claim is made.

The publication regression now covers all six aliases, mixed case, absent/empty
and unknown values, raw identity/proof distinctions and retained old sections
across replacement. Existing DirectEndpoint assertions now compare the prepared
enum while retaining address, TLS, protocol, retry and allocation guarantees.
A baseline allocation assertion included first-use context initialization (six
calls versus one); warming and dropping one context before measurement passed
all seven jump tests on unchanged production code. Its measured context/jump
construction and exact allocation assertions remain. The representation change
then required the exact jump layout assertion to change from 80 to 72 bytes.

Independent plan review approved the implementation and narrow additional test
and architecture-document updates. Fresh serial verification passed 180 tests:
41 discovery, three validation, seven jump, 104 upstream-peer, 12 selector and
13 DirectEndpoint plugin tests. Formatting, workspace check/Clippy and seven
repository guards passed with existing unrelated warnings. No full integration
suite, live DNS/provider transport or performance benchmark ran.

## Move selected Service LB policy into attempt metadata

EndpointSlice peer construction now consumes the already-owned ServicePeerInput
LB policy and moves it into UpstreamInfo. The initial captured-policy copy remains;
the helper no longer clones that value a second time. Named ConsistentHash policies
therefore retain their existing header/cookie/query-name buffer in the attempt.
Scalar policies and absent policy retain their values. Selection, test-mode hash
extraction, address rejection, outlier ownership, TLS metadata and peer creation
keep their ordering; the external-DNS path is unchanged.

The new network-free duplex regression checks seven policy cases against existing
serialization and peer/upstream metadata before checking buffer identity for the
three nonempty named policies. On unchanged production all semantic assertions
passed and the final transfer assertion failed. After the move the full matrix
passes. No throughput or whole-request allocation-count measurement is claimed.

Independent plan review approved the three-file scope. Fresh serial verification
passed 12 tests: the new matrix, four selected-input tests and seven LB attempt
accounting tests. Formatting, workspace check/Clippy and seven repository guards
passed with existing unrelated warnings after correcting a new test-only lint.
The known native DNS/TLS fixture issue remains in verification-03; this batch did
not rerun or claim the full native-dispatch suite or live integration coverage.

## Selective captured LB policy copying

ServicePeerInput now carries only borrowed Service/TLS identity. The owned LB
policy copy is created inside the EndpointSlice branch, immediately before
selection and subsequent transfer into upstream metadata. Self-resolved
ExternalName and earlier TLS/discovery/path failures no longer copy a named
ConsistentHash value they never use. EndpointSlice still owns one policy copy.

The source remains backend_context.effective_policy from the request capture,
not a live policy store. No await occurs before this branch; discovery cache
recording only touches response-cache state, and Service capture, path selection
and degrade-policy reads cannot replace the captured policy. Existing selection,
sticky behavior, metadata ownership and error ordering are preserved.

Independent plan review approved the four-file scope. A new allocation regression
passed identity/snapshot assertions on old production and then failed with one
allocation versus zero; after the change input construction has zero calls and
requested bytes. The HTTP/gRPC actual dispatch matrix now supplies a named
captured policy and verifies both retained snapshot identity and EndpointSlice
versus self-DNS upstream policy values; it passed before and after the change.

Fresh serial verification passed 14 tests: five selected-input, one actual
HTTP/gRPC dispatch matrix, one policy-transfer matrix and seven LB accounting
tests. Formatting, workspace check/Clippy and seven repository guards passed
with existing unrelated warnings. The broader native-dispatch fixture issue in
verification-03 remains separate. No whole-request allocation count, live DNS
transport, full integration run or throughput improvement is claimed.

## Borrow Service selection request inputs after sticky handling

The private sticky-selection helper now passes an immutable request context to
its ordinary-selection callback after consuming any sticky target. EndpointSlice
ConsistentHash selection borrows the original RemoteIp instead of cloning its
String before that helper. Nonempty RemoteIp therefore loses one temporary
String allocation/copy per such attempt, including attempts resolved by sticky
handling. Other algorithms retain the empty input. Query access is pure and now
occurs only for ordinary selection. The test-mode metadata copy is unchanged.

Sticky target consumption, eligibility, captured membership, error propagation
and the caller's strict/non-strict fallback ordering remain unchanged. The callback
is synchronous and its endpoint owns its existing Arcs; no request borrow escapes.
Independent plan review approved this three-file scope.

Fresh serial verification passed 118 tests: one new borrowed-input matrix, one
existing sticky dispatch regression, 12 selector tests and 104 upstream-peer tests.
The matrix covers IPv4, IPv6 and empty input with absent or empty sticky context,
checks original buffer identity and hash bytes, exactly-once ordinary selection,
and unchanged persistence state. The existing sticky regression also passed on
unchanged production. Formatting, workspace check/Clippy and seven repository
guards passed with existing unrelated warnings. No whole-request allocation
count, throughput benchmark or live integration run is claimed.

## Borrow RemoteIp while preparing test hash metadata

The opt-in test-metrics ConsistentHash branch in build_peer_from_lb_selection
now borrows RemoteIp while extracting the hash key. It no longer makes a
nonempty temporary String copy before constructing the retained owned hash_key.
The retained metadata copy, metrics gate, hash bytes, UTF-8 handling, TLS writes
and upstream policy transfer are unchanged. This benefit applies only when that
test-metrics branch executes; ordinary disabled traffic already skips it.

Independent plan review approved the three-file scope. The existing peer matrix
now runs all seven policies with test metrics both disabled and enabled. It
checks the IPv6 SourceIp hash, empty missing named keys, absent non-ConsistentHash
metadata, unchanged source buffer and existing peer/policy ownership assertions.
The expanded matrix passed before and after the production change. Buffer identity
checks establish preserved ownership; removal of the temporary clone is established
by the source diff, not by an allocation-counter or throughput measurement.

Fresh serial verification passed 117 tests: the matrix, 12 selector and 104
upstream-peer tests. Formatting, workspace check/Clippy and seven repository
guards passed with existing unrelated warnings. No live integration test or
whole-request performance claim is included.

## Borrow LB test metric strings during JSON serialization

Logging's TestData now borrows the current upstream IP and request hash key
until synchronous JSON serialization completes. Its sole production constructor
uses as_deref instead of cloning those optional Strings. The output JSON remains
owned, and Retry error formatting retains its owned String. Field order,
skip_serializing_if behavior, empty-data output, latency sampling and the
integration-testing-mode gate are unchanged. The integration harness has its
own deserialization type and does not consume this borrowed projection.

The source change removes up to two nonempty temporary payload copies in enabled
LB test metrics. It does not change ordinary production-mode metrics or claim
whole-request zero allocation or measured throughput improvement.

Independent plan review approved the four-file scope. The new exact JSON
regression passed before and after implementation, covering populated IPv6 and
escaped keys, empty versus absent fields, disabled modes, Retry count/error and
missing latency. Fresh serial verification passed ten logging and three metrics
tests. Formatting, workspace check/Clippy and seven repository guards passed
with existing unrelated warnings. No live integration or performance benchmark ran.

## Rejected circuit-breaker admission owner audit

Rechecked upstream_peer admission through CbPermit and the endpoint atomic
counter. Success retains the copied backend Arc in the RAII permit; rejection
drops it. Borrowing until admission succeeds could remove one reference-count
pair only on rejection, with no payload allocation saving. The current whole-
context accessor and release-before-acquire ordering need a reviewed borrow
arrangement. This is distinct from the earlier Service-key String optimization.

Recorded the value/API choice in lb-02-rejected-permit-owner-copy.md; no runtime
change was made. Independent plan review approved the two-document scope.
Fresh serial baseline verification passed five permit and 104 upstream-peer
tests, formatting check, workspace check/Clippy and agent-doc validation with
existing unrelated warnings. This does not verify a future borrowed API or
claim saturated-load throughput improvement.

## Exact capacity for session-persistence cookies

build_set_cookie_value now reserves the final output byte length once before
writing the existing cookie format. Optional Max-Age is still emitted only for
Permanent lifetime with a captured TTL; its decimal length includes zero and all
u64 boundaries. The existing formatter renders the number. Attribute order,
Secure gating, generated token, clock sampling, response injection and borrowed
header append are unchanged. This does not precompute expiry or alter sticky state.

Independent plan review approved three files. A differential matrix exercises
6,048 name/token/lifetime/TLS/TTL combinations, including absent and empty names,
long inputs, all powers-of-ten boundaries, zero and u64::MAX. All output comparisons
passed on original production before the allocation assertion failed with three
calls versus one in its first case. After the change, every matrix case uses one
allocation call requesting exactly its final output byte length. This measures
only cookie construction, not the subsequent header conversion or whole request.

Fresh serial verification passed 23 session-persistence tests and four direct
upstream-response-filter tests, including captured-lifetime response issuance.
Formatting, workspace check/Clippy and seven repository guards passed with existing
unrelated warnings. No throughput benchmark or live integration run was performed.

## Transfer the exact-capacity sticky cookie into HeaderValue

The response Cookie branch now moves its completed String into append_header
instead of borrowing it for another payload copy. The pinned Pingora append API
uses TryInto<HeaderValue>; http 1.5.0 copies borrowed Strings but transfers owned
Strings into bytes 1.12.1. Because the cookie builder now has capacity equal to
length, that conversion takes the boxed-slice path without Shared metadata
allocation. The ordinary session Header token branch is unchanged: its capacity
has not been given the same guarantee.

Independent plan review approved three files. The existing 6,048-case cookie
matrix now also checks capacity equality, original buffer identity, identical
header bytes and zero conversion allocation calls/requested bytes. A separate
matrix compares owned and borrowed conversion for ordinary, tab, permitted UTF-8
and invalid control-character values. The actual HTTP/gRPC response-issuance
regression passed before and after this change. Token/expiry sampling, header
append order, ignored append errors, sticky state consumption and local-reply
replacement behavior are unchanged.

Fresh serial verification passed 24 session-persistence and four direct
upstream-response-filter tests, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. The measured zero-allocation
claim covers String-to-HeaderValue conversion only; header-map work and the full
callback were not allocation-profiled. No throughput or live integration claim
is made.

## Stable session-token hash preparation audit

Response issuance and lazy affinity-map construction share canonical address
formatting and truncated SHA-256 derivation. Ordinary Service candidates use the
unscoped form; optional scoped views retain bounded uncached construction.
Only new/refreshed response tokens incur issuance work, so preparing immutable
endpoint hashes trades that cost against resident metadata and publication CPU.
Recorded the representation/value decision in lb-03-prepared-session-token-hash.md.
Expiry remains issuance-time state; no runtime or hashing contract was changed.

Independent plan review approved the two-document scope. Fresh baseline checks
passed 24 session-persistence, seven LB runtime-state and four direct response
filter tests, formatting check, workspace check/Clippy and agent-doc validation.
Existing unrelated warnings remain. Future metadata layout and saturated or
ordinary throughput benefits have not been measured.

## Borrow already-normalized HMAC parameter keys

The synchronous Authorization parser now stores Cow keys in its temporary map.
Trimmed keys without ASCII uppercase bytes borrow the input; keys needing ASCII
folding retain the existing owned lowercase conversion. Hash/equality/remove use
the same str content. Empty-key rejection, duplicate-before-value error ordering,
unknown quoted-value validation and final owned authentication outputs are unchanged.
No borrow reaches authentication side effects or asynchronous body capture.

Independent plan review approved four files, including the owning wire-contract
knowledge page. A valid four-parameter lowercase/uppercase comparison passed all
semantic assertions on original production, then failed its expected allocation
delta: both originally allocated 12 times. After the change the lowercase form
requires four fewer allocation calls and 33 fewer requested bytes than its
otherwise identical uppercase form. Case-folding regressions retain mixed-ASCII
non-ASCII duplicate rejection and distinct uppercase/lowercase accented bytes.
Uppercase keys still allocate and perform a preliminary ASCII scan; no measured
throughput benefit or whole-parser zero-allocation claim is made.

Fresh serial verification passed 55 HMAC-filtered and 68 condition-system tests
(the filters overlap), formatting, workspace check/Clippy and seven repository
guards with existing unrelated warnings. No live integration suite was run.

## Iterate HMAC authorization segments without a temporary Vec

The sole production consumer of split_auth_params processes segments once in
forward order. The helper now yields trimmed slices using the same quote/escape
state transitions through str::split. Empty input, trailing delimiters, escaped
commas and unfinished quotes retain their segmentation; quoted-value validation
and parser error precedence remain with the existing caller. Early rejection
omits only an unobserved, side-effect-free scan of remaining input.

Independent plan review approved the four-file scope. A differential unit test
checks the former scanner against 9,331 exhaustive short strings plus five longer
boundary cases. All semantic comparisons passed on original production before
the final zero-allocation assertion failed as expected. The iterator passes both
semantics and zero allocation calls/requested bytes during segment consumption.
Other parser structures and outputs still allocate; no throughput claim is made.

Fresh serial verification passed 56 HMAC-filtered and 68 condition-system tests
(the filters overlap), formatting, workspace check/Clippy and seven repository
guards. Existing unrelated Clippy warnings remain. The owning HMAC wire-contract
knowledge records the iterator boundary. No integration suite was run.

## HMAC signing-buffer audit

The current verifier constructs the complete canonical message before initializing
its MAC. Fragment updates could remove that temporary copy, but variable header
counts and late missing-header rejection create an unmeasured allocation/CPU
tradeoff. Recorded the decision, canonical byte/error boundaries and measurement
criteria in hmac-03-incremental-signing-input.md. Existing CSRF fixed-fragment
precedent does not establish this path's CPU cost; no runtime change was made.

Independent plan review approved two documentation files. Fresh baseline checks
passed 56 HMAC-filtered and 68 condition-system tests (overlapping filters),
formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated warnings remain. No throughput measurement or integration run is claimed.

## Borrow the live query during credential stripping

The shared KeyAuth/JwtAuth strip_query_param helper now uses request_query to
borrow the same current URI previously copied by get_query. Its joined replacement
is owned before mutation. Decoded-key matching, no-match no-write behavior, raw
surviving pairs, matched empty-pair filtering and ignored setter failures remain
unchanged. Frozen credential extraction and upstream path selection are unaffected.
Only the whole-query input copy is removed; decoding and output construction still
allocate, and no throughput improvement is claimed.

Independent plan review approved five files. The new 18-case boundary/error matrix
passed against original production before the accessor switch; the final version
forbids the owned getter. Fresh verification passed 29 shared-auth, 14 KeyAuth,
47 JwtAuth, two real-adapter stripping and one live-query ownership test (93 total).
Formatting, workspace check/Clippy and seven guards passed with existing unrelated
warnings. The two owning query-stripping knowledge pages now describe the borrowed
accessor. No integration suite was run.

## Compare plain query credential names without decoding storage

The shared credential-strip filter now compares names without percent or plus
markers directly. The canonical decoder is identity on these strings; marked
names still decode unconditionally, including when raw text equals configuration.
No secondary decoder or backend selector raw-equality rule was introduced.
Surviving query bytes, duplicate removal, no-match behavior and error policy stay
unchanged. Encoded names, survivor storage and rewritten output can still allocate.

Independent plan review approved four files. The expanded 30-case semantic matrix
passed on original production. The new short/long plain-name allocation comparison
then failed as intended: original totals were 3 calls/136 bytes versus 7 calls/376
bytes, including common mock/container work. After the change those totals are
identical; no key-sized temporary storage remains for plain names. This is not
whole-helper zero allocation or measured throughput evidence.

Fresh verification passed 30 shared-auth, 14 KeyAuth, 47 JwtAuth, two adapter strip
and one live-query ownership tests (94 total), formatting, workspace check/Clippy
and seven guards. Existing unrelated warnings remain; no integration suite ran.

## Transfer the merged request Cookie into its sensitive header

The shared Cookie stripper now moves its merged String into HeaderValue rather
than copying it through from_str. A private typed writer owns sensitivity marking
and session dispatch; the existing public string writer delegates to it without
changing caller signatures. Conversion failure and setter failure remain ignored
by cookie stripping, and absent/remove/replace planning is unchanged. KeyAuth,
JwtAuth and OIDC all consume this shared path.

Independent plan review approved four files. Before the source change, the new
nine-case conversion matrix and existing shared-auth suite passed (31 tests).
The matrix verifies exact capacity, original buffer identity and zero allocation
calls/requested bytes during owned conversion, plus borrowed/owned validation
parity for UTF-8, tabs and internal invalid controls. A new typed-writer test
checks sensitivity, pointer identity and setter error propagation. The claim is
limited to removing the merged-cookie payload copy, not whole-callback allocation
or throughput. Header-map insertion and cookie planning still have their own costs.

Fresh serial verification passed 32 shared-auth, 14 KeyAuth, 47 JwtAuth, 19
BasicAuth, 56 HMAC-filtered and four OIDC cookie-strip tests (172 total), formatting,
workspace check/Clippy and seven guards. Existing unrelated warnings remain.
No live integration suite was run.

## Query and Cookie survivor-storage audit

The shared stripping helpers still collect borrowed survivor slices before joining
output. Removing their Vec storage trades against repeated parsing or String
growth/retained capacity; Cookie output capacity also controls the pinned bytes
conversion's metadata allocation. Query no-match delayed materialization remains
a separately evaluable candidate. Recorded these choices and exact semantic and
measurement boundaries in auth-01-strip-survivor-storage.md; no runtime changed.

Independent plan review approved two documents. Fresh baseline checks passed 32
shared-auth, 14 KeyAuth, 47 JwtAuth and four OIDC cookie-strip tests (97 total),
formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated warnings remain; no alternative implementation was benchmarked.

## Build exact-capacity authentication challenges and transfer ownership

The canonical gateway auth-error response now assembles WWW-Authenticate with an
exact-capacity private builder and moves that String into the header. Realm
validation/escaping retains its position before response construction; optional
error-description escaping, raw scheme/code bytes and both wire forms are
unchanged. Header/body/shutdown sequencing and response payloads remain unchanged.
This removes the challenge payload copy and avoids spare-capacity Shared metadata
on the pinned http/bytes conversion path.

Independent plan review approved four files. Five existing realm-filtered tests
passed before editing. A new 1,680-case matrix compares the old formatting bytes,
exact capacity, owned/borrowed rejection and valid buffer identity, measuring zero
conversion allocations. Selected clean-description construction needs one exact
output allocation; escaping descriptions can allocate separately, and None does
not process the body. No whole-response zero-allocation or throughput claim is made.

Fresh verification passed 34 shared-auth, 14 KeyAuth, 47 JwtAuth, 19 BasicAuth,
56 HMAC-filtered, 20 JweDecrypt and one LDAP unauthorized-outcome test (191 total),
formatting, workspace check/Clippy and seven guards. Existing unrelated warnings
remain. The owning security realm knowledge records this ownership boundary;
no live integration suite was run.

## Exact-capacity authentication error body

The canonical auth-error sender now builds its unchanged decimal-status, space
and raw-body payload with exact String capacity before transferring it to Bytes.
The builder uses standard primitive formatting and decimal width, not a new
integer encoder. Construction remains after successful response-header writing;
body EOS, error propagation and shutdown placement are unchanged. The non-auth
plain-error helper and challenge construction are unaffected.

Independent plan review approved three files. Against the original format-based
body builder, all 5,454 semantic comparisons passed before the final allocation
assertion failed. The optimized complete Bytes construction allocates once and
requests exactly its output length across every fixture, including all three-digit
status values, other u16 decimal-width boundaries, raw controls, UTF-8 and long
bodies. Wider private-helper inputs do not expand accepted HTTP status codes.
Separate header/body write-failure cases passed before and after the change,
confirming no subsequent body or shutdown call after the relevant failure.

Fresh verification passed 36 shared-auth, 14 KeyAuth, 47 JwtAuth, 19 BasicAuth,
56 HMAC-filtered, 20 JweDecrypt and one LDAP unauthorized-outcome test (193 total),
formatting, workspace check/Clippy and seven guards. Existing unrelated warnings
remain. The one-allocation claim covers payload construction only; no complete
response allocation or throughput claim and no live integration run is made.

## Prepared authentication challenge audit

Realm validation/escaping and challenge construction still run on failure replies.
Preparing stable state could reduce that work but requires a canonical runtime
owner, explicit direct-constructor failure behavior and generation-lifetime costs.
Dynamic Bearer descriptions remain request-bound. Recorded the decision and
measurement/acceptance boundaries in auth-02-prepared-challenge-generation.md.
This is separate from the completed exact-capacity ownership changes.

Independent plan review approved two documents. Fresh baseline checks passed 36
shared-auth, 14 KeyAuth, 47 JwtAuth, 19 BasicAuth and 56 HMAC-filtered tests (172
total), formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated warnings remain; no prepared implementation or performance result is
claimed, and no runtime code changed.

## Server header replacement audit

The prepared Server/HSTS helper removes then inserts each field. Direct insertion
replaces the same values and casing in the pinned fork, but a fresh standalone
probe found equal HeaderMaps with different H1 field ordering. Deferred the
observable change and unmeasured performance value in response-04; runtime code
remains unchanged. The helper also serves local/error and AI responses, so future
acceptance must preserve listener options and provider-reset ordering.

Rechecked numeric cache-header construction against the locked http artifact:
short numeric HeaderValue construction requests two allocations / 44 bytes,
while the current owned String path requests one exact-length allocation. This
confirms existing cache-02 evidence; no duplicate task or numeric rewrite was added.

Independent plan review approved two documents. Fresh baseline checks passed
26 response-filter and 11 upstream-response-filter tests (37 total), formatting
check, workspace check/Clippy and agent-doc validation. Existing unrelated
warnings remain. No runtime alternative, live integration or throughput benchmark
was run; the serialization probe demonstrates the disputed observable difference.

## Consume response-stage plugin results

The shared response-stage termination helper now consumes the otherwise-unused
plugin result. HTTP Bytes bodies move into the local-reply owner and optional
gRPC messages move into the exact-status denial owner, avoiding a redundant
body clone and message copy. Both response runners and their dependency-denial
calls use this handoff. Refusal predicates, logs, status updates and writer order
remain unchanged. A moved String keeps its original capacity until delivery;
there is no blanket retained-memory or throughput claim.

Independent plan review approved four files. Ten existing response-stage tests
passed before changes. New real-adapter ownership assertions failed against the
old helper at the expected two points: the gRPC message pointer changed and the
accepted HTTP body retained a redundant owner. Existing byte/status assertions
passed before those failures. The owning response-stage denial knowledge records
the transfer boundary and its limits.

Fresh verification passed 178 session-adapter, 123 plugin-runtime-filtered,
26 response-filter, 11 upstream-response-filter and 16 fail-to-proxy tests (354
total), formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. No full workspace test or live integration suite was run.

## Transfer the gRPC-Web terminal status buffer

The shared terminal frame writer now transfers its decimal grpc-status String
into HeaderValue on the gRPC-Web branch instead of parsing a borrowed string
and copying its bytes. Native gRPC, message encoding, conflict-field removal,
header/trailer order and error propagation retain their existing paths.

Independent plan review approved three files. A standalone allocator probe linked
the locked http artifact and compared every u16 value (65,536 cases). Decimal
Strings had exact capacity, values and unset sensitivity matched, and owned
conversion retained the input pointer with zero conversion allocations. Complete
formatting plus conversion requested one allocation of the output length instead
of two allocations totaling twice that length. These are requested allocations
on this build, not physical heap size or a throughput measurement; wider u16
probe inputs do not expand the supported gRPC status vocabulary.

The existing real duplex/bridge wire test now covers status 0, 8 and 16, keeping
queued conflicting status/message removal and ordinary-header preservation
assertions. All three cases passed against the original writer before editing.

Fresh verification passed 15 server error-response, 16 fail-to-proxy and 178
session-adapter tests (209 total), formatting, workspace check/Clippy and seven
guards. Existing unrelated warnings remain. No live integration or throughput
benchmark was run; the byte-level cases use the existing in-process bridge fixture.

## Terminal gRPC message storage audit

The gRPC-Web message writer copies its encoded String through parsing, while
native gRPC already transfers it. Unlike decimal status strings, the canonical
encoder can retain spare capacity. Fresh complete-construction probes show a
workload-dependent tradeoff: direct transfer helps exact-capacity messages but
can retain spare capacity and request Shared metadata for short/escaped messages.
Deferred this decision and shared-encoder alternatives in grpc-07; no runtime
code changed and no throughput or actual retained-heap reduction is claimed.

Independent plan review approved two documents. Fresh baseline verification
passed 15 server error-response, 16 fail-to-proxy and 178 session-adapter tests
(209 total), formatting check, workspace check/Clippy and agent-doc validation.
Existing unrelated warnings remain; no live integration or alternative encoder
benchmark ran. Probe totals include encoding and conversion, unlike grpc-06's
conversion-only evidence.

## Single-pass response-cache contract metadata validation

The stored-header contract checks now collect the first case-insensitive matches
for their fixed field sets during one complete metadata framing pass. This
replaces a full validity scan followed by two or three separate field lookups.
Fixed borrowed slots add no heap storage. Final transport still validates its
singleton Date first; supported-header policy, missing/empty distinctions,
malformed-tail rejection, freshness, leases and publication order are unchanged.

Independent plan review approved four files. The initial differential matrix against the
original repeated-scan algorithm passed on the old implementation before editing.
It covers reordered/mixed-case/missing/duplicate fields, empty values, all metadata
prefix truncations, malformed tails, changed/duplicate/forbidden response headers,
and the final-transport Date allowance, with zero allocation assertions. A
standalone optimized probe sampled three, 16 and 64 fields with early/late contract
fields and supported reduced local parser work; it is not a request-throughput
benchmark or an unconditional CPU claim for all metadata shapes.

Final review added explicit malformed suffixes after all three contract fields
are present, retaining the same old-algorithm oracle and zero-allocation checks.

Fresh final verification passed 21 cache-storage and seven cache-lifecycle tests
(28 total), formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. The broader cache policy-fixture issue recorded in verification-01
was not changed or claimed resolved; no live integration or full test suite ran.

## Batch response-cache header-name digest updates

The canonical header-name digest now lowercases input bytes into a fixed 64-byte
stack buffer and feeds each populated slice to SHA-256. The domain prefix and
resulting byte stream are unchanged. HeaderCertificate and VaryCertificate retain
one shared implementation, with the same validation, capacity, deduplication,
sorting and permission rules. No heap buffer or retained state was added.

Independent plan review approved four files. A 287-input differential matrix
against the original bytewise updater passed before editing. It covers empty and
mixed-case names, punctuation/control bytes, lengths around chunk boundaries,
long inputs and UTF-8 encodings crossing the 64-byte boundary, while asserting
zero allocations. Helper-only invalid-name fixtures do not broaden admission.
Name-part digest updates decrease from n to ceil(n / 64); short inputs also gain
stack initialization/copy work, so no universal CPU or throughput claim is made.

Fresh verification passed seven cache-key, 21 cache-storage and seven
cache-lifecycle tests (35 total), formatting, workspace check/Clippy and seven
guards. Existing unrelated warnings remain. The broader cache policy-fixture
failure in verification-01 remains outside this change; no full test suite,
live integration or throughput benchmark was run.

## Response-cache Vary validation reuse audit

Traced duplicate canonical Vary parsing within successful capture, decode,
replay-charge, seal and publication checks. Each boundary still needs fresh
validation; only same-header work within one call is a reuse candidate. The
parser already uses bounded fixed storage, so benefit would be CPU work avoided,
not heap allocation removal. Deferred the API/ownership tradeoff and workload
measurement requirements in cache-03; no runtime code changed.

Independent plan review approved two documents. Fresh baseline verification
passed seven cache-key, 21 storage, seven lifecycle and four policy tests (39
total), formatting check, workspace check/Clippy and agent-doc validation.
Existing unrelated warnings and verification-01's broader admission fixture
remain outside this change. No benchmark or live integration ran.

## Reuse cache-key header value lookup

Builder::headers now retains one borrowed GetAll result per included header name
for both value counting and ordered framing. In locked http 1.5.0, get_all resolves
the map index while its iterators reuse that index. This removes a duplicate
source-level lookup without changing the value-count prefix, insertion order,
sorted names, certificate construction, exclusions or material-limit failures.
Both old and new paths allocate no value collection; no throughput or allocation
reduction is claimed.

Independent plan review approved two files. All seven existing cache-key tests
passed before editing; they cover repeated-value order, distinct length framing,
excluded fields, material/header limits and zero-allocation construction. No new
test-only abstraction was introduced for this mechanical reuse.

Fresh verification passed seven cache-key, 21 storage and seven lifecycle tests
(35 total), formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. No full test suite, live integration or throughput benchmark ran;
verification-01's broader cache admission fixture remains outside this change.

## Reuse configured cache variation exclusions

RequestState::bind now parses the canonical borrowed Connection exclusion index
once for a nonempty configured variation loop, after any asynchronous request-body
scan. Each field uses that local index without crossing an await, header mutation
or callback boundary. Empty lists still skip parsing. Grammar bounds, exclusion
case behavior, key framing and header-certificate updates are unchanged. The
integration-testing flag follows its existing startup-only contract.

Independent plan review approved five files. Existing key tests now exercise
multiple allowed/excluded names against one index and retain grammar, capacity
and zero-allocation coverage. This removes repeated bounded parsing for multiple
variations; it adds no heap allocation and makes no throughput claim.

Fresh verification passed seven cache-key, 21 storage and seven lifecycle tests
(35 total), formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. No full test suite or live integration ran; verification-01's
broader cache admission fixture remains outside this change.

## Existing outlier failure-state lookup

Traced fail_to_connect and error_while_proxy into identity-bound failure
completion. Existing endpoint state now uses a borrowed DashMap get before the
original atomic entry insertion fallback. The read guard ends before fallback
and before state accounting or percent arbitration. Both paths remain inside
the coordinator's membership/policy validation and endpoint completion lock;
retirement ordering, gauges and threshold-generation revalidation are unchanged.

A hit avoids an exclusive shard lookup and an owned identity clone, including
its Service-key Arc increment/decrement. That clone did not allocate a new heap
buffer. A miss adds one read lookup; no throughput improvement is claimed.
Accepted recovery and arbitration locking remains unchanged.

Independent plan review approved four files. The existing threshold regression
now also verifies state pointer identity across repeated failures. Fresh serial
verification passed 36 outlier-detection and 41 coordinator tests (77 total),
including concurrent percent caps and retirement checks, plus formatting,
workspace check/Clippy and seven guards. Existing unrelated warnings remain.
No full test suite, live integration or contention benchmark ran.

## Transfer the published outlier state handle

The successful failure-completion branch now moves its last local endpoint Arc
into set_outlier_ejected_for instead of cloning it. The runtime stores that same
state before publishing the deadline and ejected flag; the identity map retains
its owner. Completion locking, recovery, retirement and gauge ordering are unchanged.
This removes one Arc increment/decrement on successful publication, with no heap
allocation or throughput claim.

Independent plan review approved two files. Existing lifecycle and concurrency
regressions were sufficient for this last-use transfer. Fresh verification passed
36 outlier-detection and 41 coordinator tests (77 total), formatting, workspace
check/Clippy and seven guards. Existing unrelated warnings remain. No full test
suite, live integration or benchmark ran.

## Format ExtProc terminal summaries outside the lock

AttachedExtProc::take_summary_line now checks/sets emitted and copies its five
scalar/static-text fields under the summary mutex, then formats the same line
after releasing the guard. The snapshot and single-claim linearization remain
one critical section. Later summary updates cannot affect the claimed output.
State terminalization, owner cleanup and canonical access-log placement are
unchanged. This shortens the allocation-containing critical section; it does not
remove an allocation or claim measured contention/throughput gains or identical
thread scheduling.

Independent plan review approved three files. The existing terminalization table
was strengthened to compare exact summary text and repeated-claim refusal, and
passed against the original implementation before the production edit. Fresh
verification passed 143 ExtProc and ten terminal-logging tests (153 total),
formatting, workspace check/Clippy and seven guards. Existing unrelated warnings
remain. No full test suite, live interoperability or benchmark ran.

## Static status labels in terminal metric fallback

The dynamic backend-request counter and request-duration helpers now retain the
static lifetime of status_group and build the status label with
Label::from_static_parts. The sole production caller already receives static
status text; repository test callers use literals. Locked metrics 0.24.6 stores
these static strings directly. Each enabled status label avoids its former
String construction, up to two on a fallback request when the histogram and both
status labels are enabled. Other labels and label-vector allocations remain.
Dynamic metric ownership/expiry, config gates, label values/order and the route
handle fast path are unchanged; no measured throughput claim is made.

Independent plan review approved two files. Existing emitted-label, status-group,
handle/fallback parity and logging tests were sufficient for the mechanical
lifetime change. Fresh verification passed 36 metrics and ten logging tests
(46 total), formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. No full test suite, live integration or benchmark ran.

## Dynamic fallback metric capacity audit

Traced terminal fallback label construction through the locked metrics Vec/Key/Cow
ownership path. Fixed reservations exceed default emitted label counts, but the
ordinary prepared-handle path bypasses these builders. Deferred sizing and cold
preparation alternatives in metrics-01 pending workload, allocation and CPU evidence;
no runtime behavior changed. Independent plan review approved two documents.

Fresh baseline verification passed 36 metrics and ten logging tests (46 total),
formatting check, workspace check/Clippy and agent-doc validation. Existing unrelated
warnings remain; no full test suite, live integration or benchmark ran.

## Terminal classification and derived CORS value audit

Terminal ErrorSource classification copies only a payload-free enum; connection
reuse persists Box<()>, which needs no heap storage. Neither site warrants an
allocation-removal refactor. WAF terminal finalization updates scalar outcome and
borrows its retained access-log projection.

Traced EdgionPlugins CORS Max-Age formatting and revalidated the prior accepted
preflight-only cost. Recorded a narrower Gateway-owned preparation alternative
in cors-01 for user decision; no runtime or existing knowledge verdict changed.
Independent plan review approved two documents. Fresh baseline verification passed
45 Gateway CORS-filtered tests, formatting check, workspace check/Clippy and
agent-doc validation. Existing unrelated warnings remain; no full test suite,
live integration or benchmark ran.

## Local limiter initialization audit

Traced request-key resolution through OnceLock<Rate> and pinned Pingora estimator
construction/reset. Cold initialization is required by design rules, but directly
moving construction also moves the window clock and can change admission.
Recorded the unresolved timing/memory design in rate-limit-01; current lazy code
is not declared an accepted exception. Redis quota preparation remains sequential,
and dynamic rejection-header name ownership matches the prior pinned-API audit.

Independent plan review approved two documents. Fresh baseline verification passed
17 Gateway rate_limit_local-filtered tests, formatting check, workspace check/Clippy
and agent-doc validation. Existing unrelated warnings remain. No runtime change,
full test suite, live integration or benchmark ran.

## Instance-scoped local rate threshold lookup

The shared get_effective_rate entry now returns the configured rate directly for
Instance scope. Gateway request admission and quota-header callers no longer
perform an unused global instance-count atomic load in that scope. Cluster still
reads the live count and delegates to the existing pure formula; no threshold
cache, schema field, validation change or estimator timing change was introduced.
The supporting resource-crate edit is limited to this Gateway-consumed entry and
its existing unit test. No measured CPU or throughput improvement is claimed.

Independent plan review approved three files. The public-entry assertion passed
against the original implementation before the production edit. Fresh verification
passed 46 resource and 17 Gateway rate_limit_local-filtered tests (63 total),
including dynamic Cluster count changes, formatting, workspace check/Clippy and
seven guards. Existing unrelated warnings remain. No full test suite, live
integration or benchmark ran.

## CSRF expected-signature buffer ownership

CSRF verification now hex-encodes its expected SHA-256 digest into a fixed
64-byte stack buffer instead of a temporary String. Issuance retains its owned
lowercase hex signature through the same extracted digest helper. HMAC input
fragments, timestamp formatting, age gates, exact ASCII length/XOR comparison,
key handling and token wire shape remain unchanged. The fixed encoding assertion
follows the locked hex 0.4.3 exact-length contract. This removes one source-visible
hex String construction from verification; timestamp allocation remains, and no
stronger timing or measured CPU/throughput claim is made.

Independent plan review approved four files. Short, long, uppercase and non-hex
signature rejection cases passed on the original implementation before editing
production code. Fresh verification passed all 19 CSRF tests, including known
signature and fragment parity cases, formatting, workspace check/Clippy and seven
guards. Existing unrelated warnings remain. No full test suite, live integration
or benchmark ran. Updated the owning signature-input knowledge page.

## Borrowed Base64 certificate normalization input

HeaderCertAuth now borrows the trimmed Base64 certificate header when it has no
ASCII whitespace. Whitespace-bearing input retains the existing character filter
and owned normalization buffer. The detection scan and original filter recognize
the same ASCII set, preserving strict decoder bytes and all later error ordering,
DER verification and canonical identity extraction. URL and Connection modes are
unchanged. The common path avoids a complete temporary input copy; whitespace
input adds a scan to its first whitespace. No throughput claim is made.

Independent plan review approved three files. Existing tests already cover clean
and internally spaced/tabbed certificate headers, strict decoding, failure
precedence, trailing DER and CA selection. All 20 HeaderCertAuth tests passed
before and after the edit. Formatting, workspace check/Clippy and seven guards
passed; existing unrelated warnings remain. Updated the owning Base64 leaf
ownership knowledge. No full test suite, live TLS integration or benchmark ran.

## Header CA public-key preparation audit

Confirmed issuer-matched requests reparse the complete retained CA certificate.
The borrowed SPKI owner prevents a direct cold parsed-object move. A local locked
parser probe demonstrated that its raw slice need not contain the complete
accepted TLV, so copying and reparsing raw is not automatically equivalent.
Recorded ownership alternatives and acceptance in tls-03; the probe is not a
signed-certificate regression or trust-bypass claim. No runtime code changed.

Independent plan review approved two documents. Fresh baseline verification
passed 20 HeaderCertAuth tests, formatting check, workspace check/Clippy and
agent-doc validation. Existing unrelated warnings remain; no full test suite,
live TLS integration or benchmark ran.

## Borrowed OtherName text through SAN projection

The shared OtherName decoder now returns Cow text: BER string variants borrow
input, boolean/NULL borrow static text, and existing numeric/OID/BMP/Universal
conversions remain owned. The display projection formats directly from that
value, removing its previous intermediate text/boolean String. Typed projection
still owns every returned value and moves owned conversions. NULL previously
allocated no heap storage, and OID formatting remains owned. Exact byte/NUL,
wrapper/type, integer-bound, duplicate-order and separate DER-pass semantics are
unchanged. This is not a measured throughput improvement.

Independent plan review approved four files. Existing baseline tests passed
17 extractor, 20 HeaderCertAuth and 14 mTLS callback cases. A new UTF8String
input-pointer regression covers an embedded NUL; signed-certificate tests retain
independent expectations for both projections. Fresh verification passed 18
extractor, 20 HeaderCertAuth and 14 mTLS callback tests (52 total), formatting,
workspace check/Clippy and seven guards. Updated owning SAN projection knowledge.
Existing unrelated warnings remain; no full suite, live TLS integration or
benchmark ran.

## Borrowed OtherName OID through SAN projection

OtherName classification now borrows its parsed OID until synchronous projection.
Display formats it directly into the prefixed result, avoiding the previous outer
OID to_string temporary. Typed projection still formats and owns the OID exactly
as before. Both paths retain the same dependency Display behavior, including its
internal to_id_string allocations in asn1-rs 0.7.2. No allocation-free or measured
throughput claim is made. Neither parsed objects nor local DER backing escape;
formatting, SAN order, values and trust decisions remain unchanged.

Independent plan review approved three files. Existing signed mixed-SAN tests
cover multiple OIDs, duplicates and independent display/typed expectations, so
no redundant test was added. Baseline and final verification each passed 18
extractor, 20 HeaderCertAuth and 14 mTLS callback tests (52 total). Formatting,
workspace check/Clippy and seven guards passed. Updated the owning SAN knowledge;
existing unrelated warnings remain. No full suite, live integration or benchmark
ran.

## TCP selected-backend address borrow

TCP connection setup now borrows the selected backend's socket address for
formatting and synchronous endpoint safety validation, matching the existing TLS
paths. The formatted address still moves into the context before validation and
feeds BasicPeer through the same string API. Token capture, denial diagnostics,
connect options and backend ownership across outlier completion are unchanged.
This removes an address-enum clone; Internet address cloning was a value copy,
so no heap-allocation or measured throughput improvement is claimed.

Independent plan review approved three files. Existing tests cover selected-route
lifetime, admission, relay accounting and logs; no redundant test was added.
Baseline and final verification each passed 27 TCP, 21 shared relay and four TCP
log tests (52 total). Formatting, workspace check/Clippy and seven guards passed.
Updated the owning snapshot knowledge and its stale test-file location. Existing
unrelated warnings remain; no full suite, live integration or benchmark ran.

## L4 address round-trip audit

Traced TCP and both TLS paths through address rendering, prevalidation logging,
BasicPeer construction, redacted connect errors and conditional PP2 emission.
Recorded the shared structured-address alternative in l4-01. The pinned fork's
structured constructor is private, and direct address copying would retain IPv6
flowinfo currently removed by text normalization. A local std probe confirmed
that representation difference, without claiming a production endpoint defect.
Prevalidation non-Internet diagnostics and full IPv6 PP2 authorities remain
explicit acceptance boundaries. No runtime or fork code changed.

Independent plan review approved two documents. Fresh baseline verification
passed 27 TCP, 85 TLS and 21 shared-relay tests (133 total), formatting check,
workspace check/Clippy and agent-doc validation. Existing unrelated warnings
remain; no full suite, live transport integration or benchmark ran.

## ClientHello setup-buffer capacity audit

Revalidated SNI record/container gates and consume-and-replay ownership. The
fixed 16,384-byte initialized buffer retains capacity after truncation through
setup, then drops before relay as designed. Recorded small-buffer growth and
post-inspection compaction alternatives in tls-04, with read-batching, deadline,
trailing-data and allocator tradeoffs. No runtime change or whole-connection
retention claim is made.

Independent plan review approved two documents. Fresh baseline verification
passed 85 TLS tests, formatting check, workspace check/Clippy and agent-doc
validation. Existing unrelated warnings remain; no full suite, live socket
integration or benchmark ran.

## Listener specificity lookup audit

TLS candidate publication already prepares attachment indices and ordering;
its hot hostname predicate performs direct comparison, not compilation. Traced
the remaining specificity scan to shared route admission and Gateway-local
SNIHost isolation. Recorded indexing/query-reuse alternatives in routing-02,
including distinct scopes, unavailable-owner authority, exact/wildcard behavior
and immutable publication ownership. No runtime change was selected.

Independent plan review approved two documents. Fresh baseline passed 23 route
matching, 19 listener-store and 85 TLS tests. Request-filter tests passed 44 and
failed the known response-cache admission fixture recorded in verification-01;
that group is not reported as passing. Formatting check, workspace check/Clippy
and agent-doc validation passed. Existing warnings remain; no full suite, live
conformance or benchmark ran.

## UDP counters under the lifecycle lock

Moved the four session counters into the lifecycle state already locked by every
successful send result. Wrapping addition preserves atomic-counter overflow
semantics, and retirement still freezes accounting and activity together.
Terminal log construction snapshots counters under a short lock released before
formatting or awaiting. Normal shutdown still joins producers before logging.
This removes two atomic read-modify-write operations per accounted successful
datagram; no measured throughput improvement is claimed.

Independent plan review approved four files. The added regression first passed
on the atomic implementation, then passed after the ownership change: 59 UDP
and four UDP log tests in each run (63 total). Formatting, workspace check/Clippy
and seven guards passed. Existing unrelated warnings remain; no full suite,
live integration or benchmark ran.

## Exact-capacity sticky Header token transfer

Expiring wire tokens now reserve the exact hash, separator and decimal-expiry
length before using the existing formatter. Saturating expiry arithmetic is
unchanged; no-lifetime tokens still return their original hash buffer. The
response Header branch consumes the finished token. On the pinned http/bytes
versions, the generated bare hash and exact-capacity expiring token transfer into
HeaderValue without a payload copy or Shared metadata allocation. Header-map
work is outside this allocation claim. Cookie formatting, clock sampling,
retry placement, validation, append errors and sticky-state transitions remain
unchanged.

Independent plan review approved four files. Existing baseline tests passed
24 session-persistence and four response-issuance/helper tests. The new matrix
completed all 189 byte comparisons on the original implementation before its
capacity assertion failed (8 reserved bytes for a two-byte result). It covers
empty/ordinary/long hashes, decimal boundaries and saturating expiry. A separate
test covers generated bare-token transfer; existing conversion tests cover
owned/borrowed validation equivalence.

Final verification passed 26 session-persistence and four response-filter tests
(30 total), including actual HTTP/gRPC issuance. All 189 cases reserve one exact
buffer and convert it without another allocation, retaining byte identity and
buffer address. Formatting, workspace check/Clippy and seven guards passed with
existing unrelated warnings. No full suite, live integration or throughput
benchmark ran.

## gRPC trailer evidence representation audit

Traced original status capture through mutable trailer companions, the response
driver lease and transcoder settlement. Valid evidence currently constructs a
one-entry HeaderMap and is parsed again; invalid/missing evidence retains its
empty-map and outer presence semantics. Recorded typed-evidence alternatives
in grpc-08 because the shared public processor trait and terminal ownership
require an explicit scope decision. No runtime change was selected.

Independent plan review approved two documents. Fresh baseline passed 27
transcoder, 11 upstream-trailer and six response-callback tests (44 total),
formatting check, workspace check/Clippy and agent-doc validation. Existing
unrelated warnings remain; no full suite, live integration or benchmark ran.

## Attached ExtProc summary rendering audit

Traced terminal summary claim, immutable scalar capture, owned formatting and
first-match PluginLog insertion. Recorded a typed-snapshot/canonical-formatter
alternative in log-05, preserving claim even without a destination and bounded
log behavior. The existing lock-shortening change remains intact. Short inline
rendering may avoid temporary storage, while larger entries can spill; no
unconditional allocation or throughput improvement is claimed.

Independent plan review approved two documents. Fresh baseline passed 143
ExtProc and ten terminal-logging tests (153 total), formatting check, workspace
check/Clippy and agent-doc validation. Existing unrelated warnings remain; no
full suite, live integration or benchmark ran.

## Reuse bounded access-log extension values on override

Borrowed LogExt overrides now reuse the existing String only when the selected
UTF-8 prefix fits its capacity. A larger prefix receives an exact new allocation,
so geometric growth cannot exceed the established retained-capacity bound.
Owned inputs retain their move/oversized-capacity-release path. Key order, field
count gates, source resolution, sensitive flags and serialization are unchanged.
The shared prefix helper keeps one UTF-8 truncation algorithm.

Independent plan review approved three files. The baseline passed 19 LogExt
tests. The new full-map sequence compared all fields, bounded UTF-8 prefixes and
both sensitive-flag transitions before failing on the original implementation's
buffer replacement. It covers empty values and long/short/long transitions;
another test retains owned overwrite transfer and oversized-capacity release.
Reused short values retain the earlier bounded capacity, at most 256 bytes per
field; this removes fitting-override allocations without claiming every case
uses less retained memory or copies fewer bytes.

Final verification passed 21 LogExt and ten terminal-logging tests (31 total).
After simplifying a test iterator in response to Clippy, the 21 affected tests,
formatting and workspace check/Clippy passed again. Seven repository guards
passed; only existing unrelated warnings remain. No full suite, live integration
or throughput benchmark ran.

## Borrow typed outbound SAN mismatch reasons

The canonical outbound SAN detector now returns Cow text: typed matches borrow
from the retained error, while the Display fallback retains owned storage. The
full typed-first traversal, one depth budget and io-inner replacement semantics
remain unchanged. OIDC's classification-only consumer avoids the reason clone;
LDAP converts to owned text at its existing stored-error boundary. This affects
backend-independent outbound clients, not the unsupported Gateway rustls backend.
No diagnostic text, metric ownership or sensitive-data policy changed.

Independent plan review approved four files. The 11 verifier baseline tests
passed. The added direct/rustls/io-wrapper matrix compared reason bytes before
failing on the original implementation's one allocation; final recovery uses
zero allocations and the original buffer. The fallback is explicitly checked
as owned. Final verification passed 12 verifier, 28 LDAP and eight OIDC dependency
error tests (48 total). An initial OIDC error-module filter selected zero tests;
the actual dependency_error_tests selection was then run and passed.
Formatting, workspace check/Clippy and seven guards passed with existing
unrelated warnings. No full suite, live handshake or throughput benchmark ran.

## Backend SAN projection audit

Traced the prepared BoringSSL hook and backend-independent outbound verifier
through their SAN projections into the canonical matcher. Matching itself is
already allocation-free; full display vectors and the native path's OtherName
pass remain possible savings. Recorded tls-05 for the extraction boundary,
including display-prefix semantics, policy-major reason selection, parser/panic
failure ownership and preservation of full client certificate projections.
No runtime change was selected.

Independent plan review approved two documents. Fresh baseline passed two
prepared-backend, ten backend-SAN, 18 certificate-extractor and 12 outbound
verifier tests (42 total), formatting check, workspace check/Clippy and agent-doc
validation. Existing unrelated warnings remain; no full suite, live TLS
integration or benchmark ran.

## Disabled connection-establishment log projection

The successful-connect callback now checks the canonical logger's existing
sender count before constructing the gRPC-Web/WebSocket connection-established
entry. A zero-sender logger already silently discarded the serialized string;
this avoids the unused projection and JSON allocation. SAN validation,
connection timing and LB commit still run before this condition. Nonempty
unhealthy loggers retain dispatch and dropped-counter behavior. No other
logging callback or logger ownership contract changed.

Independent plan review passed. Fresh baseline and final verification each
passed 63 access-log tests and seven LB-attempt tests (70 total). Formatting,
workspace check/Clippy and seven guards passed with existing unrelated
warnings. No full suite, live connection test or throughput benchmark ran.

## Disabled terminal access-log projection

The terminal logging callback now skips entry/extension projection, metadata
construction and JSON serialization only when the configured logger has zero
senders and the integration-test store does not request the entry. All existing
Wasm, cache, transcoder, AI, LB, metrics, response-terminal, ExtProc and WAF
finalizers still execute before the gate. Mirror cleanup executes on both paths,
and the active-context release guard remains alive until callback exit.
Nonempty unhealthy loggers retain dispatch/drop accounting. The test-store path
retains its exact header match and existing serialized output/clone behavior.

Independent plan review passed. A subprocess-isolated, in-memory Session test
passed on the original implementation and after the change across 24 combinations
of test mode, zero/healthy/unhealthy senders and absent/exact/wrong/case-varied
store headers. It verifies LB balance, ExtProc terminalization, response closure,
mirror cleanup and test-store/sender JSON equality. Final focused verification
passed 11 logging, 63 access-log, seven LB-attempt and seven cache-lifecycle tests
(88 total). Formatting, workspace check/Clippy and seven guards passed
with existing unrelated warnings. No full suite, live network or benchmark ran.

## Borrowed TCP log projection

TcpLogEntry now borrows both peer address strings and the optional upstream
address from TcpContext, and retains TcpStatus's static label directly. Its sole
production caller keeps the context alive; synchronous JSON serialization still
produces the owned String consumed by the asynchronous logger. Masking, field
names, omissions, timestamp/duration sampling, fallback and sender behavior are
unchanged. Updated both developer-guide snippets and corrected the owning review
page's stale claim that the entry itself crossed an asynchronous channel.

Independent plan review approved the six-file scope. The new JSON-equivalence
fixture passed its content comparison on the original implementation before the
zero-allocation assertion failed with three allocations for an absent upstream.
The final test covers absent/present upstream addresses and pointer identity;
projection now allocates zero times. JSON/redaction may still allocate. Fresh
verification passed five TCP-log, 27 TCP-route and eight logger tests (40 total),
formatting, workspace check/Clippy and seven guards. Existing unrelated
warnings remain. No full suite, live connection or throughput benchmark ran.

## Disabled TLS listener log projection

Both TLS termination and passthrough disconnect paths now consult the listener
logger's existing sender count before building its JSON Value tree and output
String. The captured logging-enable gate, end-time assignment and awaited global
TLS log remain ahead of this condition. Nonempty unhealthy listener loggers
retain dispatch and drop accounting; active listener output and masking are
unchanged. The separate tls-01 direct-serialization decision remains deferred.

Independent plan review passed. Fresh baseline passed 85 TLS-route tests; final
verification passed those 85 tests plus eight logger tests (93 total), including
existing isolated Off/Replace/Wrap masking checks. Formatting, workspace
check/Clippy and six specialized guards passed with existing unrelated warnings.
No full suite, live TLS connection or allocation/throughput benchmark ran.

## Redirect listener Location ownership transfer

The dedicated HTTP-to-HTTPS redirect callback now moves its completed Location
String into Pingora's header insertion instead of passing a borrowed String.
Pinned http 1.5 validates both forms identically, but the owned conversion feeds
Bytes directly and avoids copying the payload. The local String had no later
consumer. Query/percent encoding, raw authority and IPv6 handling, port choice,
header order and the separate query-free log URL remain unchanged.

Bytes may allocate shared metadata and retain spare String capacity; this is
not a zero-allocation header or throughput claim. Independent plan review passed.
Fresh baseline and final verification each passed 13 redirect tests, including
in-memory wire and connection-reuse cases. Formatting, workspace check/Clippy
and six specialized guards passed with existing unrelated warnings. No full
suite, external-network integration or allocation/throughput benchmark ran.

## Disabled redirect and mirror log rendering

The redirect logging callback now returns before timestamp/error projection and
JSON serialization when its logger has zero senders. The mirror emitter checks
the same condition after resolving the existing optional logger and before JSON
serialization. Its caller still records mirror metrics and releases admission
permits after the helper returns; request execution and entry construction are
unchanged. Nonempty unhealthy loggers still dispatch and count loss. Active
log formatting, redaction and serialization failure handling remain unchanged.

Independent plan review passed. Fresh verification passed 13 redirect, 27 mirror
and eight logger tests (48 total), formatting, workspace check/Clippy and six
specialized guards, with existing unrelated warnings. No new full suite, live
network integration or allocation/throughput benchmark ran.

## Request-pinned RemoteIp header ownership

Removed the duplicated HeaderName from EdgionHttpContext and its new_ctx clone.
Forwarding, credential collision checks and response-cache key framing now borrow
that name from the same immutable configuration Arc already pinned by the request.
The upstream HeaderMap still receives its required owned key. Publication,
validation, fallback, cache-key bytes and header precedence remain unchanged.
The lifecycle documentation now describes this ownership accurately.

Independent plan review passed, including the narrow lifecycle-document update.
Fresh final verification passed 109 targeted tests: new_ctx (3), upstream request
filter (15), AI provider (4), response cache (40), credential injector (7), and
Gateway config (40). The new generation test exercises real upstream output and
case-insensitive credential-header checks after update and deletion. Formatting,
workspace check/Clippy and seven guards passed with existing unrelated warnings.
This removes duplicate storage and cloning; no unconditional heap-allocation,
context-size or throughput claim is made. No full suite or live network test ran.


## Service candidate runtime eligibility lookup audit

Traced upstream Service selection through compiled RR/ConsistentHash and P2C
predicates into runtime health/outlier reads. Endpoint identities are already
prepared, but candidate predicates still query the captured endpoint map; degrade
counting and selection can repeat that lookup. A prepared binding requires a
separate lifetime/publication design and measured value, so backend-02 records it
for user decision. Existing selected-slot lookup remains a separate candidate.

No runtime code changed. Membership activation creates fresh states; policy-only
snapshots can reuse the map; old states freeze on retirement. Do not cache health
booleans or remove accepted cooldown/recovery synchronization. Fresh baseline
verification passed 97 selector, compiled-selector, coordinator and outlier tests.
No benchmark establishes a benefit for the proposed representation.

## Borrow the captured retry budget for synchronous gates

retry_budget_for now returns a reference to the request-captured budget Arc
instead of cloning it. The attempt denominator and three ordinary retry gates
consume it synchronously, with no later use across context mutation. Service/AI
policy selection, pinned absence, epoch sampling, accounting and denial responses
remain unchanged. Only the test retaining its first budget across a mutable
attempt transition explicitly clones an owner; generation identity assertions
remain intact. The removed work is an Arc increment/decrement per successful
lookup, not a heap-allocation or measured throughput claim.

Independent plan review passed. Fresh verification passed upstream-peer (104),
connect-failure (9), proxy-error (11), upstream-response (11), and retry-budget
(28) test selections; these filters overlap. Formatting, workspace check/Clippy
and seven guards passed with existing unrelated warnings. No full test suite or
live network benchmark ran.

## Move final AI usage owner into the response wrapper

AI attempt preparation now moves its local usage Arc into AiAttemptResponseProcessor
at the final use instead of cloning it and dropping the local owner on return.
The earlier request-context and concrete-processor owners are unchanged, as are
EOS, quota, unary admission and commit sequencing. No callback or await occurs
between this ownership transfer and returning the prepared attempt. The benefit
is one fewer Arc increment/decrement, not a heap or throughput claim.

Independent plan review passed. Baseline dispatch verification passed 35 tests;
fresh final verification passed 112 tests: 35 dispatch, 25 attempt-runtime and
52 usage-accounting tests. Formatting, workspace check/Clippy and seven guards
passed with existing unrelated warnings. No full suite or live provider/Redis
integration test ran.


## Non-SSE usage character-count fast-path audit

Followed the AI response processor into the incremental non-SSE UTF-8 counter.
An all-ASCII/no-pending fast path avoids per-byte state transitions, but an
exploratory extracted-counter probe showed workload-dependent timing: ASCII
improved while end-position multibyte cases took longer. Fixed execution order,
synthetic chunks and excluded parser work limit that evidence. The proposal and
required split-state/callback benchmarks are deferred in usage-01, with no
runtime or test changes. The separate SSE and native Responses policies remain
unchanged. Fresh baseline checks passed 52 usage and 13 finalization tests,
formatting, workspace check/Clippy and document consistency; existing warnings
are unrelated. Baseline tests do not prove candidate equivalence.

## Transfer the finalized AI attempt ledger into its summary

Terminal AI accounting now takes the completed attempt Vec into GenAiSummary
instead of deep-cloning its entries. Runtime finalization still consumes the
active attempt and drains pending settlements first. The transfer and summary
publication contain no await; the existing summary sentinel prevents a repeated
transfer. Subsequent metrics and quota settlement use independent cumulative
fields, which remain intact, including unknown/overflow and truncation state.
The terminal callback does not resume attempt dispatch afterward.

The retained Vec remains bounded by the existing eight-entry ledger policy;
its capacity already stayed alive in the runtime until context destruction.
The transfer removes the additional Vec/entry-string copies without adding a
new owner or claiming measured throughput. The expanded retry-metrics test
checks exact entries, nonempty buffer identity, empty source ledger, preserved
charges/totals and repeated-finalization summary/metric stability.

Independent plan review passed. Baseline finalizer tests passed (13); final
verification passed 84 tests: 13 finalizer, 25 attempt-runtime, 35 dispatch and
11 terminal-logging tests. Formatting, workspace check/Clippy and seven guards
passed with existing unrelated warnings. No full suite or live Redis/provider
integration ran.

## Borrow fixed Stream and TLS fail-closed reasons

The always-deny Stream and TLS route gates now store the canonical
Cow<'static, str> reason instead of String. Fixed invalid/unsupported reasons
remain borrowed when returned by each callback, removing their per-denial
String allocation and copy. Formatted policy reasons remain owned and cloned.
Exact messages, policy precedence, unsupported-before-disable handling, cache
invalidation and publication ordering are unchanged. Downstream log ownership
may still require a copy; this is not an end-to-end zero-allocation claim.

Tests assert borrowed fixed reasons, owned policy reasons, both unsupported
source gates, direct unsupported entries and TLS invalid-configuration denial.
Independent plan review passed. Baseline runtime tests passed (15 Stream and
7 TLS); fresh final verification passed 48 tests: 15 Stream runtime, 8 TLS
runtime and 25 HTTP/Stream plugin-generation tests. Formatting, workspace
check/Clippy and seven guards passed with existing unrelated warnings. No full
suite or live integration ran.

## Borrow connection-owned SNI across TLS routing and setup

Terminated TLS no longer clones the context SNI into a second connection-local
String; passthrough moves the parser result into that context instead of cloning
it. Route matching, Stage-2 plugin dispatch and the shared PP2 helper borrow the
same context owner. Their redundant SNI parameters are removed. Entry-point
missing-SNI rejection remains in place, and no later connection phase mutates
SNI. No take/restore, cancellation cleanup, new normalization or cache is needed.
The existing PP2 builder owns its authority/header bytes before socket writes;
plugin borrowing ends before mutable connection-log updates. ClientHello
read/replay bounds, TLS identity isolation and log ownership remain unchanged.
This removes one redundant SNI allocation/copy per connection reaching the
handler in either mode, without a throughput claim.

Independent plan review passed. Baseline TLS verification passed 85 tests;
fresh verification passed the same 85 TLS tests plus 8 TLS runtime tests. The
existing identity spy now checks context SNI pointer identity and retention in
both modes; PP2 exact-byte, skip and failure tests retain their assertions with
context-owned input. Formatting, workspace check/Clippy and seven guards passed
with existing unrelated warnings. No full suite or live integration ran.

## Borrow the PP2 authority payload until header encoding

The PP2 builder now stores TLV values as Cow byte slices. Authority borrows
its caller's hostname instead of allocating an intermediate byte Vec; generic
add_tlv still takes ownership of its supplied Vec without copying. The sole
production caller builds the final independently owned header before socket
writes, so this removes one intermediate nonempty-authority allocation/copy.
The TLV collection and final output allocations remain. Address-family mapping,
TLV order, empty-authority encoding, existing length casts and write/flush
failure accounting are unchanged. No new size policy or throughput claim is
introduced. This supersedes the prior checkpoint's temporary owned-authority
buffer description; its final-header ownership boundary still holds.

Independent plan review passed. Baseline encoder verification passed 7 tests;
fresh verification passed 8 encoder and 85 TLS tests. Tests check nonstatic
borrow identity, transferred generic TLV ownership, repeated-build equality,
output survival after hostname/builder destruction and empty authority bytes.
Formatting, workspace check/Clippy and seven guards passed with existing
unrelated warnings. No full suite or live integration ran.


## Shared relay idle-reset audit

Followed TCP/TLS relay progress into the shared idle timer. Each successful
read or advancing write samples time and resets Sleep; both directions can
produce two resets in a sweep. Deferred sweep-local coalescing in relay-01:
retaining the last progress timestamp still needs expiry/wakeup review and
workload evidence, while sampling once per sweep changes the deadline policy.
The locked Tokio 1.53.1 traditional timer already has an atomic extension fast
path, so repeated resets are not necessarily repeated wheel registration.
Runtime and tests remain unchanged. Fresh baseline verification passed 21 relay
tests, formatting, workspace check/Clippy and document consistency with existing
unrelated warnings. No candidate benchmark or live integration ran.

## Borrow the selected GCIR ACL after one profile lookup

GlobalConnectionIpRestriction's Selector resolver now returns the borrowed
compiled ACL instead of returning its name after contains_key validation.
A valid Selector override therefore performs one profile-map lookup instead
of contains_key followed by get. Base fallback stays lazy; missing, disabled,
null, unknown-profile and wrong-type Selector branches retain their diagnostics.
Missing compiled profiles still fail closed. Selection and membership continue
to use one captured ConfigData generation, with no new cache, Arc clone or
matcher projection. This is a lookup reduction, not an allocation or throughput
claim, and is separate from the existing precomputed reference-key mechanism.

Independent plan review passed. Baseline GCIR verification passed 16 tests;
fresh verification passed 42 tests: 16 GCIR, 4 shared ACL and 22 ConfigData.
The existing successful-switch test also checks selected ACL pointer identity.
Formatting, workspace check/Clippy and seven guards passed with existing
unrelated warnings. No full suite or live integration ran.

## Prepare GeoIP reference-presence demand once

CompiledGeoIpRefs now records whether its immutable rule grid contains any
bound CIDR predicate after successful HTTP or Stream construction. has_refs
reads that boolean instead of traversing the same fixed grid on every request
or connection. Both constructors compute the exact prior predicate-presence
expression after binding finishes. The grid has no postconstruction mutator.
This adds a per-instance scalar; it does not cache live reference availability.
Required-reference preflight, permission gates, single-generation capture and
membership retain their existing traversal and failure behavior.

Independent plan review passed. The actual-owner constructor matrix covers
geographic-only, late deny CIDR, allow CIDR and HTTP parse-only configurations.
Baseline shared-reference tests passed (5); final verification passed 35 tests:
6 shared GeoIP references, 22 HTTP/Stream GeoIP and 7 IP-reference tests.
A temporary iterator lifetime compile error and a missing required failOpen
field in the new fixture were corrected before these final results. Formatting,
workspace check/Clippy and seven guards passed with existing unrelated warnings.
No full suite, live integration or throughput measurement ran.

## Prepare HTTP GeoIP CIDR address demand

HTTP GeoIpLocation now computes direct/remote parsed-address demand from CIDR
rules in both deny and allow groups during construction. Geographic dimensions
consume GeoDimFacts only, so rule-context construction skips IP parsing for
sources without CIDR rules. The immutable key/source scan happens after canonical
validation; sanitization does not change either field. Fact-demand compilation,
parse-only log filling, required-reference preflight, snapshot capture and
lookup-failure precedence remain unchanged. There is no request-address cache.
GeoIP fact resolution may still parse addresses independently; this removes only
unused rule-context parsing and makes no throughput claim.

Independent plan review passed. Baseline HTTP GeoIP verification passed 11 tests;
fresh final verification passed 43 tests: 23 HTTP/Stream GeoIP, 6 shared GeoIP
references and 14 resource GeoRule tests. The new matrix uses existing request
fact slots and asserts IP-getter counts for dimension-only, direct/remote/both
CIDR demand, mixed rules, both policy sides and empty/malformed CIDR addresses,
alongside exact result/log assertions. A fixture-only unsupported Default use
was corrected before final verification. Formatting, workspace check/Clippy and
seven guards passed with existing unrelated warnings. No full suite or live
integration ran.

## Reuse equal-address GeoIP slots without a second parse

After parsing and normalizing the current address successfully, resolve_geo
now recognizes identical raw address strings before parsing the other slot's
address. This skips the second SocketAddr/IpAddr parse only when a cached other
slot exists and the valid strings match. Different textual representations
retain the existing normalized comparison, including mapped IPv4 and ports.
Invalid current input still returns NotFound before consulting the cache.
Lazy ownership, Found Arc sharing, RealIp invalidation and logging are unchanged.
Unequal strings incur an additional comparison; no overall throughput claim is
made and no parsed-address cache is introduced.

Independent plan review passed. The former equal-result test now seeds a Found
value and checks Arc identity through both accessor orders for identical and
equivalent address spellings. Negative cases cover identical invalid input,
invalid other input and different hosts. Baseline context tests passed (3);
fresh verification passed 59 tests: 4 GeoIP context, 23 HTTP/Stream GeoIP and
32 RealIp-filtered tests. Formatting, workspace check/Clippy and seven guards
passed with existing unrelated warnings. No full suite or live integration ran.

## Empty gzip output buffer retention audit

Traced the downstream compression module into the pinned fork's gzip encoder.
Its unconditional output-Vec transfer resets capacity even on empty output;
bytes 1.12.1 retains spare capacity in the returned Bytes instead of dropping it
as the fork comment suggests. Compression-02 defers retaining empty buffers:
potential allocation savings need comparison with longer capacity residency,
actual empty-output frequency and caller lifetimes. No runtime or dependency
changes were made, and this does not close the remaining compression audit.

Independent plan review passed. Existing response-filter verification passed
26 tests; formatting, workspace check/Clippy and agent-doc consistency passed
with existing unrelated warnings. No allocator benchmark, external fork suite,
full workspace test suite or live integration ran.

## Borrow fixed discovered-protocol labels

RequestInfo.discover_protocol now stores an optional static string reference.
The only production writers assign the fixed grpc, grpc-web and websocket labels,
so protocol detection and backend WebSocket supplementation no longer allocate
an owned String for this field. Consumers read the same strings directly;
RequestInfo serialization still emits a string or omits the absent field.
Existing classification short circuits, WebSocket precedence, gRPC flags,
backend/transcoder guards, body admission, cache eligibility and connection-log
timing remain unchanged. This is an internal Rust field-type change, with no
new dynamic-label compatibility layer or protocol policy.

Independent plan review passed. Baseline detector tests passed (3); final
verification passed 253 tests across detector, early request, session adapter,
peer builder, request entry, gRPC integration, GraphQL and DSL metadata filters.
New tests measure zero allocations for classification and backend label filling,
check existing-label preservation and verify serialized labels/None omission.
Formatting, workspace check/Clippy and seven guards passed with existing unrelated
warnings. No full suite, live integration or end-to-end throughput measurement ran.

## Project scoped HTTP runtimes without a temporary resource owner

PluginScope now exposes a direct HTTP runtime projection. Global/Gateway and
route ExtensionRef resolution, plus the pinned backend-reference resolver, clone
only the selected runtime Arc instead of first cloning and discarding its outer
CompiledPlugins Arc. The immutable scope still owns the compiled resource during
the lookup. Complete-resource access remains available for access-log declarations.
Reference ordering, duplicate/missing slots, scope capture, denial runtimes and
backend hit/miss caching remain unchanged. No prepared-list cache was introduced;
early-01 remains deferred.

Independent plan review passed after correcting the initial accessor inventory:
access-log declarations still require the complete resource, and backend reference
projection was added to the explicit scope. Baseline resolver tests passed (11).
Final targeted verification passed 90 tests across resolver, publication, early
request, plugin request state and backend-reference pinning, explicitly excluding
one layout assertion. A new Weak-owner test verifies runtime pointer identity and
survival without retaining the compiled wrapper. The layout assertion failed with
184 versus 176 bytes both on the implementation and exact unmodified 25e0fefda;
verification-04 tracks it without changing the assertion. Later assertions in
that test were not reached. Formatting, workspace check/Clippy and seven guards
passed with existing unrelated warnings. No full suite, live integration or
throughput measurement ran.

## Borrow captured access-log declarations

Terminal access-log extension collection now reads the canonical declaration
slice directly from its captured PluginScope. It no longer clones a complete
CompiledPlugins Arc for each referenced declaration source. The immutable scope
owns the borrowed slice throughout synchronous field resolution. Missing and
empty declarations remain empty; source resolution, tier order, overwrite rules,
field/value bounds, masking and serde escaping are unchanged. Complete-resource
lookup is now only a unit-test helper in the existing publication test module.

Independent plan review passed, including the helper relocation after the layout
guard rejected a test-only method in production source. Baseline log-extension
verification passed 25 tests; final verification passed 105 tests across log
extensions, publication, early request and runtime resolution. The extended
fixture checks nonempty slice identity, missing references, new-snapshot deletion
and old-snapshot collection, retaining valid/invalid route-reference assertions.
Final formatting, workspace check/Clippy and all seven guards passed with existing
unrelated warnings. No full suite, live integration or throughput measurement ran;
the unrelated layout failure tracked in verification-04 was not rerun here.

## Delay connection-rate Redis dimension keys until client availability

ConnectionRateLimit passes a statically dispatched key constructor into its
private Redis dimension helper. Missing references, absent published clients and
the existing unhealthy-client fast exit now return before formatting listener
or source-IP keys. The helper builds the same owned key once after those checks,
then uses the unchanged canonical Redis core and timeout. Local admission still
precedes Redis, Redis denial never refunds a token, and source dimensions keep
their existing failure policy. No cache, clock, registry or metric was added.

The adjacent Stream attachment audit retained selected CompiledStreamPlugins
ownership across awaits: callers deliberately release the larger scope and
publication first, and TLS also needs declared-stage metadata. Local gate Arc
ownership likewise releases the port-map lock before bucket locking; this is
not a mechanical clone-to-borrow candidate.

Independent plan review passed. Baseline plugin tests passed (9); final connection
rate-limit verification passed 32 with one existing measurement case ignored,
and shared Redis-core verification passed 29. The new panic-factory matrix proves
missing/absent clients do not build keys and preserves both failure results.
Its missing-reference helper case does not bypass production invalid-config
rejection, which remains covered separately. Formatting, workspace check/Clippy
and seven guards passed with existing unrelated warnings. No live Redis exchange,
full suite, allocation benchmark or throughput measurement ran.

## Stream and TLSRoute dispatch representation

The connection and TLSRoute runtime audit traced ordered dynamic plugin calls
through async-trait 0.1.92. Several callbacks execute without await but still use
its boxed-future representation. This establishes a profiling candidate, not a
measured allocation or throughput gain. Interface complexity, evaluation timing
and cancellation make an automatic rewrite inappropriate. The decision and
required evidence are recorded in
[stream-01-synchronous-plugin-dispatch.md](stream-01-synchronous-plugin-dispatch.md).
No runtime or test code changed in this documentation batch.

## Share immutable named load-balancing hash keys

The captured per-attempt LB policy and live policy getter clone ParsedLBPolicy.
Header, cookie and query names previously deep-copied a String on each clone.
Their runtime-only payloads now use Arc<str>, constructed from the same original
bytes by LoadBalancerConfig::to_parsed. Selector lookup remains borrowed string
lookup with identical case, decoding and missing-key behavior. SourceIp and
non-hash policies remain scalar. Resources explicitly enables serde rc support;
the enum's serialized shape stays unchanged and no resource schema field changed.

Cold Arc storage and atomic clone/drop replace repeated nonempty String allocation
and copying. Empty keys can now cost cold Arc storage where an empty String did
not; no measured throughput improvement is claimed. Sharing retains only the key,
not a policy generation or configuration owner. Request policy capture, retries,
live getters and selected-backend ownership retain their existing boundaries.
Independent plan review approved this scope. The added resource test checks all
three named modes, missing/empty/mixed-case/multibyte keys, shared clone identity,
owner release and exact serialized values. Existing gateway policy-transfer and
captured-input tests retain their original lifetime and ownership assertions.

Baseline resource policy tests passed 18; final verification passed 19 resource
policy tests, 12 Gateway selector tests, five captured-input tests and one policy
transfer test (37 total). An initial new-test initializer omitted degrade_threshold;
adding None corrected the compile error before these successful results. Formatting,
workspace check/Clippy and seven guards passed with existing unrelated warnings.
No full suite, live traffic or throughput benchmark ran. The known unrelated
native TLS and context-layout baseline failures were not included in this scope.

## Borrow fixed upstream attempt error reasons

UpstreamInfo.err now stores Cow<'static, str>. Fixed connection error types,
ordinary/AI retry markers, body-size diagnostics and unsafe-address reasons
borrow their existing static strings; the formatted DNS failure still moves its
owned String into the attempt. The locked Pingora ErrorType::as_str contract
returns a static string, including custom error kinds. No new diagnostic, log,
metric or deduplication policy was introduced. Append order, exclusions and the
terminal upstream-timeout-only exact deduplication rule remain unchanged.

Independent plan review passed. Added tests prove mixed borrowed/owned serde
values, empty err omission, ownership transfer without a dynamic-string copy,
borrowed body-size diagnostics, exact deduplication against an existing owned
timeout and continued duplicate ordinary errors. With the Error objects and
Vec capacity prepared outside the measured closure, fixed-error append performs
zero allocations. This does not claim an allocation-free error callback: vector
growth, dynamic messages and Pingora error context retain their existing costs.

Baseline fail-to-connect/proxy tests passed 25. Final targeted verification passed
73 tests across failure callbacks, upstream response handling, AI attempts,
UpstreamInfo and unsafe-address scenarios. Formatting, workspace check/Clippy
and all seven guards passed with existing unrelated warnings. No full suite,
live traffic or throughput benchmark ran; the known unrelated context-layout
baseline test was outside this targeted scope.

## Format CSRF signing timestamps in bounded stack storage

CsrfToken::signature_bytes now formats the existing signed i64 timestamp with
standard Display into a 20-byte stack buffer through std::io::Write. The longest
representation is i64::MIN, including its sign; only the written prefix enters
HMAC. This removes expires.to_string without a new formatter, dependency or
cached cryptographic state. Delimiters, nonce bytes, key handling, clock/lifetime
checks, token JSON/Base64 and exact signature comparison remain unchanged.

Independent plan review passed. Existing signed-boundary/nonce/key parity and
wire-shape fixtures are retained. The added digest-only allocation assertion
constructs its fixture and expected signature outside the measured closure and
checks boundary/current timestamps with a long nonce. The owning signature-input
knowledge now distinguishes stack timestamp formatting from still-owned issuance
and encoding output. No throughput or cryptographic timing claim is made.

Baseline CSRF verification passed 19 tests; final verification passed all 20,
including zero digest allocations at every tested timestamp boundary. Formatting,
workspace check/Clippy and seven guards passed with existing unrelated warnings.
No full suite, live integration or throughput benchmark ran.

## Defer CSRF response cookie storage transfer

[csrf-01-cookie-header-ownership.md](csrf-01-cookie-header-ownership.md) records
the temporary cookie String copied through the session string appender. Owned
transfer, retained capacity and exact formatting remain a user decision, separate
from pending-operation replay ownership. The task preserves append/order,
validation, direct-versus-queued invalid-value handling and token redaction.
No runtime code changed. Formatting, workspace check/Clippy and documentation
validation passed; existing unrelated Clippy warnings remain. No new runtime
test or throughput benchmark ran for this documentation-only decision record.

## Reuse canonical transport identity without global RealIp policy

Request metadata now copies the transport extractor's already canonical address
straight into remote_ip when no global RealIp policy is configured. Previously
that branch cloned the same address and parsed/formatted it again. The extractor
uses the canonical IP helper for Inet peers and the current caller supplies an
empty missing-address fallback; no intervening code mutates downstream_peer_ip.
The policy-enabled branch retains its snapshot, header selection, resolution and
canonicalization. Direct peer, effective PROXY peer, XFF and later plugin identity
changes keep their existing ownership and trust boundaries.

Independent plan review approved the data-flow proof and existing test coverage.
This removes one redundant parse/format and temporary String allocation on the
valid Inet/no-policy path; it does not remove the independently owned identity
fields or claim an allocation-free metadata callback or measured throughput.

Before and after the change, all 45 other request-filter tests passed and the
same response-cache reload test failed with backend policy ancestor limit, as
already tracked in verification-01-cache-policy-fixture.md. Final transport
identity tests passed four and RealIp tests passed 32. Formatting, workspace
check/Clippy and seven guards passed with existing unrelated warnings. No full
suite or live traffic test ran; the pre-existing cache fixture remains deferred.

## Keep RealIp extraction typed until its consumer boundary

RealIpResolution now retains the selected IpAddr instead of allocating its
Display text. Header traversal returns the already parsed address, preserving
membership-call order, recursive/nonrecursive selection, hop caps and fallback.
Global request metadata passes that address directly to canonicalize_ip, removing
the former Display-to-String, parse and second-format sequence. The RealIp plugin
still renders ordinary Display text before its existing comparison, setter and
sensitive log; mapped IPv6 is deliberately not collapsed before that boundary.
No session API, trust snapshot, schema, log field or GeoIP invalidation changed.

Independent plan review covered all consumers, including configuration-publication
fixtures. Existing textual expectations remain unchanged. Added tests prove zero
extractor allocation with an allocation-free membership callback across IPv4,
IPv6, mapped IPv6, invalid input and cap/fallback cases; final canonical projection
matches the old text round trip. A plugin regression preserves setter invocation
when canonical IPv4 differs textually from mapped IPv6, and the unchanged branch
when mapped text already matches. This does not claim zero allocations in policy
lookup, plugins, final address formatting or the complete request callback.

Baseline RealIp tests passed 32; final RealIp tests passed 34, including global
publication and visibility recovery. The request-filter group passed 45 and
retained the same known response-cache ancestor-policy fixture failure recorded
in verification-01-cache-policy-fixture.md. Formatting, workspace check/Clippy
and seven guards passed with existing unrelated warnings. No full suite, live
traffic or throughput benchmark ran.

## Defer shared HTTP session address projection

[ip-01-session-address-projection.md](ip-01-session-address-projection.md) records
repeated string-to-IP parsing across ACL, IP conditions and RequestAccessPolicy.
A shared typed or lazy projection requires an enforceable mutation boundary and
request-memory measurements, including requests with no IP consumers. Bare-IP
ACL grammar must remain distinct from GeoIP's socket-address acceptance, and
RemoteIp updates must preserve trust snapshots and remote GeoIP invalidation.
This is separate from the completed local typed RealIp result optimization.
No runtime code changed. Formatting, workspace check/Clippy and documentation
validation passed with existing unrelated warnings. No runtime tests or benchmark
were added or run for this documentation-only decision record.

## Prepare RequestAccessPolicy primary IP demand

RuntimeProfile records whether its compiled primary rules use DirectPeerIp or
RemoteIp. Profile evaluation parses only needed sources, retaining their existing
entry-time snapshot before URL lookup and asynchronous supplemental conditions.
URL-only profiles no longer read or parse either address merely to pass unused
arguments. Supplemental conditions retain their own session reads and short
circuiting; they do not cause an unused primary snapshot. Flags are derived from
all compiled groups once, with no request-path rule scan or shared IP cache.

Independent plan review passed. Two cold profile booleans (and possible padding)
replace repeated unused getter/parse work; request context layout is unchanged.
The new matrix covers neither, either and both source demands, repeated rules
in a later group, distinct IPv4/IPv6 inputs and empty/invalid inputs reaching URL
fallback. The imported-URL supplemental-condition regression now requires exactly
one direct read and either one remote read or zero on direct-condition failure.
Existing group ordering, required-reference gates and profile selection remain.

Baseline RAP tests passed 18. Final verification passed 19 RAP tests, 68 shared
condition tests and 34 RealIp tests (121 total). Formatting, workspace check/Clippy
and seven guards passed with existing unrelated warnings. No full suite, live
traffic or throughput benchmark ran. The deferred shared session address
projection remains separate; this change adds no cached request identity.

## Prepare RequestAccessPolicy URL host demand

RuntimeUrlIndex records whether any final compiled candidate has a nonempty
hosts set. Pure path profiles now skip the unused Host read and normalization;
demanding profiles keep the same pre-condition snapshot and NormalizedHostname
rules. Imported candidate sets remain authoritative over authored fallback
fields. A nonempty set containing the normalized empty string still requires a
Host read. Supplemental conditions retain their independent header reads, and
path matching is unchanged even when no candidate restricts hosts.

Independent plan review passed. Added cases cover hostless hit/miss, a hostless
candidate before a restricted candidate, imported-candidate precedence, mixed
case/port/trailing-dot normalization, mismatch/missing Host and the empty-string
set boundary. A separate condition test requires exactly its own Host-map read
without a redundant outer read. One cold index boolean and possible padding
replace unused per-request normalization; no shared cache or context field is
introduced and no throughput improvement is claimed.

Baseline RAP tests passed 19. Final verification passed 21 RAP tests and 68
shared condition tests (89 total). Formatting, workspace check/Clippy and seven
guards passed with existing unrelated warnings. No full suite or live traffic
benchmark ran.

## Retain RequestAccessPolicy candidate segments as ranges

Each RuntimeRule::Url now stores its contiguous candidate Range instead of
collecting that range into Vec<usize>. Compilation appends exactly one inline
candidate or the resolved imported candidates, so the stored start/end preserves
the existing sequence, including empty imports. Request evaluation clones the
range cursor and visits the same candidate indices in order. Short-circuiting,
conditions, terminal outcomes and actual candidate ownership remain unchanged.

Independent plan review verified the single append-only constructor and all
consumers. A new behavioral regression combines an empty import, a multi-candidate
import and consecutive inline rules across two required groups: the common path
passes while paths satisfying only one group fail. Existing imported ownership,
condition ordering and profile selection tests remain intact.

This removes nonempty per-rule cold index-vector allocation/storage and request
index-array reads. It does not remove the request-local matched_urls vector,
claim a smaller overall enum layout or establish throughput improvement.
Baseline RAP tests passed 21; final verification passed 22 RAP and 68 shared
condition tests (90 total). Formatting, workspace check/Clippy and seven guards
passed with existing unrelated warnings. No full suite or live benchmark ran.

## Defer RAP URL match-result storage

[rap-01-url-match-storage.md](rap-01-url-match-storage.md) records the per-evaluation
dense candidate result vector and its lifetime across supplemental-condition
awaits. Inline, packed and sparse alternatives need measurement of CPU work,
retained storage and concurrent future sizes. The task preserves policy ordering
and the existing path snapshot, and distinguishes per-reference limits from
aggregate profile size. Radix scratch and completed cold candidate ranges are
separate costs. No runtime code changed. Formatting, workspace check/Clippy and
documentation validation passed with existing unrelated warnings; no runtime
tests or benchmarks ran for this documentation-only decision record.

## Defer Guardrail held replacement-header representation

[guardrail-01-held-replacement-header-storage.md](guardrail-01-held-replacement-header-storage.md)
records the ready 403 template prepared at the final post-onion Hold boundary and
cloned on policy rejection. A smaller frozen projection might defer construction,
but must preserve security fields, RequestId echo/suppression, first publication,
terminal ownership and failure timing. Re-reading later mutable state is not an
equivalent optimization. This differs from ordinary pending-header queue replay.
The preceding read-only callback audit passed eight response-head tests and seven
content-type tests. This documentation batch changed no runtime code; formatting,
workspace check/Clippy and documentation validation passed with existing unrelated
warnings. No new runtime tests or benchmark ran for this decision record.

## Skip unused AI retry gates for excluded transport errors

on_transport_failure now returns unclassified errors immediately after the
existing status-retry acknowledgment branch. Downstream/control-flow errors and
timeouts outside the backend-request class no longer read retry opt-in or evaluate
unused deadline/write-state gates. Recognized failure handling keeps its order;
the original error and retry decision remain untouched for excluded classes.

Independent plan review passed. A new production-adapter regression covers six
excluded error/source/timeout combinations with both relay states and incoming
retry decisions under an expired gRPC deadline. It checks error identity/text,
retry preservation, scheduler progress, attempt ledger and diagnostics. Existing
status-retry acknowledgment and recognized failure tests remain intact.

Fresh verification passed seven AI attempt and 37 outer callback tests (44 total),
formatting, workspace check/Clippy and seven guards, with existing unrelated
warnings. This removes source-level unused getter/clock work; no allocation or
throughput improvement is claimed. No full suite or live benchmark ran.

## Borrow body-driver presence before sharing ownership

The writable response-body callback now uses a body-specific context accessor
that checks the driver's existing immutable has_body flag through a borrowed Arc
before cloning it. Trailer-only drivers skip an Arc increment/decrement per body
event. Present body processors retain the same owned handle across execution.
The generic accessor remains unchanged for trailers, logging and cleanup.

Independent plan review passed. Frozen-state, oversized guarded chunks and stale
canonical-batch checks retain their order before the accessor. The old empty-body
branch already returned before the mutex lease and cancellation watcher, so this
change removes no synchronization boundary. It does not implement body-06's
separate terminal-finalizer lock-elision proposal.

The existing real trailer ordering test now first drives nonempty Data and
TerminalBeforeTrailers through the production body dispatcher, checking unchanged
bytes, an unterminated sink, open terminal state and zero companion calls. It
then verifies ordered, exactly-once trailer execution. Fresh verification passed
31 body, 11 trailer, three driver and six cancellation tests (51 total), formatting,
workspace check/Clippy and seven guards with existing unrelated warnings.
No full suite, real-network transport run or throughput benchmark ran.

## Read each borrowed IP key once

The shared key_value RemoteIp and DirectPeerIp branches now bind their respective
session getter once before checking emptiness and returning Cow::Borrowed.
Nonempty inputs previously invoked the same getter twice. Production getters
borrow RequestInfo fields directly; each evaluation still reads live state with
no cache, parsing, normalization or identity substitution. Empty strings remain
None, and the two IP sources remain independent.

Independent plan review passed. The existing distinct-identity regression now
covers empty inputs, distinct IPv4/IPv6 inputs and independently empty sources,
requiring exactly one getter call per key and borrowed nonempty results.
Baseline and final shared condition tests passed 68; final RealIp tests passed
34 (102 final tests). Formatting, workspace check/Clippy and seven guards passed
with existing unrelated warnings. This removes repeated getter calls; no
allocation or throughput improvement is claimed. No full suite or live traffic
benchmark ran.

## Move body-gate constructor requirements

BodyGatedRequestFilter::new and BodyGatedUpstreamResponse::new now take the
secondary conditions out of their owned BodyRequirement before compiling them,
then move the remaining requirement into BodyGatePolicy. This avoids cloning
and discarding the max-size string and complete condition tree during cold
construction. Compilation still precedes logical-limit parsing; the policy only
retains the parsed limit, failure mode and compiled conditions. The existing
with_conditions path and request-time body processing are unchanged.

Independent plan review passed. The cached-policy regression now uses both real
constructors and verifies 64 MiB, FailOpen, and POST Run / GET Skip decisions.
Existing secondary-regex, body-limit, capture and failure-policy tests remain.
Baseline body-gate tests passed 31; final verification passed 31 body-gate and
68 shared-condition tests (99 total), formatting, workspace check/Clippy and
seven guards with existing unrelated warnings. No full suite, live traffic or
allocation benchmark ran; no request-throughput improvement is claimed.

## Move generated RequestId strings with empty prefixes

RequestId generation now moves its owned suffix directly into HeaderValue when
its prefix is empty, avoiding the redundant formatted String construction.
Nonempty prefixes retain the existing concatenation. Random draws, generator
bounds, header validation and the final value check remain unchanged; request
identity ownership, suppression and access-log masking are unaffected.

Independent plan review passed. The existing five-generator shape regression
now covers empty and nonempty prefixes, suffix lengths, UUID versions and the
hex/NanoId alphabets. Fresh verification passed 29 request-trace-ID, 15 RequestId
and 17 early-request-filter tests (61 total), formatting, workspace check/Clippy
and seven guards with existing unrelated warnings. No full suite or allocation/
throughput benchmark ran; no fixed allocation count or capacity saving is claimed.

## Retain ProxyRewrite strings after unmatched regex stages

The argument, numeric-capture and named-path substitution stages now retain their
input String when regex returns Cow::Borrowed, accepting a new String only for
Cow::Owned. Locked regex 1.13.1 returns the original haystack on no match; matches
still build an owned result even when a replacement callback returns an unchanged
span. All regex calls, callbacks, getters, stage order, encoding and sentinel
handling remain intact. The initial URI replacement and final restoration are
unchanged, and staged-template compilation remains deferred separately.

Independent plan review approved the runtime change and the owning knowledge
sentence correction. The existing 21 ProxyRewrite tests passed before and after,
including both contexts, injected dollars, exact named spans and URI/header sinks.
Formatting, workspace check/Clippy and seven guards passed with existing unrelated
warnings. This removes no-match text copies but retains prior String capacity;
no retained-memory, total-allocation or throughput improvement is claimed.
No full suite, live integration or benchmark ran.

## Borrow published probe backends during endpoint projection

AllEndpointStatus's synchronous resolved_endpoint_from_backend helper now borrows
the published Backend instead of receiving a clone from each ready Service slot.
It only reads the address for canonical safety validation and formatting. The
returned address/name remain owned, and the same transport Arc is transferred.
Discovery and policy generations, serving ports, unsafe counts, selection order
and fan-out timing remain unchanged; endpoint-01's address-lifetime decision is
still deferred.

Independent plan review passed. The current discovery constructor publishes Inet
addresses with empty Extensions, so this removes an unnecessary Backend clone
without claiming a fixed heap-allocation saving. All 23 AllEndpointStatus tests
passed before and after, including the real Service-section/serving-port path.
Formatting, workspace check/Clippy and seven guards passed with existing unrelated
warnings. No full suite, live probe or throughput benchmark ran.

## Move non-retained raw JSON keys into duplicate detection

Gateway DSL JSON calls reach the canonical edgion-dsl raw parser. Its key visitor
already owns each decoded String; non-retained nested objects now move that String
into their duplicate-detection HashSet instead of copying it and discarding the
original. Retained top-level members still keep their original key and a separate
set owner. Cow distinguishes these cases only during admission; no persistent
representation changes.

The frozen supporting scope is edgion-dsl/src/json.rs and its mounted json_test.rs.
All conservative work charges, set reservation, insertion/duplicate rejection,
value traversal and member reservation keep their existing order. DOM parsing,
raw lexemes, limits and Gateway body ownership remain unchanged. Independent plan
review passed. The focused regression checks owned buffer transfer and both
ownership modes' duplicate rejection and exhausted-budget rejection before set allocation.
Existing nested escaped-key, reconstruction and boundary tests remain.

Baseline DSL JSON tests passed seven; final verification passed eight DSL JSON,
seven Gateway JSON builtin and 11 JSON Schema plugin tests (26 total), formatting,
workspace check/Clippy and seven guards with existing unrelated warnings.
This removes the redundant nested-key copy; no whole-path allocation count,
throughput benchmark, full suite or live integration result is claimed.

## Borrow plain decoded DSL JSON strings before Arc publication

Gateway json.get_string reaches edgion-dsl::json::decode_string. A private
serde-transparent Cow string wrapper now borrows unescaped input through decoding,
then copies into the same final owned Arc. Escaped values still use serde-owned
decoded text. This removes the intermediate String for plain input without a new
JSON parser or ownership crossing into the caller's source lifetime.

The frozen supporting scope is edgion-dsl/src/json.rs and its mounted json_test.rs.
Initial quote gating, pre-parse work charge, complete-input validation, final
copy charge and error mapping remain unchanged. Independent plan review passed.
Tests compare against the previous String decoding for ordinary/escaped text,
whitespace, trailing garbage, malformed strings and surrogate cases, verify plain
source borrowing, and check exhaustion before parsing and at the Arc-copy budget.

Baseline DSL JSON tests passed eight; final verification passed nine DSL JSON,
seven Gateway JSON builtin and 11 JSON Schema plugin tests (27 total), formatting,
workspace check/Clippy and seven guards with existing unrelated warnings.
No whole-path allocation count, throughput benchmark, full suite or live
integration result is claimed.

## Direct formatting of request-side streaming refusals

AiGuard's content-filter reply now formats the SSE prefix, completion JSON and
DONE suffix directly into the final String. The intermediate complete JSON
String and its copy into the SSE envelope are removed. Timestamp acquisition,
128-Unicode-scalar model truncation, canonical JSON string escaping/fallback,
field order and the non-streaming branch remain unchanged. This is a bounded
request-stage local reply, not an upstream response aggregation change.

The new matrix checks exact wire bytes and two-event framing for empty/plain,
escaped/control and permitted multibyte model names, including 128/129-scalar
boundaries. Fresh targeted tests passed two error-renderer and 79 request-guard
cases. Formatting, workspace check/Clippy and seven repository guards passed;
existing unrelated warnings remain. No throughput benchmark or full suite ran.

## Move the decoded request-guard refusal model

AiGuard request_facts now takes the decoded model String from its private,
soon-dropped JSON Value instead of copying it. Complete serde_json parsing,
object-only field lookup, last-duplicate-key behavior, non-string defaults and
subsequent stream-boolean lookup are unchanged. Rendering still owns the
128-scalar truncation; extraction retains the full decoded model. The full JSON
tree allocation remains and no selective-parser optimization is claimed.

The new fact-extraction matrix covers malformed/trailing input, non-object
roots, wrong field types, duplicate model/stream keys, escaped and permitted
multibyte strings, and a long model. Fresh targeted tests passed 80 request-guard
and two renderer cases. Formatting, workspace check/Clippy and seven repository
guards passed with existing unrelated warnings. No full suite or performance
benchmark ran; the benefit is removal of the extra model String copy.

## Serialize request evaluator spans without a projection Vec

AiGuard's evaluator now serializes the borrowed post-mask span slice through a
length-known iterator instead of collecting a second Vec of wire spans. Each
item retains the original RequestSpan/WirePart serializer, so wire field order,
text escaping and key projection stay unchanged. Empty/all-empty short circuits,
CappedWriter admission and error labels, encoding before the timeout/permit, and
strict decision parsing retain their existing boundaries.

A differential test compares the collected reference and direct encoding at
zero, below-exact, exact and above-exact caps, including empty lists, mixed span
kinds, unordered/duplicate keys, maximum indices and escaped/multibyte text.
Fresh verification passed 81 request-guard and two renderer tests, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. The removed allocation is the temporary wire-span Vec; final request
bytes and the earlier post-mask projection remain owned. No full suite or
throughput benchmark ran.

## Pin BasicAuth user generations only after a hit

BasicAuth now performs its username lookup through the plugin-owned immutable
map before cloning that map's Arc. An unknown user avoids the immediately
released Arc increment/decrement; a known user still pins exactly the same
generation for spawn_blocking verification. The fast-miss policy, password
verification, failure delay, anonymous behavior and masked/group log contracts
remain unchanged. No heap allocation reduction, constant-time property or
throughput improvement is claimed.

Existing known/unknown dispatch and pinned-generation regressions cover the
control-flow and lifetime boundary. Fresh verification passed 20 BasicAuth and
36 shared-auth tests, formatting, workspace check/Clippy and seven repository
guards with existing unrelated warnings. No full suite or benchmark ran.

## Prepare KeyAuth metadata header values once

KeyAuth now prepares metadata HeaderValues at construction instead of scanning
and parsing the same immutable name/value strings after each successful lookup.
Prepared values are marked sensitive immediately and cloned through the existing
live session setter. Raw names and source-map iteration order are retained,
including case collisions; failed writes still do not stop subsequent fields.
RPC sealing, deferred-credential mutation handling, authentication, failure delay
and credential hiding keep their existing owners. Exact-case username text is
retained separately before filtering so skipped values keep the same masked log
behavior and group attribution.

A differential regression compares the previous projection's attempted writes
with prepared writes for invalid names/values, control bytes, empty/tab/multibyte
values and case collisions. It checks sensitivity, shared payload identity and
continuation after setter failure. Fresh verification passed 16 KeyAuth,
36 shared-auth and one real typed-header adapter test, formatting, workspace
check/Clippy and seven repository guards with existing unrelated warnings.
This removes repeated validation and value payload copies; it does not eliminate
header-name work or promise zero allocations, lower total resident memory or
measured throughput. No full suite or benchmark ran.

## Borrow HMAC usernames until header verification succeeds

HMAC's parsed username now retains the quoted-value Cow through synchronous
header verification. Unescaped usernames borrow the Authorization input;
escaped usernames retain the existing decoded String. Only the successful
Verified outcome takes ownership before asynchronous body capture or session
mutation. Credential lookup still hashes the same str bytes, and parameter,
algorithm, date, signature and signed-digest checks keep their error order.
This removes an unescaped username copy on synchronous rejection paths without
changing the owned success result or its masked logging.

Existing tests now check the parsed username's borrowed pointer or escaped
ownership and consume successful results after dropping the session. Parser
validation and debug-redaction test identities remain intact. The owning wire
contract knowledge now describes the actual ownership boundary. Fresh runs
passed 56 HMAC-filtered, 68 condition-system and 36 shared-auth tests (filters
overlap), formatting, workspace check/Clippy and seven repository guards with
existing unrelated warnings. No full suite or throughput benchmark ran.

## Prepare HMAC metadata through the shared auth helper

HMAC now replaces its runtime credential's raw metadata map with prepared
sensitive HeaderValues. KeyAuth and HMAC share one cold preparation helper,
preserving the prior CR/LF/NUL filter, HeaderValue parse result, raw header names
and source-map iteration order. Each successful request clones values through
the existing live setter, retaining RPC sealing, deferred mutation handling and
continuation after a failed write. Secret bytes and group remain independently
owned by the same immutable verifier generation; no second credential index or
cache was introduced. Signature initialization is unchanged.

The Controller resolves condition HMAC with an empty metadata whitelist, so
ordinary condition credentials require no prepared-header backing allocation.
Directly constructed nonempty condition metadata still has no publication path.
This removes repeated metadata scans, parsing and value payload copies on
standalone successful requests, without promising zero request allocations or
lower total resident memory for every configuration.

A differential HMAC regression covers source-config independence, secret/group
ownership, case collisions and write order, skipped control/invalid values,
invalid names passed to the live setter, sensitive shared payload clones,
continued writes after errors and zero header capacity for empty metadata.
KeyAuth's existing differential regression covers the shared preparation too.
Fresh verification passed 57 HMAC-filtered, 16 KeyAuth, 36 shared-auth,
68 condition-system and one typed-header adapter test (filters overlap),
formatting, workspace check/Clippy and seven repository guards. After correcting
a new fixture initialization warning, HMAC tests and workspace check/Clippy
passed again; only existing unrelated warnings remain. No full suite or
throughput benchmark ran.

## Copy verified JWT issuer only after expiration admission

JWT verification now borrows the issuer from its verified claims while applying
the existing maximumExpiration checks, then copies it when constructing the
owned success result. InvalidNumericDate and ExpTooFar returns avoid the unused
issuer String. Issuer extraction remains before those checks; issuer routing,
algorithm and scoped-key binding, signature validation, clock reads, numeric-date
rounding, denial precedence and logs are unchanged. Successful results still own
both the issuer copy and complete claims; this does not remove either JWT decode.

The existing success test now drops the token before inspecting issuer and claims.
Existing expiration, fractional-date, group-isolation and redaction regressions
remain intact. Fresh verification passed 47 JWT and 36 shared-auth tests,
formatting, workspace check/Clippy and seven repository guards with existing
unrelated warnings. No full suite or throughput benchmark ran.

## Select admitted claim-array scalars without measuring again

The shared auth claim-header materializer now selects String, Number and Bool
array entries by their JSON variants instead of calling scalar_measure again.
The sole caller still measures the complete immutable candidate, checks unsafe
bytes and admits per-value/aggregate budgets before materialization. This removes
one redundant string control-byte scan and one number formatting pass per such
array element; measurement and final rendering remain. Commas, empty scalar
entries, skipped objects/null/nested arrays, source order, UTF-8 byte limits,
failed-write accounting and skip-versus-stop precedence are unchanged.

The existing array regression now covers every JSON variant, both booleans,
integer extremes, a float and a trailing empty string at exact and undersized
caps. Fresh verification passed 36 shared-auth, 47 JWT, 150 OIDC and 20 JWE tests;
16 OIDC tests retained their existing ignored status. Formatting, workspace
check/Clippy and seven repository guards passed with existing unrelated warnings.
No full suite or throughput benchmark ran.

## Move OIDC typed claims into their JSON result

The only production consumer of Claims conversion, OIDC JWT verification, now
consumes the decoded Claims through into_value. Issuer, subject, audience and
custom-field keys/subtrees move into the same freshly built JSON map instead of
being cloned before their source owner is dropped. Standard-field insertion
order and precedence remain unchanged: present fields win even for a null
audience, absent fields permit extra fallback, and non-object extra is ignored.
JWT validation, required-scope checks and later ctx/header publication keep their
existing ordering and owners. No schema or serialization contract changed.

Focused regressions cover all six standard-field collisions, missing-field
fallback, non-object extras and pointer identity for nonempty issuer/subject,
audience and nested extra strings. The existing standard-fields test retains its
identity. Fresh verification passed 38 shared-auth, 150 OIDC, 47 JWT and 20 JWE
tests; 16 OIDC tests retained their existing ignored status. Formatting,
workspace check/Clippy and seven repository guards passed with existing unrelated
warnings. This removes payload/subtree copies, not the output map or standard-key
allocations. No full suite or throughput benchmark ran.

## Borrow the selected OIDC introspection endpoint

Introspection endpoint selection now borrows from immutable plugin configuration
or the local discovery Arc snapshot instead of cloning a String before cache
lookup. The nonempty configured endpoint still takes precedence over discovery.
Discovery fetch and missing-endpoint rejection remain before cache lookup;
cached claims still undergo claim/scope validation, while URL admission and
request construction remain on cache miss. Borrowing the owned snapshot retains
no discovery lock across the await. Reqwest still owns its parsed request URL.

A network-free cached-call matrix covers absent/empty endpoint combinations,
discovery fallback availability and missing-endpoint-before-cache versus
URL-admission-after-cache behavior. When both endpoints are nonempty, selection
precedence is established by source review; a cache hit does not consume the URL
and cannot distinguish the selected destination. Fresh verification passed
151 OIDC tests with 16 existing ignores, formatting, workspace check/Clippy and
seven repository guards with existing unrelated warnings. No full suite or
throughput benchmark ran.

## Borrow OIDC token-operation form values

Authorization-code exchange, token refresh and revocation now borrow form values
from their arguments and immutable plugin generation instead of allocating a
String for each value before encoding. Field order, optional PKCE/hint fields,
Basic/Post authentication branches, endpoint admission and failure handling are
unchanged. The locked reqwest 0.12.28 form builder synchronously serializes the
values into its owned request body; prepared-secret access holds no lock.
The form Vec and encoded body still allocate. Introspection already borrows its
form values and needed no change.

Fresh verification passed 151 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards. Existing unrelated Clippy
warnings remain. Serialization equivalence follows from unchanged tuple order
and serde's String/str serialize_str implementations; no new mirror test or
network fixture was added. No full suite or throughput benchmark ran.

## Move the OIDC logout session reference after revocation

Logout's read-only revocation helper now borrows its session cookie. After it
returns, the sole caller consumes the cookie and moves session_ref into cache
removal instead of cloning that String before revocation. ID-token hint removal,
refresh/access-token revocation, best-effort failure handling, cache eviction and
redirect ordering are unchanged. Other cookie fields are dropped before cache
removal; the change does not claim earlier token release than the old owned
helper, and does not retain the cookie across redirect construction.

Fresh verification passed 151 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. The existing logout regression checks cache eviction at response write,
ID-token hint, clearing cookies and continued logout after revocation failure.
No new mirror test, full suite or throughput benchmark was run. The scoped
benefit is one avoided nonempty session-reference allocation/copy per logout
with a decoded session cookie.

## Borrow the OIDC logout endpoint from discovery

Logout redirect construction now retains the cached discovery Arc snapshot and
borrows its end_session_endpoint for URL parsing, removing the temporary endpoint
String clone. The provider releases its read guard before returning the snapshot;
there is no later await in this builder. Missing/empty endpoints and parse errors
still return the same fallback. Query append order and final URL serialization
remain unchanged. The parsed Url retains its own storage.

Fresh verification passed 151 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. Existing logout coverage exercises no-session fallback, post-logout
redirect construction and ID-token hints. No new mirror test, full suite or
throughput benchmark ran. Moving the final Url serialization was not included:
the spare-capacity ownership tradeoff already identified in outbound-02 also
requires assessment for these redirect outputs.

## Borrow the sanitized OIDC callback redirect target

sanitize_redirect_target now returns its accepted input slice or the static root
path instead of allocating a String. Its sole production caller owns the decoded
state cookie throughout response delivery and passes the borrowed target directly
to the existing redirect writer. The path predicates and resulting bytes remain
unchanged, including backslash and protocol-relative rejection; Location still
receives its independently owned header value. No lock or cache was introduced.

The existing cross-origin regression retains its identity and security cases,
adds empty/relative inputs, and checks pointer identity for an accepted owned
path. Fresh verification passed 151 OIDC tests with 16 existing ignores,
formatting, workspace check/Clippy and seven repository guards with existing
unrelated warnings. This removes the temporary redirect-target allocation, not
the response header allocation. No full suite or throughput benchmark ran.

## Use fixed cookie arrays for OIDC redirects

Authorization redirect, callback completion and logout now pass fixed arrays to
the existing cookie-slice writer instead of constructing one- or two-element
Vecs. Cookie order, first-header insertion and subsequent append behavior,
owned cookie text, response headers and failure handling remain unchanged.
Each selected branch avoids the outer Vec heap allocation. The two-element array
stores more String descriptors inline than a Vec handle, so this is not a claim
of measured net-memory or throughput improvement; string/header allocations remain.

Fresh verification passed 151 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. Existing redirect/logout tests retain their cookie and response checks.
No collection-shape-only test, full suite or throughput benchmark was added/run.

## Borrow the OIDC ID-token audience slice

Nonce validation now passes slice::from_ref over the immutable generation's
client_id to the existing JWT verifier instead of cloning that String into a
single-element Vec. The audience override contains the same value, with the same
JWT, expiration, scope and nonce validation order. The existing self borrow
already covers the awaited verifier. Validation::set_audience still constructs
its owned audience set; this removes only the caller's temporary Vec and string.

Fresh verification passed 151 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. The existing signed-token regression covers client-ID audience override,
wrong audience, nonce mismatch and old-generation policy retention. No new
mirror test, full suite or throughput benchmark ran.

## Borrow OIDC redirect host and scheme components

Path-form callback and post-logout URI construction now uses a static scheme
and a Host slice borrowed from the request HeaderMap. The two synchronous
consumers still format their final owned URI. TLS false selects http; TLS true
or unknown selects https. Host keeps first-comma selection, trimming and nonempty
validation, while forwarded Host/proto remain untrusted. No URL validation,
missing-host outcome, final allocation or configuration behavior changed.

A focused regression covers borrowed Host identity, trimmed first values,
empty/whitespace/absent Host and an empty first comma component. Existing
forwarded-header and TLS-state tests remain. Fresh verification passed 152 OIDC
tests with 16 existing ignores, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. The scoped benefit is
removing intermediate host and scheme String allocations for path-form URI
composition. No full suite or throughput benchmark ran.

## Compare OIDC reserved authorization names without lowercase allocation

The Gateway's defensive authorization-parameter predicate now compares the same
eight literal names with eq_ignore_ascii_case rather than allocating a lowercase
String first. ASCII case handling, no-trim behavior, reserved-name membership and
callback parameter iteration/encoding order remain unchanged. Resource admission
still rejects reserved names through its existing cold validation. This change
removes the runtime temporary allocation for nonempty keys; it does not claim a
measured CPU or throughput improvement for the bounded comparison sequence.

Focused tests cover every name in lower/upper and single-letter mixed case,
whitespace, prefixes/suffixes, empty text, non-ASCII and separator boundaries.
Existing authorization-parameter response tests remain. Fresh verification passed
153 OIDC tests with 16 existing ignores, formatting, workspace check/Clippy and
seven repository guards with existing unrelated warnings. No full suite or
throughput benchmark ran.

## Borrow the OIDC HTTP client accessor

OpenidConnect::http_client now borrows its generation-owned reqwest Client rather
than cloning the outer Arc. Its five production uses immediately construct
requests for token exchange, refresh, revocation, introspection and userinfo.
Reqwest's request builder still owns its internal Client clone. Plugin/provider
Arc ownership, per-instance isolation, connection pools, TLS/egress policy and
request cancellation remain unchanged. This removes the accessor's outer Arc
increment/decrement, not reqwest's request ownership or a payload allocation.

The existing distinct-client test retains its Arc identity check and verifies
that each accessor points into its own client. Fresh verification passed 153
OIDC tests with 16 existing ignores, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. No full suite or throughput
benchmark ran. JWKS/discovery snapshot Arcs remain necessary for caller ownership
outside provider state locks and were not replaced by borrowed cache references.

## Compute OIDC refresh coordination keys only when needed

Session authentication now starts with an empty request-local optional refresh
key. Cache-miss, preemptive and expired-token refresh branches initialize that
same slot on first use and reuse it thereafter. The digest still uses the
original encrypted cookie text, never a refreshed payload. Requests taking no
refresh branch avoid SHA-256 over the cookie and the Base64 output allocation.
Refresh predicates, timeout creation, flight/result-cache identity, cookie writes,
stale-token fallback and diagnostics remain unchanged. No shared cache or new
coordination mechanism was introduced.

Fresh verification passed 153 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. Existing request tests seed results and occupy flights using the raw
cookie digest, alongside the stable-key and timeout regressions. Lazy computation
and reuse at all three sites are established by source dataflow, not a new
counter or an Option-internals test. No full suite or throughput benchmark ran.

## Avoid copying the token cleared from persisted OIDC sessions

Session-cookie publication now explicitly copies the six-field payload projection.
For a nonblank access token, the persisted projection starts with an empty token
and caches the original source bytes, avoiding the copy that was immediately
cleared. Empty and whitespace-only tokens preserve their previous representation
and skip cache insertion. Session-reference generation, remaining fields, TTL,
cache-before-cipher/size-check ordering and response append behavior are unchanged.
The input payload is not mutated; other field and cache-owned copies remain.

A six-case roundtrip matrix covers normal/empty/whitespace tokens with existing
or generated references, source immutability, all persisted fields and exact cache
contents. The original persistence test remains. Fresh verification passed 154
OIDC tests with 16 existing ignores, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. This removes one temporary
nonblank access-token buffer, not encryption or serialized-cookie storage. No
full suite or throughput benchmark ran.

## Borrow the OIDC logout fallback target

Logout target selection now borrows immutable configured text or the static root
path. Building an IdP logout URL copies the fallback only when discovery has no
usable endpoint or endpoint parsing fails. Path-form post-logout URI expansion
formats the borrowed target directly; absolute URI results retain their owned
output. Prefix acceptance, Host/TLS derivation, query order, revocation, cookies
and response ownership remain unchanged. No lock is held by the borrowed target.

A focused test covers configured-pointer identity and the existing empty, relative,
absolute, case-sensitive and slash-prefix rules. Fresh verification passed 155
OIDC tests with 16 existing ignores, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. This removes temporary target
copies, not the final redirect String. No full suite or throughput benchmark ran.

## Use fixed OIDC authorization random input arrays

Authorization state, optional nonce and optional PKCE verifier generation now use
const-sized 24/64-byte arrays instead of transient Vec buffers. All three call
sites retain their byte counts, RNG call order, ThreadRng fill_bytes operation and
URL-safe unpadded Base64 encoding. The encoded String remains owned. This removes
one input-buffer heap allocation per generated value, at the cost of small bounded
stack buffers and two generic instantiations; no measured speedup is claimed.

A protocol test constructs an authorization redirect and decrypts its state cookie,
checking query/cookie state and nonce identity, decoded byte counts, encoded lengths
and alphabet, original URL and the S256 challenge of the stored verifier. Existing
tests remain. Fresh verification passed 156 OIDC tests with 16 existing ignores,
formatting, workspace check/Clippy and seven repository guards with existing
unrelated warnings. No full suite or throughput benchmark ran.

## Decode OIDC cookie nonces into a fixed array

Cookie decoding now uses the safe Base64 decode_slice API with a 12-byte nonce
array and checks its returned length. Valid nonce text reaches the same ciphertext
and AEAD stages; short, oversized or malformed nonce text still returns the fixed
MalformedCookie classification. Decoder details remain private. This removes the
nonce Vec allocation and bounds nonce output storage without changing ciphertext
storage, cookie encryption, JSON decoding or error projection.

Locked base64 0.22.1 uses the same internal decoder for both APIs; canonical
16-character nonce text fits the array exactly. Tests cover decoded lengths
0/1/11/12/13/64, padding, alphabet and trailing-bit failures, invalid ciphertext,
and a real encrypted cookie with a changed nonce returning InvalidCookieSignature.
Existing roundtrips remain. Fresh verification passed 157 OIDC tests with 16
existing ignores, formatting, workspace check/Clippy and seven repository guards
with existing unrelated warnings. No full suite or throughput benchmark ran.

## Retain the default OIDC JWT required-claim set

OIDC JWT verification no longer replaces Validation::new's required-claim set
with another identical set containing exp. Locked jsonwebtoken 9.3.1 constructs
that set and owned claim name in new; the removed setter allocated replacements.
The per-verification Validation object and all issuer, audience, algorithm, time
and nonce policies remain unchanged. A source comment preserves the requirement
that exp remains mandatory even when expiration comparison is disabled.

A six-case real-signature test through the network-free scripted JWKS provider
covers missing/future/past exp with validate_exp enabled and disabled. Missing exp
still returns InvalidToken in both modes; expired tokens return Expired only when
comparison is enabled. Successful cases omit nbf, aud and sub to preserve their
non-required status. Existing tests remain. Fresh verification passed 158 OIDC
tests with 16 existing ignores, formatting, workspace check/Clippy and seven
repository guards with existing unrelated warnings. This removes redundant
HashSet/String construction, not all validation allocations. No full suite or
throughput benchmark ran.

## Compare OIDC JWK algorithm metadata without text conversion

jwk_matches_alg now compares KeyAlgorithm directly with an exhaustive mapping
from the token Algorithm. It no longer allocates a temporary String and parses
it back to an Algorithm for each candidate with algorithm metadata. All twelve
signature variants preserve their correspondence; the three encryption-only
KeyAlgorithm variants remain rejected. Absent metadata, key-family and curve
checks, kid lookup order and forced-refresh behavior are unchanged.

A differential test covers all fifteen metadata variants against twelve token
algorithms across RSA, P-256, P-384, octet and Ed25519 fixtures (900 comparisons).
Its oracle retains the former locked jsonwebtoken 9.3.1 text conversion alongside
the metadata-absent family result. Existing curve and selection tests remain.
Fresh verification passed 159 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. This removes metadata conversion work, not DecodingKey construction
or cryptographic verification. No full suite or throughput benchmark ran.

## Avoid formatting discarded OIDC cookie-limit errors

The private ensure_session_cookie_size_limit helper now returns a unit error
instead of formatting the rejected length and limit into a String. Both production
callers only inspect is_err(): callback still sends its fixed 500 response, and
session publication still returns without appending the cookie. The byte-length
comparison, threshold, cache/encoding order and all successful paths are unchanged.
This removes a temporary formatting allocation only on oversized cookies.

Existing success/oversize tests remain without new implementation-mirroring tests.
Fresh verification passed 159 OIDC tests with 16 existing ignores, formatting,
workspace check/Clippy and seven repository guards with existing unrelated
warnings. No full suite or throughput benchmark ran.

## Avoid rescanning LDAP username leading whitespace

LDAP authorization parsing now computes trim_start once, uses its byte offset,
and applies trim_end to that remaining slice. Previously trim scanned both ends
and trim_start scanned the leading whitespace again for the offset. The decoded
String owner, UTF-8 slice ranges, empty-username/password rejection, first-colon
split and untrimmed password are unchanged. Header priority, DN escaping, per-bind
connections, deadlines and authentication outcomes keep their existing behavior.

The existing backing-buffer test retains its identity and now covers leading,
trailing, ASCII and permitted Unicode whitespace, UTF-8 usernames, all-whitespace
rejection and password spaces/colons. Fresh verification passed all 28 LDAP tests,
formatting, workspace check/Clippy and seven repository guards with existing
unrelated warnings. The change avoids a duplicate leading-whitespace scan; it
removes no allocation and claims no measured throughput gain. No full suite or
network integration test ran.

## Borrow JWE protected-header member names

ProtectedHeaderVisitor now deserializes member names through a private
Serde borrowing Cow wrapper. Unescaped names borrow the decoded protected-header
buffer; escaped names retain owned decoded text. The parser-local HashSet still
rejects every duplicate decoded name, including unknown members and mixed
escaped/unescaped spellings. Value parsing, failure order, algorithm checks and
the original encoded protected segment used as AES-GCM AAD are unchanged.

Existing duplicate tests now cover both mixed-spelling orders for known and
unknown members. A successful-decryption test covers escaped known names and
unknown names with escapes, retaining raw AAD semantics. Fresh verification
passed all 21 JWE tests, formatting, workspace check/Clippy and seven repository
guards with existing unrelated Clippy warnings. The change removes per-name
String allocations for unescaped names; the HashSet and escaped-name allocations
remain. No full suite or throughput benchmark ran.

## Avoid repeated lookup when merging extra OIDC claims

Claims::into_value now uses the JSON map Entry API for each owned extra field,
replacing contains_key followed by insert. Vacant keys avoid a second lookup;
occupied standard claims still win, including an explicit null audience. Missing
standard fields still accept their extra counterparts, non-object extras remain
ignored, and existing key/value buffers move without cloning. OIDC token
verification remains the only production consumer of this Claims conversion.

Existing precedence/fallback and buffer-identity tests cover this change without
new mirrored tests. Fresh shared-auth and OIDC tests, formatting, workspace
check/Clippy and seven repository guards passed with existing unrelated Clippy
warnings and 16 existing OIDC ignores. This reduces map lookup work, not claims
serialization or overall allocation count. No full suite or benchmark ran.

## Transfer rendered claim values into sensitive headers

The shared claims projection now transfers its owned number/array String into
HeaderValue instead of copying its text again. Borrowed string/boolean values
keep the existing from_str path. Both conversions use the same byte validation
and the canonical private sensitive-header setter; failed conversion or mutation
still skips the write without charging the aggregate budget. Plan order, safety
checks and pre-materialization byte caps are unchanged.

The existing invalid-header budget regression now covers both borrowed text and
an owned array rendering containing DEL. Fresh verification passed 38 shared-auth,
159 OIDC, 47 JWT and 21 JWE tests (16 existing OIDC ignores), formatting,
workspace check/Clippy and seven repository guards with existing unrelated
Clippy warnings. This eliminates a second payload copy for owned renderings;
Bytes metadata allocation can still depend on retained capacity. No universal
allocation-count or measured throughput claim is made. No full suite ran.

## Borrow fixed endpoint probe error text

AllEndpointStatus's private Serialize-only EndpointResult now holds an optional
static error string. failed_endpoint already receives static classified errors;
wall-timeout assembly also uses a fixed literal. Both now retain those references
instead of allocating copied Strings. Success still serializes a null error;
classification text, raw-byte totals, timeout scheduling and probe ordering are
unchanged. Transport policy, TLS, address safety and dependency-error redaction
retain their existing implementations.

The assembly regression checks serialized success/timeout errors, and a focused
failed-result test checks fixed text, null status/body and omitted headers.
Fresh verification passed all 24 AllEndpointStatus tests, formatting, workspace
check/Clippy and seven repository guards with existing unrelated Clippy warnings.
This removes copied error-text buffers only; response JSON and other endpoint
fields still allocate. No live probe integration or throughput benchmark ran.

## Format ProxyRewrite capture markers in stack storage

The regex-URI capture loop now formats each present capture's dollar-prefixed
index into a bounded stack buffer instead of a temporary String. Capacity covers
one dollar plus every decimal digit of usize (21 bytes on 64-bit targets); only
ASCII primitive formatting reaches the two statically justified expect calls.
Replacement order, capture-dollar escaping, unmatched groups and query handling
are unchanged. The separate rewrite-01 grammar decision remains open.

The new regression passed against the old implementation before the change and
covers two-digit indices, sequential prefix replacement and unmatched groups.
Final verification passed all 22 ProxyRewrite tests, formatting, workspace
check/Clippy and seven repository guards with existing unrelated Clippy warnings.
This removes temporary marker allocations, not replacement-result allocations.
No full suite or throughput benchmark ran.

## Borrow literal JSON Patch expected values

ProxyRewrite's patch-value resolver now returns Cow: configured JSON literals
borrow their immutable compiled owner, while rendered templates remain owned.
The read-only Test operation no longer deep-copies its expected literal tree.
Add/Replace retain their required owned values through into_owned after the same
profile and work checks; their successful clone count is unchanged, while a
rejected literal no longer incurs that discarded clone. Profiling, comparison,
failure ordering, request-body publication and physical limits remain unchanged.

A focused actual-operation regression checks literal loan provenance, nested
equality/mismatch, exact and exceeded profiling/comparison work budgets, and an
unchanged document. Existing tests cover ordered operations, atomic publication
failure and bounded template strings. Fresh verification passed all 23
ProxyRewrite tests, formatting, workspace check/Clippy and seven repository
guards with existing unrelated Clippy warnings. Profiler traversal and mutable
patch operations still allocate; no full suite or throughput benchmark ran.

## Borrow the standard CORS allow-origin wildcard

Cors's selected ACAO output now uses Cow with a static lifetime: the standard
wildcard borrows the literal star, while accepted echoed origins retain their
owned String before session mutation or asynchronous preflight handling. Header
selection, validation, emission order and final insertion remain unchanged.
The separate Max-Age preparation decision in cors-01 remains open.

The existing warmed allocation matrix now verifies zero temporary allocations
for an accepted standard wildcard and retains one for nonempty echoes; rejected,
missing and non-text origin cases are unchanged. The selector test also checks
the borrowed variant. Both owning knowledge pages reflect the new return type.
Baseline verification passed 16 plugin tests; final verification passed all 45
Gateway CORS-filtered tests, formatting, workspace check/Clippy and seven
repository guards with existing unrelated Clippy warnings. This removes the
temporary star String only; final header insertion still copies bytes. No full
suite or throughput benchmark ran.

## Keep fault-injection delays in millisecond form until sleep

FaultInjection's private delay kind and sampler now retain the original u64
millisecond values. Runtime logging passes the sampled integer to the canonical
PluginLog formatter, then constructs the same Duration at the sleep sink. This
removes the temporary decimal String and Duration-to-millisecond round trip;
it introduces no integer narrowing or shared log-argument layout change.
Fixed/range selection, equal-bound RNG bypass, inclusive sampling, independent
delay/abort dice order and log/sleep/abort ordering remain unchanged.

Existing sampling tests retain their assertions in millisecond units. New tests
cover private zero/u64 extremes without RNG use or long sleeps and the actual
zero-delay log/continue path. The schema's 30,000 ms admission cap is unchanged;
private u64::MAX coverage does not imply an admitted configuration. Baseline
verification passed seven fault-injection tests; final verification passed nine
fault-injection and 34 shared PluginLog tests, formatting, workspace check/Clippy
and seven repository guards with existing unrelated Clippy warnings. No timer
latency guarantee, full suite or throughput benchmark is claimed.

## Transfer selected webhook JSON strings

Webhook response extraction now traverses its disposable parsed JSON tree mutably
and takes a selected String instead of cloning it. Index/key decisions, null and
composite handling, malformed-input failure and response bytes are unchanged.
Only the local parsed tree is modified; outbound limits and final context-setter
ownership remain unchanged. The locked serde_json string visitor creates owned
text from the parsed string slice, which can move directly to the existing result.

Differential tests compare the prior copied selection across numeric/empty path
segments, arrays, strings, null, scalars, composites and malformed inputs. A warmed
thread-local measurement observes one fewer allocation for a nonempty string
selection; JSON parsing and other result paths still allocate. No throughput or
whole-extractor allocation-free claim is made.

Baseline verification passed 45 provider tests. The first final parallel run had
46 passes and one existing TLS audit-count failure in
candidate_comparison_does_not_parse_material_or_emit_tls_audit (zero versus one).
That preparation path does not call response extraction; its parallel failure
mechanism was not established. The unchanged-source serial rerun passed all 47
provider tests, followed by four WebhookKeyGet tests. Formatting, workspace
check/Clippy and seven repository guards passed with existing unrelated Clippy
warnings. No live webhook or full suite ran.

## Short-circuit ineligible response-head encoding scans

Guardrail's final response-head Hold gate now checks its existing final-response
eligibility and 2xx status before calling the canonical identity-encoding
classifier. Already rejected responses skip repeated Content-Encoding traversal.
All predicates are pure; the same nonretryable 503, owner outcome, usage and limits
are published on rejection. Eligible responses retain the shared all-field/list
classification. gRPC/unary dispatch and Hold ownership remain unchanged.

Baseline and final verification each passed eight response-head tests and seven
content-type tests. Formatting, workspace check/Clippy and seven repository guards
passed with existing unrelated Clippy warnings. This saves unnecessary scanning
on rejected heads, not allocations; no throughput benchmark or full suite ran.

## Borrow Guardrail response Content-Type

Guardrail response admission now reads Content-Type directly from the canonical
borrowed response header map instead of materializing a temporary String through
get_response_header. The first field, HeaderValue::to_str rejection and shared
SSE classifier remain unchanged. The borrow ends before mutable session access;
admission order, encoding checks, failure policy and processor ownership are
unchanged. This removes a temporary copy for nonempty accepted text, not all
plugin allocations.

A real PingoraSessionAdapter regression passed before and after the source change.
Its ten cases cover missing/empty fields, mixed case, parameters, wrong media
types, duplicate first-value selection, invalid bytes and non-ASCII UTF-8 text.
It checks processor admission versus 503 and unchanged response headers, using
an in-memory duplex session without an outbound call. Final verification passed
two Guardrail plugin tests and seven content-type tests, formatting, workspace
check/Clippy and seven repository guards with existing unrelated Clippy warnings.
No throughput benchmark or full suite ran.

## Borrow optional Guardrail decision text

The decision envelope now retains optional code/reason strings through a private
transparent Serde wrapper around a directly borrowed Cow field. This is necessary
because the locked derive's borrowing adapter does not apply directly through
Option<Cow>. Plain text borrows the bounded response buffer; escaped text still
uses Serde's owned decoding. Replacement text remains independently owned.
Null/missing fields, duplicate rejection, byte limits, character checks and error
precedence are unchanged; optional diagnostic text remains unexposed.

The semantic regression passed against the original parser before the change.
Final verification passed all ten protocol tests, including input-pointer and
escaped-ownership checks and zero allocations for the prepared plain rejection
fixture with both optional fields. Formatting, workspace check/Clippy and seven
repository guards passed with existing unrelated Clippy warnings. This removes
two discarded String allocations for that nonempty unescaped fixture, not all
parser allocations. No throughput benchmark or full suite ran.

## Reuse callback-local Guardrail remaining-byte accounting

Guardrail rollback reservation no longer rescans every remaining record and its
raw slices at each decision. An available canonical batch already owns a checked
raw-byte total. The callback retains that scalar and subtracts each record's saved
length only after successful window admission. Failed admission and temporary
requeue/retry leave the count unchanged, so the pending record remains reserved.
Unavailable/fallback batches keep their previous path. Checked arithmetic,
inclusive capacity bounds, input summaries, failure ordering and EOS are unchanged.

The actual two-callback regression passed before and after the change. It forces
two successive full-window release/retry transitions with multi-slice and empty
records, then checks all output bytes and summaries. Boundary tests retain an
independent suffix-sum reference and cover both addition overflows, missing totals
and exact limits. Final verification passed 19 network-free response tests and
12 canonical-batch tests, formatting, workspace check/Clippy and seven repository
guards with existing unrelated warnings. Six existing wiremock response tests
were excluded from this unit-only run; no integration or full suite ran.

This removes repeated suffix traversal, which can revisit the same tail across
many decisions in a bounded batch. It does not make all window processing linear
or establish a throughput or allocation improvement.

## Stop impossible contiguous SSE range searches

SliceChain::range now stops searching for a single containing slice as soon as
the requested range crosses a slice boundary. Every later slice starts after
the requested start, so it cannot supply the borrowed result. The existing
canonical range walker still constructs the same owned fallback. Empty and
out-of-bounds handling, coalesced storage, borrowed pointers, allocations,
capacity admission and decoder/window lifecycle remain unchanged.

The regression enumerates every valid range over four layouts, including empty
input slices, and compares bytes against a contiguous oracle. It checks Cow
ownership and exact borrowed pointers, plus coalesced storage and bounds. All
five chain tests passed on the old implementation before the production edit.
Final verification passed 88 SSE-filtered tests, 33 native Anthropic tests and
56 Chat-to-Anthropic conversion tests, formatting, workspace check/Clippy and
seven repository guards. Existing unrelated Clippy warnings remain.

The change removes an impossible suffix search for fragmented ranges. It adds
a branch to the sliced lookup and the table is already bounded at 256 handles;
no universal CPU, allocation or throughput improvement is claimed. No full
suite or live-provider integration test ran.

## Skip character-counter dispatch for ignored SSE line tails

DataCharObserver::feed_segment now stops processing a segment once its line state
is Ignore. The old per-byte Ignore arm returns zero without modifying state, and
only finish_line resets it. The main decoder still validates complete field lines
and records malformed events. The degraded feed_opaque path still scans framing
and invokes finish_line, so ignored content does not hide subsequent data fields.
UTF-8 accounting, semantic newlines, saturation and quota policy are unchanged.

A differential regression compares per-segment counts and every observer field,
including pending UTF-8 state, with the original bytewise dispatch. It covers
control fields, data-prefix near misses, empty data, malformed bytes and split
UTF-8. An actual decoder regression checks every two-way split of a long comment
followed by an invalid event field and valid data, both normally and after forced
degradation. It verifies the character total and retained malformed-event signal.
All 48 SSE module tests passed before the production edit. Final verification
passed 90 SSE-filtered tests and 52 usage tests, formatting, workspace
check/Clippy and seven repository guards; existing unrelated warnings remain.

This removes repeated zero-result state dispatch in ignored line tails. Active
data scanning gains a branch; no universal CPU, allocation or throughput gain is
claimed. No full suite or live-provider integration test ran.

## Stop outlier arbitration scans at the ejection cap

Identity-bound Service failure completion now stops counting open endpoint states
once the normalized ejection cap is reached. is_open only reads state under its
mutex; it does not expire cooldowns, recover endpoints or update metrics. Later
observations cannot reduce the count already accumulated, so they cannot change
the denial. Concurrent success already could clear a previously observed state
without being revisited by the original scan; no coherent-snapshot guarantee is
added or removed.

The current coordinator membership, exact policy token, endpoint completion
lock, service ejection lock and candidate revalidation remain unchanged.
Zero-percent bypass is preserved, and positive caps are normalized to at least
one before limiting the iterator. Below-cap scans still inspect the full map.
No shared counter or retained state is introduced.

The existing actual-fixture percent-cap regression now covers both 1% and 25%
of four endpoints, pinning the rounded-down minimum-one case. It passed on the
old production code before editing. Baseline and final serial verification
passed 36 outlier-detection tests and 41 coordinator tests, including concurrent
caps, retirement, recovery and policy replacement. Formatting, workspace
check/Clippy and seven repository guards passed with existing unrelated warnings.

The benefit is fewer map entries and endpoint read locks on cap-reached scans,
depending on iteration order. No allocation, throughput or contention benchmark
claim is made; no full suite or live integration test ran.

## Stop credential ejection scans at the observed cap

AiCredentialOutlierBackendHandle::failure_allowed now limits its filtered
open-state count to the normalized positive percentage cap. Once enough open
credentials are observed, additional read-only is_open calls cannot change
denial. Low-count scans still exhaust the map. No-policy rejection, zero-percent
bypass, minimum-one allowance and the existing alias-count denominator remain
unchanged.

The independent AI wrapper retains its generation-stable credential handles,
backend arbitration lock and final candidate revalidation. Explicit provider
cooldowns and half-open states still count as open; the scan neither expires
them nor acquires probes. Concurrent alias reconciliation and success retain the
existing observation semantics, not an atomic membership/state snapshot.
No extra counter, cache or shared owner is introduced.

The actual-store percent test covers both 1% and 50% of two credentials.
A barrier-driven regression records simultaneous failures for four captured
credentials and checks exactly two openings and rejected admissions at 50%.
Both cap fixtures use long ejection deadlines and no sleeps. All 17 credential
tests passed before the production edit. Final serial verification passed
17 credential tests, 25 attempt-runtime tests and 36 shared outlier tests,
formatting, workspace check/Clippy and seven repository guards. Existing
unrelated warnings remain.

This skips trailing map entries and state locks after observing the cap; savings
depend on iteration order. No allocation, throughput or contention benchmark
claim is made. No full suite or live integration test ran.

## Admit OIDC literal addresses without a singleton vector

OIDC outbound URL admission now calls a scalar Gateway egress adapter instead of
constructing a one-element Vec and checking the filtered result for emptiness.
The policy match lives in one common is_ip_allowed_with_observer helper; the
existing batch filter delegates every address to that same helper. The common
crate change is supporting work for this Gateway path, not a broader crate audit.

IP classification, mapped IPv6 handling, optional private-address blocking and
loopback short-circuit order are unchanged. Common unit-test/integration-mode
exceptions and the Gateway unit-test loopback adaptation remain at their original
layers. Each dropped address still invokes the binary-owned observer exactly once.
Bulk filtering keeps input order and duplicates, and does not stop early after
finding an allowed address. OIDC URL parsing, hostname passthrough, policy loading
and fixed dependency-error mapping remain unchanged.

The explicit address/observer matrix passed against the old batch implementation.
It now also checks scalar decisions and drop reasons. Gateway tests exercise the
adapter's mapped-address and loopback behavior, and the existing four OIDC
admission regressions exercise the actual call site. Baseline and final common
runs passed nine network-free tests; final Gateway adapter and OIDC runs passed
three and four tests respectively. Formatting, workspace check/Clippy and seven
repository guards passed with existing unrelated warnings.

This removes the singleton address-vector allocation on literal OIDC admission.
URL parsing and metric machinery may still allocate. No additional allocation
was inferred for the old consuming filter collect, which may reuse its input
storage. No throughput benchmark, live DNS/provider test or full suite ran.

## Reject oversized OIDC header values before scanning their bytes

The request-filter session and bearer paths reach `apply_upstream_headers`, then
`try_set_header_with_limits`. Its per-value byte-length rejection now precedes
the CR/LF/NUL safety scan. Both checks are pure and silently return without
writing a header or changing the budget. Admitted values still pass the same
safety scan, total-budget check and canonical sensitive-header writer; only a
successful write consumes budget. No total-budget arithmetic was moved.

Independent plan review passed. The existing three upstream-header tests passed
before and after the change, covering accepted sensitive headers, oversized
values and unsafe values. The failed-write budget regression also passed after
the change. Formatting, workspace all-target check/Clippy and seven project
guards passed with existing unrelated Clippy warnings. Final independent review
passed.

The benefit is limited to skipping the linear safety scan for a value already
over its configured limit. No allocation reduction or general throughput gain
is claimed; no network test or full suite ran.

## Avoid unused self-weak ownership for unregistered OIDC refresh flights

The request-filter refresh path reaches `RefreshFlightRegistry::get_or_create`.
When its bounded map is full, each caller receives an independent unregistered
flight. This saturation branch now constructs the flight with `Arc::new` and an
empty `self_weak`, rather than cyclic initialization and a cloned self-reference.
The only reader of `self_weak` is registered-entry identity cleanup; ephemeral
Drop returns immediately because `registered_key` is absent. The caller's strong
Arc still owns the Mutex throughout waiting, refresh and cancellation.

Registered-flight cyclic construction, weak-map ownership, capacity admission,
lock release and allocation-identity cleanup remain unchanged. Independent plan
review passed. All six existing registry tests passed both before and after the
change: same-key reuse, different-key independence, saturation without map growth,
last-owner cleanup without deadlock, and both capacity validation boundaries.
Formatting, workspace all-target check/Clippy and seven guards passed, with
existing unrelated Clippy warnings. Final independent review passed.

This removes unused self-weak ownership and cyclic setup only in the saturated
branch. It does not remove the flight allocation or claim a measured throughput
improvement. No network test or full suite ran.

## Defer borrowed OIDC session-cookie projection

Rechecked request-filter refresh publication and its six-field persistence
projection. Nonblank access-token copying was already eliminated; session
reference, optional ID/refresh tokens and whitespace-only access tokens still
use temporary owned copies before serialization. Borrowing them requires a
choice between a second serialization view and a broader shared-schema change.
The new oidc-07 task records that decision and exact serialization/cache-order
acceptance separately from oidc-02 Base64 output construction.

Independent plan review passed. This documentation-only batch passed formatting,
workspace all-target check/Clippy, agent-doc validation and diff checks, with
existing unrelated Clippy warnings. No runtime tests or benchmarks ran.
Final independent review passed.

## Reject malformed CSRF signature lengths before HMAC

The request plugin decodes matching header/cookie tokens and calls verification.
After the unchanged lifetime checks, verification now rejects signatures whose
byte length is not 64 before HMAC construction and digest hex encoding. This
uses only public supplied length. Every 64-byte input retains the original HMAC
and full XOR fold, including uppercase/non-hex/mismatched inputs. Issuance,
clock handling, nonce bytes, key material, response and log behavior are unchanged.

Independent plan review passed. All 20 CSRF tests passed before and after the
change, including 63/65-byte rejection, exact-length invalid signatures, known
valid signatures and lifetime boundaries. Formatting, workspace all-target
check/Clippy and seven guards passed with existing unrelated warnings. No full
suite or throughput benchmark ran. The gain is avoiding cryptographic work only
for malformed lengths; it is not an allocation reduction claim.

Knowledge follow-up outside this batch: the typical CSRF example in
`skills/04-review/security/mandatory-design/iron-rules.md` assumes the nonce is
unknown to the caller. Here `CsrfToken::decode` obtains `random` from the supplied
token and verification uses those bytes. That rationale did not justify this optimization or establish exploitability.
The subsequent knowledge-correction batch below resolves this follow-up.
The owning signature-input knowledge now states the actual public-length proof.
Final independent code and knowledge review passed.

## Correct CSRF timing-review knowledge

Revalidated the header/cookie agreement, client token decoding and signature
verification chain. Corrected the security timing table and its duplicated
historical P0-04 rationale: supplied nonce entropy does not make the nonce an
unknown secret. Noise and query restrictions require evidence rather than
blanket non-exploitability claims. The linked auth-timing page now qualifies its
old whole-surface table as historical, removes the same nonce premise and permits
re-evaluation of existing paths with contrary evidence.

The current source checks lifetime, rejects public non-64-byte lengths and then
uses HMAC plus a full XOR fold. This correction claims neither machine-code
constant-time verification nor an exploit. Unrelated historical cases and runtime
code are unchanged. Independent plan review passed; no runtime tests or timing
benchmarks ran in this documentation-only batch.

Formatting, workspace all-target check/Clippy, agent-doc validation and diff
checks passed with existing unrelated warnings. Final independent code-evidence
and knowledge review passed.

## Reuse the CORS request-entry Origin lookup

The request filter now passes its initial borrowed textual Origin directly to
`get_validated_origin`. Previously the presence check and that helper each
looked up and text-validated the same immutable header with no intervening
mutation or await. The helper retains direct config allowance and ACAO selection;
accepted echoes own their output and the standard star borrows static storage
before later mutable session operations. Missing/non-text inputs still skip
without logging. Preflight and timing-origin reads remain unchanged.

Independent plan review passed. Baseline and final Gateway CORS-filtered runs
passed 45 tests each. The existing denied-GET test now requires one header-map
read; the adapted allocation matrix retains absent/non-text/denied/star/echo
cases. Formatting, workspace all-target check/Clippy and seven guards passed
with existing unrelated warnings. The owning knowledge page distinguishes its
historical session-taking signature from the current borrowed input.

This removes one duplicate lookup and text validation for present textual
Origins, not a buffer allocation. No full suite or throughput benchmark ran.
Final independent code and knowledge review passed.

## Extend empty compression-buffer investigation to Brotli

Traced the registered downstream module into the pinned fork's Brotli compressor.
Like gzip, it reserves output storage and transfers the Vec after every encode,
including empty output; the pinned Bytes conversion retains spare capacity in
a Shared owner. The existing compression-02 task now covers this same mechanism
for both codecs with separate frequency/capacity/CPU measurements and unchanged
codec-specific terminal operations. Decompressors remain excluded.

This source audit makes no empty-output frequency or net performance claim.
No dependency checkout, runtime source or lockfile changed. Independent plan
review passed; no runtime tests or benchmarks ran in this documentation batch.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
checks passed with existing unrelated warnings. Final independent review passed.

## Inspect downstream request-body module dispatch

The pinned compression and gRPC-Web implementations inherit the async no-op
request-body hook. When the relay reaches body-hook dispatch, HttpModuleCtx
awaits every registered module before the application action; earlier trailer
errors or termination can bypass this stage. The locked async-trait expansion
uses boxed futures. A stage-participation or synchronous-result contract is
deferred in context-02 rather than hardcoding a two-module bypass.

This source observation is distinct from outer context storage and per-connection
Stream plugin dispatch. It does not establish optimized allocation counts or
CPU gains. Independent plan review passed. No runtime code, dependency checkout
or revision changed; no runtime tests or benchmarks ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
checks passed with existing unrelated warnings. Final independent review passed.

## Defer prepared cache-hit Content-Length

Rechecked response-cache lookup, pending capture and publication: each hit
formats immutable published entry.used after full header decode. Preparing a
value may remove repeated formatting, but adds a persistent derived value whose
capacity, admission and reader-retained lifetime require explicit accounting.
The new cache-04 task records the alternatives and consistency tests; cache-02
Age measurements are not extrapolated to this construction.

Independent plan review passed. Runtime code and cache charges are unchanged.
No runtime tests or benchmarks ran for this documentation-only investigation.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
checks passed with existing unrelated warnings. Final independent review passed.

## Validate cache decimal fields during checked accumulation

The cache policy decimal helper now checks each ASCII digit inside its checked
fold instead of performing a complete digit scan first. Empty input remains
invalid, leading zeros remain valid, and every non-digit or u64 overflow returns
None. Neither caller distinguishes invalid syntax from overflow: strict max-age
admission and Age freshness parsing retain the same result. The helper has no
observer or mutation, so an overflow may reject before reading an invalid suffix.

Independent plan review passed. A new byte-grammar/overflow regression passed
against the old implementation before the production edit: all 256 single-byte
values, embedded bytes, leading zeros, u64 maximum and overflow suffixes. Final
verification passed five policy tests and 21 storage tests, formatting, workspace
all-target check/Clippy and seven guards with existing unrelated warnings.

This removes a separate validation traversal, not allocations. The old digit
prepass may optimize differently from the combined loop; no universal CPU or
throughput gain is claimed. No full suite or performance benchmark ran.
Final independent review passed.

## Read optional cache Age through one header lookup

Freshness construction now reads Age with one get_all iterator rather than
contains_key followed by singleton lookup. Missing remains zero; any second
value rejects before strict decimal parsing. The Date/lifetime validation order,
apparent age, response delay, overflow and remaining-lifetime calculations are
unchanged. No stored state or header mutation is introduced.

Independent plan review passed. A real HeaderMap matrix passed against the old
implementation before the edit, covering absent, zero, leading zeros, maximum,
empty, whitespace, signs, overflow, non-text and repeated values (including empty
first or last). Final verification passed six policy and 21 storage tests,
formatting, workspace all-target check/Clippy and seven guards with existing
unrelated warnings. This removes one lookup when Age is present; no allocation
or throughput improvement is claimed. No full suite or benchmark ran.
Final independent review passed.


## Borrow the affinity runtime cell from its captured generation

Ordinary Service affinity lookup now borrows runtime_cell from the available
projection held by its local ServiceSelectorGeneration. The synchronous cache-hit
and cache-build paths no longer clone that cell Arc. The generation still pins
the cell through active/SP snapshot reads, the build lock, membership checks and
publication revalidation. Lock release order and returned affinity-map ownership
remain unchanged; typed-scope map construction is untouched.

Independent plan review passed. The existing pre-change LB and coordinator
baselines passed 127 and 41 tests respectively. This removes one Arc reference
count increment/decrement after an available Service lookup, not heap allocation
or measured throughput cost. Final verification passed the same 168 tests,
formatting, workspace all-target check/Clippy and seven guards. Existing Clippy
warnings are outside the changed code. Tests cover stale-builder rejection,
membership repair and section isolation. No full suite or benchmark ran.
Final independent review passed.


## Prepare ExternalName DNS mode in the Service projection

AvailableServiceProjection now stores the canonical parsed DNS resolution mode,
computed at its single publication constructor. Native Service dispatch reads
that Copy enum instead of looking up the annotation and trimming/matching its
text on every ExternalName attempt. The existing Service semantic digest already
includes the mode annotation, so changes and removal rebuild the projection.
Retained captures keep the old mode. Type/hostname validation, readiness-based
controller fallback, DNS cache identity and live resolver selection are unchanged.

Independent plan review passed. A new mode-only update regression passed against
the old production implementation, checking current and all retained captures
through absent, gateway, per-request, whitespace, controller, invalid and removed
values. The existing native baseline passed 15 tests with the known TLS fixture
in verification-03 explicitly excluded; that fixture was not modified. This is
cold preparation with one internal enum per projection, not a measured throughput
or allocation improvement. Final verification passed 77 tests: 16 native dispatch
(with that same explicit TLS-fixture exclusion), 41 coordinator and 20 DNS-cache
tests. Formatting, workspace all-target check/Clippy and seven guards passed
with existing unrelated warnings. No full suite, live DNS or benchmark ran.
Final independent review passed.


## Compression response classification reuse audit

Traced the downstream module into the pinned fork's response-header callback.
Enabled compression with absent Content-Encoding and nonempty preferences can
classify the same response twice for Vary and action selection. Between those
checks only Vary changes. Length parsing can repeat; MIME scanning repeats only
when the length gate permits it. Compression-04 records callback-local reuse for
user decision, preserving short circuits and exact negotiation/header behavior.
No external checkout, dependency revision or runtime code changed.

Independent plan review passed. Formatting, workspace all-target check/Clippy,
agent-doc validation and diff checks passed with existing unrelated warnings.
This documentation-only batch ran no runtime tests or performance measurements.
Final independent review passed.


## Use the stored trailer field count before byte validation

ExtProc checked_map_size now checks HeaderMap::len against the field cap before
walking bytes. The pinned http implementation counts every value, including
repeated values, through entries.len plus extra_values.len. This replaces the
per-value checked count and comparison and rejects field overflow without scanning
content. The byte loop retains both checked additions and the inclusive cap,
including the name bytes for every repeated value. All failures remain the same
pure Err(()); projection, atomic mutations and failure-policy handling are unchanged.

Independent plan review passed. All 143 existing ExtProc baseline tests passed.
A new regression passed against the old production implementation, covering
empty maps, 256/257 repeated values, removal/reappend, overwrite contraction and
exact/over-byte-cap repeated values. No allocation or measured throughput benefit
is claimed. Final verification passed 144 ExtProc and 11 upstream-trailer tests
(155 total), formatting, workspace all-target check/Clippy and seven guards.
Existing warnings remain outside changed code. No full suite, interoperability
test or benchmark ran. Final independent review passed.


## Typed response processor forwarding audit

The leased response driver dispatches on_event, and AiAttemptResponseProcessor
forwards that event to its child processors. UsageExtractionProcessor,
GuardrailResponseProcessor, SemanticSseSource, AnthropicResponseProcessor and
MessagesProcessor inherit the default on_event, which awaits on_chunk. Locked
async-trait 0.1.92 expands both async methods through Box::pin. This is source
structure, not measured allocation counts, future sizes or gateway-level cost.

ExtProc and gRPC processors override on_event; their compatibility on_chunk
methods call it, so that reverse wrapper is not automatically extra work on the
normal typed driver path. Wasm overrides both methods and calls synchronous
observe directly from each. The outer Pingora callback and installed processor
forwarding are distinct layers and must be measured separately.

The canonical cpu-memory async-response-body-hook-profile-before-gate decision
continues to govern the proposed public no-work predicate. It is not an absolute
ban on local helper or direct-future forwarding alternatives. No representative
profiling was added here, so no runtime refactor or duplicate deferred task was
selected. Any later change must establish value and preserve typed terminal
mapping, first-poll behavior, cancellation leases, backpressure, ordered child
execution and early terminal arbitration.

Independent plan review passed. This source-audit batch changes only this record;
no runtime tests or allocation/throughput measurements were run. Formatting,
workspace all-target check/Clippy, agent-doc validation and diff checks passed
with existing unrelated warnings. Final independent review passed.

## Borrow WAF request identity during transaction construction

WafPlugin::run_request now uses get_request_id_ref().unwrap_or_default()
for UNIQUE_ID. The current production PingoraSessionAdapter's owned getter
performs exactly that lookup followed by to_owned; the independent trait default
is not a universal equivalence guarantee. This removes the temporary String for
a present identity. RequestState::build still validates and copies the same bytes
into its bounded arena before any subsequent mutable session access or await.
Canonical W3C identity, missing identity as empty bytes, limits, phase order,
transaction ownership and access-log behavior are unchanged. No measured
throughput or whole-request allocation claim is made.

The mounted WAF regression checks actual UNIQUE_ID rule outcomes for a present
canonical identity and missing identity. Existing hot-reload and ExtensionRef
mocks use the borrowed getter with the same value; the real-adapter identity test
covers absence, initialization, wire-header suppression and later header rewrites.

Fresh verification passed 90 Gateway tests selected by waf and the real-adapter
borrowed-identity test. Formatting, workspace all-target check and Clippy, agent
documentation, SSA-force, metrics inventory, Pingora-source policy, Gateway API
transform contract, unit-test layout and gateway no-tracing guards passed.
Clippy retains unrelated existing warnings. This batch covers identity ingestion,
not the remaining WAF engine operators or complete callback coverage.
