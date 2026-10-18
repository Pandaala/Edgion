---
name: response-condition-live-input-borrowing
description: Review per-chunk response-body condition extraction and live input borrowing.
knowledge_type: regression-guards
---

# Response-body conditions borrow live inputs for each evaluation

## Conclusion and scope

`ConditionalUpstreamResponseBodyFilter` evaluates its compiled `skip` and `run`
conditions for every response chunk. The shared condition evaluator borrows live
`Header`, `Cookie`, `Path`, `RequestPath`, `Method`, `RemoteIp`, `DirectPeerIp`
and `Ctx` values for one predicate evaluation. It also borrows the captured
canonical `TraceId`, frozen normalized `Host` and mTLS certificate candidates.
Query conditions borrow the decoded frozen entry-query cache for the same short
evaluation; they do not read the live rewritten URI. `KeyExist`, `KeyMatch`,
`SecretMatch`, and keyed `Probability` use that value accessor. KeyExist uses a
direct presence check for RemoteIpAndPath, whose existence depends only on a
nonempty RemoteIp. It does not cache
a predicate result or a request snapshot across chunks. The response-body
filter still emits no per-chunk `PluginLog`.

The fields `PluginConditions.skip` and `PluginConditions.run` retain their
skip-first evaluation order. `KeyGet` remains the canonical source definition;
`condition_key_value` changes ownership for selected reads, not the predicate
model. `DirectPeerIp` still means the physical peer and `RemoteIp` still means
the resolved identity.

## Rationale and evidence

The response-body wrapper calls `CompiledPluginConditions::evaluate` on each
chunk. The previous condition leaves called `PluginSession::key_get`, whose
production adapter constructed an owned `String` for each selected value.
`PluginSessionHeaderExt::header_str` already supplies the same first UTF-8
header value as the owned `header_value` method. `cookie_value_ref` is the
canonical parser underlying the owned cookie accessor; it can return a slice
of the live request header. The path, method, and IP methods already return
borrowed strings. Reading them within one synchronous predicate evaluation
removes the corresponding owned extraction without extending any value's
lifetime. An invalid UTF-8 header still reads as absent.

A historical focused test with a real Pingora session and production session adapter
measured each present source after the query cache was primed. The thread-local
test allocator counted allocation calls and requested bytes for one owned
`key_get` call, plus one JWT or mTLS candidate extraction. Results for this
fixed fixture are evidence of work done by each path, not portable byte or
throughput targets:

| Source | Allocation calls | Requested bytes |
|---|---:|---:|
| `TraceId` | 1 | 32 |
| `RemoteIp` | 1 | 12 |
| `DirectPeerIp` | 1 | 9 |
| `Host` | 1 | 11 |
| `Header` | 1 | 7 |
| `Cookie` | 1 | 3 |
| `Query` (parsed cache hit) | 1 | 3 |
| `Path` | 1 | 6 |
| `RequestPath` | 1 | 6 |
| `Method` | 1 | 3 |
| `Ctx` | 1 | 5 |
| `RemoteIpAndPath` | 2 | 36 |
| `JwtClaimMatch` with two role candidates | 9 | 902 |
| typed URI `MtlsMatch` with two stored SANs | 7 | 139 |

The selected borrowed condition sources bypass their corresponding owned
`key_get` allocation; Query, Ctx, TraceId and Host were added in subsequent bounded batches. JWT extraction now borrows context JSON, parses it with
`serde_json`, and creates candidate strings. The historical table includes the
source copy that the borrowed context boundary removes. The historical mTLS extraction cloned the
certificate record and created candidate strings; ARCH-007 replaced that path
with a borrowed certificate getter and lazy borrowed candidates. The test asserts source
values, not these remaining allocation counts, so future improvements are
free to reduce them.

The other sources have different costs or lifetimes:

