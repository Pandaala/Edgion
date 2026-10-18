---
name: response-condition-live-input-borrowing
description: Review per-chunk response-body condition extraction and live input borrowing.
knowledge_type: regression-guards
---

# Response-body conditions borrow live inputs for each evaluation

## Conclusion and scope

`ConditionalUpstreamResponseBodyFilter` evaluates its compiled `skip` and `run`
conditions for every response chunk. The shared condition evaluator borrows live
`Header`, `Cookie`, `Path`, `RequestPath`, `Method`, `RemoteIp`, and `DirectPeerIp` values
and mTLS certificate candidates for the duration of each predicate evaluation.
Query conditions borrow the decoded frozen entry-query cache for the same short
evaluation; they do not read the live rewritten URI. `KeyExist`, `KeyMatch`,
`SecretMatch`, and keyed `Probability` use that same accessor. It does not cache
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

The first seven borrowed condition sources now bypass their listed owned
`key_get` allocation. JWT extraction clones context JSON, parses it with
`serde_json`, and creates candidate strings. The historical mTLS extraction cloned the
certificate record and created candidate strings; ARCH-007 replaced that path
with a borrowed certificate getter and lazy borrowed candidates. The test asserts source
values, not these remaining allocation counts, so future improvements are
free to reduce them.

The other sources have different costs or lifetimes:

| Source | Current per-evaluation work | Ownership and source boundary |
|---|---|---|
| `TraceId`, `Host` | Copies a string | No equivalent borrowed condition accessor; request ID rules can mutate the trace ID. |
| `Query` | Borrows one decoded value from the lazily initialized entry-query cache | `get_query_param_ref` ties the value to the session read; the owned getter still copies for consumers requiring ownership. |
| `Ctx` | Clones a context value | Plugins may set or remove context variables. |
| `RemoteIpAndPath` | Formats a combined string | The resolved IP can change; the composite needs an owned representation today. |
| `JwtClaimMatch` | Clones context JSON, parses it, and creates scalar candidate strings | JWT/OIDC claim context may be changed by plugins; a cross-chunk parsed snapshot would need invalidation. |
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
