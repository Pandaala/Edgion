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
for the duration of each predicate evaluation. `KeyExist`, `KeyMatch`,
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

A focused test with a real Pingora session and production session adapter
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
`serde_json`, and creates candidate strings. mTLS extraction clones the
certificate record and creates candidate strings. The test asserts source
values, not these remaining allocation counts, so future improvements are
free to reduce them.

The remaining sources have different costs or lifetimes:

| Source | Current per-evaluation work | Why it remains live and owned |
|---|---|---|
| `TraceId`, `Host` | Copies a string | No equivalent borrowed condition accessor; request ID rules can mutate the trace ID. |
| `Query` | Clones one value from the request's already parsed query cache | The entry query is frozen, but the current `PluginSession` getter returns an owned value. |
| `Ctx` | Clones a context value | Plugins may set or remove context variables. |
| `RemoteIpAndPath` | Formats a combined string | The resolved IP can change; the composite needs an owned representation today. |
| `JwtClaimMatch` | Clones context JSON, parses it, and creates scalar candidate strings | JWT/OIDC claim context may be changed by plugins; a cross-chunk parsed snapshot would need invalidation. |
| `MtlsMatch` | Clones `ClientCertInfo`, then selects or copies candidate strings | The certificate is connection-derived, but `PluginSession::client_cert_info` exposes only an owned return. |
| `TimeRange`, `Probability` | Reads current time or draws probability per evaluation; keyed probability reads its current key | Freezing the decision would change timing or sampling semantics. |
| `DirectPeerIpMatch`, `RemoteIpMatch`, `RateLimitExceeded`, `HmacAuth` | Reads current session state or verifies current headers | These predicates do not use the changed `KeyGet` extraction path unless a nested verifier does so independently. |

The selected input values may themselves change between chunks. The borrowing
scope deliberately ends before the next filter runs, so later plugin mutations
remain visible. Query and certificate inputs may support further optimization,
but that requires a separately justified borrowed session boundary and a
measured benefit. This page makes no claim that all response-body conditions
are allocation-free.

## Alternatives and limits

Caching a complete condition result at response-header time would freeze time,
probability, mutable headers, context values, and resolved remote IP. Caching
only apparently stable values would add request state and invalidation rules.
The current correction instead uses existing borrowed accessors where their
semantics match the owned read. JWT JSON parsing and certificate cloning are
known remaining costs, not accepted as a permanent design requirement.

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
context write, or profiling shows that the remaining JWT, certificate, query,
or cookie extraction dominates response-chunk cost. Preserve the response-body
streaming and no-per-chunk-log invariants in any follow-up.

## Resolution references

- `tasks/todo/reduce-response-body-condition-extraction-cost.md` (2026-09-07):
  response-body condition extraction cost; selected live reads corrected while
  other sources retain their per-evaluation semantics.