| Source | Current per-evaluation work | Ownership and source boundary |
|---|---|---|
| `TraceId`, `Host` | Borrow captured request identity | The standard W3C ID survives default-header suppression; Host uses the frozen normalized entry value, not the live header. |
| `Query` | Borrows one decoded value from the lazily initialized entry-query cache | `get_query_param_ref` ties the value to the session read; the owned getter still copies for consumers requiring ownership. |
| `Ctx` | Borrows the current context value | The short session borrow ends before later plugins set or remove variables. |
| `RemoteIpAndPath` | KeyExist checks RemoteIp presence; value consumers still format the composite | A nonempty RemoteIp produces a value even with an empty Path; matching and hashing still need the composite contents. |
| `JwtClaimMatch` | Borrows context JSON, parses it, and creates scalar candidate strings | JWT/OIDC claim context may be changed by plugins; a cross-chunk parsed snapshot would need invalidation. |
| `MtlsMatch` | Borrows `ClientCertInfo` and lazily selects string slices | The getter retains the existing certificate exposure gate; each synchronous evaluation owns only its short borrow. |
| `TimeRange`, `Probability` | Reads current time or draws probability per evaluation; keyed probability reads its current key | Freezing the decision would change timing or sampling semantics. |
| `DirectPeerIpMatch`, `RemoteIpMatch`, `RateLimitExceeded`, `HmacAuth` | Reads current session state or verifies current headers | These predicates do not use the changed `KeyGet` extraction path unless a nested verifier does so independently. |

The selected input values may themselves change between chunks. The borrowing
scope deliberately ends before the next filter runs, so later plugin mutations
remain visible. Query inputs now use an explicit borrowed session boundary. First access may
still parse and allocate the frozen cache; repeated selected value reads borrow it. This page makes no claim that all response-body conditions
are allocation-free.

## Alternatives and limits

Caching a complete condition result at response-header time would freeze time,
probability, mutable headers, context values, and resolved remote IP. Caching
only apparently stable values would add request state and invalidation rules.
The current correction instead uses existing borrowed accessors where their
semantics match the owned read. JWT JSON parsing remains a known cost, not an accepted permanent design
requirement. Certificate cloning is now explicit only at HeaderCertAuth's
legitimate owned boundary before mutable header operations; DSL retains its
selected CN/fingerprint copies for VM ownership.

The independent async response-body hook decision concerns outer dispatch and
future allocation. It does not decide the cost of condition extraction inside
installed filters; see
[`async-response-body-hook-profile-before-gate.md`](../intentional-design/async-response-body-hook-profile-before-gate.md).

## Verification and re-evaluation

Gateway evaluator tests cover re-evaluation across different header states,
invalid UTF-8 header absence, distinct direct/resolved IP inputs, canonical
cookie parsing, and the existing condition semantics. The borrowed
header-and-method evaluation and cookie extraction tests forbid `key_get`
calls and measure zero allocation calls using the gateway's thread-local test
allocator. This is focused unit evidence, not a production throughput estimate.

Re-evaluate if a condition source gains a borrowed session accessor, the
header/path/IP semantics change, response-body plugins acquire a new mutable
context write, or profiling shows that the remaining JWT, query,
or cookie extraction dominates response-chunk cost. Preserve the response-body
streaming and no-per-chunk-log invariants in any follow-up.

## Resolution references

- `tasks/todo/reduce-response-body-condition-extraction-cost.md` (2026-09-07):
  response-body condition extraction cost; selected live reads corrected while
  other sources retain their per-evaluation semantics.

## ARCH-007 certificate borrowing boundary

`PluginSession::client_cert_info` returns a borrowed record with an explicitly
named self lifetime. Mockall 0.14.0 supports this nested return shape; elided
nested return lifetimes fail macro generation. Mock expectations use bounded,
fixed OnceLock fixtures rather than retaining production copies or leaking test
fixtures. Existing EdgionTls exposure gating and Gateway frontendValidation
certificate extraction remain unchanged, as do redacted Debug and sensitive
HeaderCertAuth header writes.

`extract_mtls_values` produces a lazy iterator without collecting candidates.
Untyped SANs preserve raw strings; typed URI/email values borrow prefix-stripped
slices; DNS uses the existing canonical display-prefix classifier. The shared
any-match evaluator short-circuits and negates once, and still suppresses an
uncompiled-regex diagnostic when no candidate exists. No result cache or new
request state is introduced.

Fresh real-adapter allocation tests compared an explicit whole-record clone with
compiled CN and first matching URI SAN conditions. Setup and compilation were
outside the thread-local allocator measurement. Both conditions returned Run:

| Unrelated SAN count | Whole-record clone calls / requested bytes | Compiled CN calls / bytes | Compiled first URI SAN calls / bytes |
|---|---|---|---|
| 0 | 5 / 72 | 0 / 0 | 0 / 0 |
| 4096 | 4101 / 203762 | 0 / 0 | 0 / 0 |

The clone baseline measures the historical certificate getter ownership cost,
not the complete former predicate. The adapter borrow aliases the RequestInfo
record. These fixed-fixture results show removal of unrelated certificate copies;
they do not establish production throughput benefit. The updated source inventory
separately collects one borrowed URI candidate into a Vec for assertions and
measured 1 allocation / 64 requested bytes. That allocation is test collection,
not compiled predicate evaluation. The original fixture table above remains
historical evidence and is not a current mTLS cost estimate.

Final-source focused tests passed: 58 shared-condition tests, 14 HeaderCertAuth
tests, 15 DSL request-metadata tests, and one test each for the allocation
comparison, source inventory, and absent certificate record. They retain existing
cases and cover borrowed pointers, canonical SAN types, regex/negation and
missing-candidate diagnostics. This is targeted unit evidence; no live TLS
integration or production throughput measurement is claimed.

- `ARCH-007`: borrowed mTLS condition candidates, preserving live evaluation and
  explicit consumer ownership.


## Borrowed decoded query boundary

`PluginSession::get_query_param_ref` has an explicit self lifetime independent of
its name argument. The production adapter reads the same canonical cached_query
owner as before; get_query_param explicitly copies that borrow for owned callers.
The shared condition accessor uses the borrowed method for Query, so KeyExist,
KeyMatch, SecretMatch and keyed Probability retain their source and predicate
semantics without a temporary selected-value String. No cache is initialized
before the original read site, and no predicate result survives an evaluation.

The real-adapter regression rewrites the live URI before and after first query
access, checks plus/percent decoding, missing versus empty values, and verifies
pointer identity with the frozen cache. After cache initialization it compares
one owned key_get allocation with zero allocations for repeated compiled query
matching. This is a fixed-fixture allocation result, not a cold-parse or throughput
claim. The unresolved entry-capture lifecycle boundary described in URI
normalization knowledge remains unchanged.


## Borrowed live context boundary

`get_ctx_var_ref` borrows the existing request context map for one session read;
the owned getter explicitly copies only for consumers requiring ownership.
Ctx conditions use the borrow, and JWT extraction parses the borrowed jwt_claims
or oidc_claims text. Missing-only fallback, dot-key resolution and owned parsed
JSON/candidates are unchanged. No mutation invalidation cache or new logging is
introduced; the claim-key redaction policy remains independently required.

Real-adapter tests replace, empty, remove and restore a Ctx value between compiled
predicate evaluations. Matching reads allocate zero times, compared with one
owned key_get copy for a present nonempty value. JWT source tests cover malformed,
empty and claim-missing JWT preventing OIDC fallback, valid JWT precedence and
removal revealing OIDC. Only the source text copy is removed: JWT parsing and
candidate allocations still leave the recorded per-chunk allocation tension open.


## Borrowed captured identity boundary

TraceId conditions use get_request_id_ref, which borrows standard_trace_id from
the request-owned W3C context. Host conditions use get_request_hostname, which
borrows the same frozen normalized hostname as owned KeyGet and treats empty as
missing. Existing owned callers still copy explicitly; no header reparsing or
additional request state is introduced.

The former statement that RequestId rules mutate the standard trace ID was stale:
initialize is the current W3C state's sole assignment, and default-header patches
or custom correlation rules retain it. This correction does not permit freezing
other mutable condition sources or skipping per-evaluation source reads. Header
rewrites also do not replace the captured normalized Host identity.

A real-adapter test starts before initialization/capture, checks absence, then
establishes both identities. After default-ID suppression and live Host/traceparent
rewrites, borrowed pointers still alias the canonical owners. Each selected owned
key_get allocates once; repeated compiled identity matches allocate zero times.
Initialization, compilation and header rewriting are outside that measurement.
This is source-copy removal evidence, not a throughput or whole-condition claim.


## Composite-key existence

KeyExist(RemoteIpAndPath) checks the current remote_ip borrow directly. The owned
adapter returns None exactly when that string is empty; otherwise it formats
ip:path even if the path is empty. Skipping value construction for presence does
not skip a meaningful path-side effect: get_path reads the current normalized
path from the request context. Negation still applies once after presence.
KeyMatch, SecretMatch and keyed Probability retain the unchanged value accessor;
this does not change composite encoding, matching or deterministic hash buckets.

The real-adapter regression reuses compiled normal/negated conditions while
changing one context through absent, IPv4, IPv6 and absent-again inputs, each with
empty, ordinary and long paths. Results match owned KeyGet presence and predicate
allocation counts remain zero. Composite content consumers and JWT parsing still
allocate; no whole-condition-system compliance or throughput claim follows.
