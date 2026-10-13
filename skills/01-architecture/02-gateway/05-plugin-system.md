---
name: gateway-plugin-system
description: Plugin system architecture — 4-phase execution, PluginRuntime pre-build, conditional execution, 29 HTTP plugins, Stream plugins.
---

# Plugin system

> The plugin system is Edgion Gateway's request-handling extension mechanism.
> Four execution phases cover the full lifecycle of a request; the Gateway builds and shares plugin runtimes during configuration publication to avoid per-request construction.

## Four-phase execution model

| Phase | trait | Timing | Async/sync | Method signature | Return |
|-------|-------|--------|------------|------------------|--------|
| RequestFilter | `RequestFilter` | Before forwarding to upstream | async | `run_request(&self, session, log) -> PluginRunningResult` | Can terminate the request or return a custom response |
| UpstreamResponseFilter | `UpstreamResponseFilter` | After upstream response headers received | sync | `run_upstream_response_filter(&self, session, log) -> PluginRunningResult` | Can modify response headers, terminate |
| UpstreamResponse | `UpstreamResponse` | Final response headers, before downstream header commit and body streaming | async | `run_upstream_response(&self, session, log) -> PluginRunningResult` | Await header decisions, modify headers, or install bounded streaming companions |
| UpstreamResponseBodyFilter | `UpstreamResponseBodyFilter` | Data callbacks and terminal completion without trailers | sync | `run_upstream_response_body_filter(&self, body, end_of_stream, session) -> Option<Duration>` | Returns Duration for bandwidth limiting; **note: the signature deliberately omits `&mut PluginLog`** — see the "no PluginLog in body phase" constraint below |

The response stages have distinct boundaries. The Gateway's
`pg_upstream_response_filter` skips interim 1xx, arbitrates status retries,
normalizes the selected response, and runs synchronous `UpstreamResponseFilter`
plugins. `pg_response_filter` then runs asynchronous `UpstreamResponse` header
decisions before committing headers or forwarding the response body. Interim
1xx (such as 100/103) do not run either chain; 101 is a terminal upgrade response
and is not treated as interim.

Neither header stage exposes the complete main response body. The upstream body
remains streaming: body filters and bounded streaming processors run on its later
body path, while trailer companions run through the separate trailer dispatcher.
Do not implement full-response aggregation in `UpstreamResponse` or treat it as
an end-of-response hook. See the
[`UpstreamResponse` trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response.rs)
and the [Gateway response driver](../../../edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs).

Execution flow notes:

- Native WAF request-body admission uses `spec.waf.requestBody.inspection`.
  Default `full` inspects complete documents under the effective policy window.
  Explicit `prefix` reads up to `prefixSize` (64 KiB by default, at most 1 MiB)
  into in-memory `BodyStorage` before upstream dispatch through the
  canonical `BodyStorage` / Pingora registered-buffer seam, then drains the
  prefix and resumes live forwarding. Prefix sealing is not transport EOS and
  never publishes a partial complete-body cache. Default `full` requires complete inspection before rule publication; a
  WAF-only request may use bounded native N+1 capture when its inspection or
  storage window is below the hard cap. A proven policy cutoff discards
  provisional facts and skips remaining WAF phases; `onLimit` selects continue
  or reject. Other complete consumers retain complete capture. Existing complete caches are reused and their logical
  body is frozen after prefix inspection. See the
  [prefix admission contract](../../04-review/http-body/regression-guards/waf-prefix-before-dispatch.md).
  Optional full-only `requestBody.json` supplies lexical/structure/results limits;
  omission selects bounded complexity defaults. Source-byte limits inherit the
  pinned physical cap and narrow to the effective full inspection window. Both inline and resolved Full
  configurations reject JSON dependencies requiring a complete field-value map.
  After phase-one controls, processor preflight precedes capture at the EGC cap.
  Gateway feeds real capture chunks into one persistent inspection session;
  cached input uses an immutable snapshot reader with pre-read size admission.
  JSON alone opts into a shared contiguous resident slice for a complete,
  single-segment snapshot of at most 128 KiB. Larger, spilled and multi-segment
  snapshots retain bounded reads. The native checker borrows complete plain
  tokens and uses bounded carry storage across chunk or escape boundaries,
  retaining only required metadata, match results and named deferred values.
  Actual EOF, byte-count parity and physical/WAF/JSON limits precede publication.
  JSON-only inspection never materializes the complete source; explicit raw
  visibility or DetectionOnly syntax fallback may require a canonical body read.
  There is no DOM fallback or second body cache. Limits and dispatch plans are
  resolved before request processing.
  See [fused JSON inspection](../../04-review/waf/regression-guards/fused-json-inspection.md).
- Within each phase, plugins run in registration order.
- If any plugin returns `ErrTerminateRequest` or `ErrResponse`, the phase terminates immediately and subsequent plugins are skipped.
- In the UpstreamResponseBodyFilter phase, when multiple plugins return Duration values, the maximum is used (the strictest limit takes effect).
- After each phase completes, plugin logs are recorded into `ctx.stage_logs`, including plugin name, elapsed time, and conditional-skip information.
- **Exception**: the UpstreamResponseBodyFilter phase **does not** write to `ctx.stage_logs` and does not push entries into `ctx.pending_edgion_plugins_logs`. This phase runs on data callbacks and no-trailers terminal completion; per-callback logging would blow up log volume on long streams. See the "no PluginLog in body phase" constraint below.

Unary GrpcJsonTranscoder replacement is a separate bounded owner: its final JSON
bypasses upstream body filters, so incompatible body consumers are rejected using
actual request-pinned runtimes. Header/trailer-only companions remain valid. Sticky
registration/mutation conflicts are checked through replacement consumption, and
one final PluginLog outcome is added at universal logging. See the
[transcoder regression boundary](../../04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md).

Transport admission reads two immutable facts from the effective PluginRuntime.
Resources owns the exhaustive `EdgionPlugin::is_ai_conversion_owner` classification
(AiProxy, AiGuard and Guardrail), shared by resource validation and Gateway
construction. Gateway publishes it only when the request executable is constructed,
including Guardrail's attached request/response companion path. The response-body
factory supplies an explicit unary conflict fact with each successful construction;
BandwidthLimit and response-body Wasm conflict, while request/header Wasm does not.
EntryBundle carries these facts through the existing source-generation cache and
ORs them only into admitted views. Disabled and policy-masked slots contribute
neither fact; conditions remain conservative. A Failed bundle's facts describe
its successful executable members, rather than its failure label or raw config.
Demand release and retained in-flight views preserve existing Arc lifetimes.

These cold facts replace name-based scans only for these two decisions. Actual
selected-backend ExtensionRef targets remain request-pinned, and dynamic ExtProc
effective modes and installed processors retain their context checks. An outer
ExtensionRef body trait is not evidence of an incompatible target. This boundary
adds no request state, lock, allocation, response aggregation or dispatch wrapper.


### OIDC signing policy preparation

`OpenidConnect::try_new` resolves the expected and allowed signing algorithms once
per plugin generation through the OIDC-local `resolve_algorithm_policy` helper.
The immutable owner retains either the prepared policy or the data-free
`AuthFailure::InvalidAlgorithm`; an outside-default expected algorithm without an
explicit allowlist remains a verification-time failure. The internal verifier
decodes the JWT header first, borrows the policy, validates the token algorithm,
and only then accesses JWKS/discovery. Captured old plugin generations retain
their policy across configuration replacement. Dynamic key selection and JWKS
rotation remain provider-owned; constructor prewarm and callback token exchange
retain their separate ordering. See the [signing-policy regression boundary](../../04-review/oidc/regression-guards/prepared-signing-policy.md).

### Request-body read contract ownership

`request_body/contracts.rs` defines `BodyReadError`, `BodyReadResult`,
`RequestBodyChunkConsumer`, and `RequestBodySnapshotReader`; import them from
`crate::request_body`. Storage, state, operations, scanners, plugin adapters and
feature consumers share this canonical owner. Snapshot readers own one immutable
logical version and its backing references across awaits and later mutations;
replay cursors and detached-consumer isolation budgets remain separate.

`PluginSession` consumes these contracts while retaining stage/policy/transport
operations. `RequestBodyInspectionCapture` remains session-owned, and the
request-body observer retains its access-log integration. This boundary does not
make the body subsystem completely independent of plugins. Extend a storage/read
capability in the body-owned contract and its concrete implementations, then
migrate actual consumers; do not introduce a session compatibility export or a
second body cache. See the [ownership decision](../../04-review/http-body/intentional-design/request-body-read-contract-ownership.md).

### Selected-WAF session capability

`PluginSession::waf()` returns a short mutable borrow of
`WafSessionCapability`, defined in `runtime/traits/waf_session.rs`. This
integration contract contains selected transaction transfer, phase claims,
bounded summary writes, inspection stops and native body admission. The real
`PingoraSessionAdapter` implements it directly and returns itself. The sole
request state owner remains `ctx::WafRequestContext`; obtaining the capability
creates no state wrapper or request allocation.

Release the capability borrow after each operation, before another session call
or body-capture await. Transaction installation consumes its offer on success
and rejection: rejection drops the offered transaction, retains any resident
transaction, and the WAF caller returns its existing terminal conflict failure.
Body-limit installation still freezes mutation before checking native/cached
lengths, and admission outlives transaction take or inspection stop. Election,
phase-once execution, pinned generations, BodyStorage and access-log ownership
remain unchanged.

A new WAF operation extends this trait, its adapter implementation and feature
consumers/tests; it does not extend the common session operation list or common
mock. Capability methods have no production defaults. Isolated tests explicitly
configure a feature mock, then attach it through the common accessor. See the
[design decision](../../04-review/plugin-runtime/intentional-design/selected-waf-session-capability.md)
for alternatives, transfer contracts and verification limits.

### AI session capability

`PluginSession::ai()` returns a short mutable borrow of `AiSessionCapability`
in `runtime/traits/ai_session.rs`. Twelve request-plan, guard and canonical
handoff operations extend this feature contract instead of the shared operation
list. `PingoraSessionAdapter` implements it directly and returns itself; the
existing lazy `ai::context::AiHttpState` remains the sole exchange state owner.
The accessor and absent-state reads/take create no state or wrapper.

Release each synchronous borrow before ordinary session operations or awaits.
First-writer context/plan and processor offers are consumed even when rejected.
Canonical publication instead returns the original rejected batch and preserves
any resident batch; take transfers that batch exactly once. Publication still
initializes AI state before its existing semantic-ownership check. Guard identity,
body generation, duplicate/frozen/transcoder conflicts and Guardrail head-hold
admission retain their adapter checks and existing failure contracts.

Generic body capture/mutation, stream and ordinary processor installation,
ExtProc/transcoding, response terminal claims/emission and transport facts stay
on the common boundary. The response driver retains AI -> semantic -> ordinary
ordering, canonical drain checks and cancellation leases. Bounded response streams,
RawWindow and the reviewed unary converter retain their independent owners and
limits. Capability methods are required; tests configure one feature mock and
attach it through the common accessor. No throughput improvement is claimed.
See the [AI boundary decision](../../04-review/plugin-runtime/intentional-design/ai-session-capability.md).

### Streamed request admission

Installed request-body handlers own a monotonic source bit in the optional
request-body plugin state. The common body hook checks independent cumulative
input and final-output accounts against the request-pinned physical body cap,
before handlers and before WAF/observer/mirror/upstream publication respectively.
Checked overflow and cap rejection are fixed 413 outcomes, independent of
processor failure policy. Actual request trailers are fixed 400 outcomes for
handler-selected requests; ordinary requests retain the transport discard policy.
Terminal cleanup cancels incomplete handlers and the mirror without sending a
successful EOS, while preserving completed/initiating processor outcomes and
selected response ownership.

Complete-body capture/mutation and streamed handlers are incompatible in either
order. Session adapter guards reject installation after a cache or pre-read and
reject capture/replay-owner creation after handler selection. This is runtime
protection across plugin scopes, not a claim of whole-chain static validation.
See [the maintained regression contract](../../04-review/http-body/regression-guards/streamed-request-body-admission.md).

### Response body and trailer completion

The typed driver keeps these events distinct:

| Driver event | Streaming processors | Ordinary `UpstreamResponseBodyFilter` plugins | Trailer dispatch |
|--------------|----------------------|-----------------------------------------------|------------------|
| `Data { end_of_stream: false }` | Run on the mutable current chunk/window | Run on the immutable current chunk/window with `end_of_stream = false` | None |
| `Data { end_of_stream: true }` | Run | Run with `end_of_stream = true` | Dispatch `None` to `UpstreamResponseTrailer` companions |
| `TerminalWithoutTrailers` | Run the typed terminal event | Run once with `body = None` and `end_of_stream = true` | Dispatch `None` to companions; this is also the header-only response path |
| `TerminalBeforeTrailers` | Run the typed terminal event | Do not run | Keep the trailer lifecycle open for the actual transport trailer callback |
| Actual trailers | Already flushed at `TerminalBeforeTrailers` | Do not run | Dispatch `Some(&mut HeaderMap)` to `UpstreamResponseTrailer` companions |

A filter must tolerate `body = None` and must not treat a missing chunk as missing
completion. `Option<Duration>` delays forwarding or completion of the current task;
it does not permit sleeping inside the synchronous callback. Trailer-aware
finalization belongs to request-owned `UpstreamResponseTrailer` companions:
`Some(trailers)` is the real trailer map, while `None` confirms normal completion
without trailers. Neither lifecycle supplies a complete upstream body or permits
full-response aggregation.

Sources: [typed body driver](../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs),
[read-only body trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs),
and [trailer companion trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_trailer.rs).

## PluginRuntime

`PluginRuntime` is the core container for plugin execution; it holds plugin lists for the four phases:

```rust
pub struct PluginRuntime {
    request_plugins: Vec<Arc<dyn RequestFilter>>,
    upstream_response_plugins: Vec<Arc<dyn UpstreamResponseFilter>>,
    upstream_response_body_plugins: Vec<Arc<dyn UpstreamResponseBodyFilter>>,
    upstream_response_async_plugins: Vec<Arc<dyn UpstreamResponse>>,
}
```

**Build timing and ownership**: the Gateway builds executable runtimes when publishing
configuration, never per request. `HttpRouteRuleUnit::compile_http_rule` and
`GrpcRouteRuleUnit::compile_grpc_rule` build rule and backend chains once before
match/hostname fan-out. Gateway API filter constructors are scope-specific:
`from_httproute_filters_for_rule()` / `from_grpcroute_filters_for_rule()` and
`from_httproute_filters_for_backend()` / `from_grpcroute_filters_for_backend()`.
There are no unscoped forwarding constructors. `new()` / `Default` construct an
empty builder, including the no-filters case.

For `EdgionPlugins`, the Controller calls schema-layer `validate_plugins()` for
source diagnostics and resolves references without applying execution policy.
Gateway `PluginPublisher` receives source resources plus GatewayClass/EGC and
attachment inputs. `PluginGeneration` owns immutable source and a demand-owned
entry cache. It masks disabled/restricted entries before effective validation,
shares permitted constructors and their response companions, and seals each
class view with the current Block/Bypass/invalid gates. `PluginStore` is the
source inventory adapter; it no longer owns a second executable publication.

**Sharing**: `PluginRuntime` does not implement `Clone`. Entry filters and their
companions use shared Arcs so multiple scopes do not reset state such as rate
limits. HTTP requests capture one bounded class scope and use it for global,
Gateway, route and delayed backend references. Backend ExtensionRefs execute
only on first use of the selected backend; unselected candidates cannot cause
early policy rejection. Response and body stages keep
those selected runtime Arcs; they do not load policy again. Configured early
RequestId runs only after unambiguous Gateway ownership and the scope gate.
Atomic publication does not make separately arriving configuration events a
cross-kind transaction. See [scoped policy publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

Route ExtensionRef construction borrows the actual HTTPRoute/GRPCRoute and
indexed effective field. One canonical validated projection produces optional
complete cold keys, preserving ExtensionRef ordinals and invalid/missing holes.
The four stage filters share immutable target metadata; neither request resolution
nor access-log collection formats route keys. Publication demand and AI/adaptive
consumers apply the same projection before any plugin lookup.

Malformed discriminator/payload unions retain the whole-runtime configuration
gate. An unsupported target, invalid name or invalid effective namespace retains
its original ExtensionRef filter and fixed diagnostic. The first invalid rule
filter is also captured for preflight dispatch through the ordinary request
runner using a borrowed one-element slice. Leading policy/configuration gates
keep precedence; earlier ordinary/CORS filters cannot bypass this rejection.
The runner uses the supplied slice for iteration and log reservation and retains
entry identity restoration, canonical logs and terminal response handling.
Backend filters remain selected-only. Their captured-context cache pins both hits
and misses through all response stages and retries; body resolution remains silent.

Each entry may also carry an optional `alias: String` (`^[a-zA-Z0-9._-]{1,32}$`, unique within its stage array — a duplicate is a validation error in both layers), echoed verbatim into `PluginLog.alias` so multiple entries of the same plugin type can be distinguished in the access log. The alias is also the discriminator in the entry's runtime identity `<resource key>/<alias or #index>` (`plugins::common::entry_scope`), which namespaces state a plugin keeps outside its own instance — `RateLimitRedis` Redis buckets today.

## Conditional execution

Plugins defined in EdgionPlugins are automatically wrapped with a `Conditional*` wrapper to support conditional evaluation:

| Wrapper | Wrapped object |
|---------|----------------|
| `ConditionalRequestFilter` | `RequestFilter` |
| `ConditionalUpstreamResponseFilter` | `UpstreamResponseFilter` |
| `ConditionalUpstreamResponse` | `UpstreamResponse` |
| `ConditionalUpstreamResponseBodyFilter` | `UpstreamResponseBodyFilter` |

Condition model (`PluginConditions`):
- `skip`: evaluate its `ConditionSet` first; a match skips plugin execution.
- `run`: when `skip` did not match, execute only if its `ConditionSet` matches.
- Each set has exactly one top-level `allOf` or `anyOf`. `allOf` contains
  direct conditions. `anyOf` contains direct conditions or one-level
  `{ name?, allOf: [...] }` groups. Deeper or reverse nesting, mixed top-level
  keys, empty arrays, and blank group names are invalid.

Condition kinds include:
- `KeyExist` — whether a key exists (e.g., header, query param).
- `KeyMatch` — flattened `MatchCore`: exact `values` (no singular `value`) and/or `regex`. No wildcards. `caseSensitive` defaults to true and applies to `values` only. `negate: true` is the exclude spelling.
- `TimeRange` — whether the current time falls within a given range.
- `Probability` — probabilistic execution (e.g., 10% sampling).

Value predicates carry `negate` (default false), applied after an ordinary request-value miss (missing is base false; missing + `negate: true` is true). A Controller-owned reference payload that is unresolved is a separate state and always remains a non-match; `negate` must never invert it. `keyMatch` with `key.type: path` normalizes exact `values` at load time (regex is never normalized).

`RateLimitExceeded` is an execution-result dependency without `negate`: it is
supported only in a request-stage RateLimitRedis entry's `run` conditions and
references an earlier active Continue RateLimitLocal in the same resource by alias.
Shared validation rejects other placements and invalid references. Gateway
compilation binds the alias to a typed, invocation-local outcome slot. Before
the skip-first/run-second Boolean evaluation, all referenced outcomes must be
available; NotEvaluated returns generic 500 rather than becoming a false predicate.
Both ordinary request dispatch and ExtensionRef dispatch establish/restore frames.
See [local overflow knowledge](../../04-review/rate-limit/regression-guards/local-allowance-redis-overflow.md).

`keyMatch`, `jwtClaimMatch`, and `mtlsMatch` require at least one non-empty
`values` or `regex` list. This invariant is enforced by the shared `Condition`
validation path for every consumer; plugin-specific validators must not carry
partial copies of it. A present `values` list must not be empty, even when a
non-empty `regex` list is also present.

`jwtClaimMatch.claim` is required and must be non-empty after trimming. This
accessor invariant is also owned by shared `Condition` validation, before any
runtime claim extraction or `negate` handling.

`secretMatch` carries both a request-side `key: KeyGet` and a required
`secretKey` selecting exactly one entry from `secretRef`. The Controller never
ORs unrelated Secret data values. A missing, non-UTF-8, or blank selected value
leaves `resolvedValues` absent and is reported through `ResolvedRefs=False`.

Unresolved-reference policy is consumer-specific. Active entry/body `PluginConditions` seal the EdgionPlugins runtime fail-closed. `RequestRestriction` fails with the generic 500 response in `Deny` mode but treats an incomplete `Allow` group as unavailable and continues with sibling groups. `RequestAccessPolicy` likewise treats an incomplete allow candidate as unavailable. Dye remains non-blocking: an unresolved dye condition does not match. Missing or denied references retain the Controller status signal. `secretMatch` also reports an unusable selected data key; other present-but-unusable Secret consumers may rely on their resolver-specific status or warning plus Gateway enforcement. These paths do not add a per-request metric or tracing call.

The Gateway converts schema-level `ConditionSet` / `PluginConditions` values
into one shared immutable compiled representation when it builds the plugin
runtime. Besides preserving the regexes compiled during preparse, this step builds a
header-only HMAC verifier once and retains its credential map for the lifetime of the
runtime snapshot. Entry wrappers, secondary body gates, dye rules, RequestRestriction,
and RequestAccessPolicy must use this compiled representation; constructing an HMAC
verifier from schema configuration during request evaluation is a hot-path bug.

Request-body logical limits are also resolved during runtime construction.
`body_gate::plugin_limit_bytes` uses the schema's canonical byte-size parser;
`BodyGatePolicy` and `HmacVerifier` retain the resulting `Option<u64>`. The shared
`over_plugin_limit_len` runtime API accepts only that numeric policy and the exact
captured length, allowing equality and rejecting larger bodies without parsing.
HMAC resolves the limit after merging its intrinsic fail-close requirement with
operator tuning. It still authenticates headers before capture, uses the gateway
physical cap for shared storage, checks its independent logical cap, then hashes
the immutable body view without materializing a disk spill. Body-aware HMAC
predicates keep their existing no-match/negate handling for logical-limit outcomes.
See [ARCH-006's regression boundary](../../04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md).

Compiled set evaluation exposes log-less and detailed synchronous paths plus a
body-aware asynchronous path. They share the same ordered short-circuit
semantics. The body phase and dye use the log-less path; request/response
filters use the detailed path and may write `cond_skip` into the outer
`PluginLog`.

**Condition match-only logging threshold.** Count atomic `Condition` leaves once
when the logical decision scope is compiled. A scope with zero or one atomic
leaf MUST NOT emit match-only condition descriptors: the plugin outcome,
`cond_skip`, or policy decision already carries the useful result, and repeating
the sole match on every request is observability noise. Emit match-only atomic
descriptors only when the scope contains more than one leaf and the matched
path helps explain a genuinely composite decision. For `PluginConditions`, the
scope is the combined `skip` plus `run` gate; for RequestRestriction it is the
whole restriction expression; for RequestAccessPolicy it is one typed
rule/candidate's supplemental condition expression. This threshold never
suppresses `cond_skip`, final allow/deny/reject records, named-group
attribution, unresolved/error diagnostics, or body-read failures. Compute the
flag at runtime construction, never by recounting leaves per request.

Body phase: call the compiled conditions' log-less evaluation path, never the
detailed path that produces a skip descriptor.

Gateway API standard filters (added through the private `add_from_httproute_filters_inner`; the public entries are the scope-explicit `from_httproute_filters_for_rule` / `from_httproute_filters_for_backend` mentioned above) are not wrapped with conditions and always execute.

Skip log format: `"skip:keyExist,hdr:X-Internal"` or `"!run:keyMatch,method"`.

mTLS conditions borrow `ClientCertInfo` from the live session for one synchronous
predicate evaluation. Subject/CN and lazily filtered SAN candidates are borrowed
slices; typed URI/email SANs strip display prefixes and DNS uses the canonical
prefix classifier. Any matching candidate determines the base result before one
final negation. Empty candidate sets suppress uncompiled-regex diagnostics.
Response-body conditions continue to read live inputs for every callback without
per-chunk logs or result caching. HeaderCertAuth explicitly clones the complete
record when it must retain ownership across mutable header operations; DSL copies
only the selected CN/fingerprint into owned VM values. See the
[ARCH-007 regression evidence](../../04-review/cpu-memory/regression-guards/response-condition-live-input-borrowing.md).

### ⚠️ Critical constraint: regex must be compiled during runtime construction

`KeyMatch` supports a `regex` field. The `compiled_regex` field is annotated with `#[serde(skip)]` and is `None` after deserialization. **`compile()` MUST run before the runtime is published**; otherwise every request that hits the fallback path will execute `Regex::new()`, causing significant CPU overhead. The Controller validates the schema for status but does not build a runtime. The Gateway compiles conditions while building each EdgionPlugins resource generation.

**Validation and compile call chains**:

```
Controller EdgionPluginsHandler::preparse()
  → EdgionPlugins::validate_plugins()      # schema validation for status; no runtime

Gateway HTTP/stream source and EGC/binding/attachment callbacks
  → PluginPublisher                       # plugins/publication/
      → PluginGeneration::compile(policy) # source slots masked by current scope
          → permitted entry cache         # constructors/companions shared by source
          → compile_plugin_conditions()   # compile once, never on request path
          → effective cross-entry validation
          → seal_fail_closed() / seal_plugin_policy()
      → current-policy retention checks
  → publish immutable class/connection views
  → request/session captures its applicable view
```

**When adding a new condition kind that uses regex**: add a corresponding arm to `Condition::compile()`'s match; otherwise the new kind's regex will never be compiled.

### ⚠️ Critical constraint: no PluginLog in body phase (per-chunk path must allocate zero)

**Background**: the trait signature of `UpstreamResponseBodyFilter::run_upstream_response_body_filter` **deliberately omits** the `&mut PluginLog` parameter. The body phase covers data callbacks and terminal completion without trailers — for a long stream (gRPC server-streaming, large file downloads), this can mean hundreds to thousands of invocations per request. Any per-chunk log allocation accumulates at O(chunks × plugins) and has previously caused two bugs:

- `ExtensionRefFilter` called `start_edgion_plugins_log` in the body phase, pushing one entry per chunk into `ctx.pending_edgion_plugins_logs` without a stage drain — long streams leaked memory continuously (fixed, issue g2-001).
- `run_upstream_response_body_plugins` allocated a `PluginLog::new(filter.name())` for every filter on every chunk and immediately dropped it — pure wasted allocation, plus silent loss of `cond_skip` and other diagnostics (fixed, issue g2-004).

**Hard constraints to follow when designing new plugins, conditions, or wrappers**:

1. **Do not add `&mut PluginLog` to `UpstreamResponseBodyFilter::run_upstream_response_body_filter`**. The trait signature is the contract; it promises no per-chunk logging in this phase. If diagnostics are truly needed someday, follow #4.
2. **When implementing a new body-phase filter**, do not call `log.push*`, `log.set_*`, or any
   `tracing::` macro. Use bounded metrics or per-request aggregation emitted outside the per-chunk path.
3. **When adding a new body-phase wrapper** (similar to `ConditionalUpstreamResponseBodyFilter`), do not create temporary logs via `PluginLog::new(...)`. Condition evaluation must go through `CompiledPluginConditions::evaluate`, never `evaluate_detail`.
4. **For "body-phase summaries"** (e.g., how many times this request hit rate limiting): aggregate per-request, not push per-chunk. Add an aggregation slot in `EdgionHttpContext`, accumulate during the body phase, and emit once at the end of the request into `stage_logs`. **Never** write per-chunk execution traces into the access log.
5. **ExtensionRef body path**: use `ExtensionRefFilter::resolve_runtime_silent`, **not** `prepare_execution` (the latter calls `start_edgion_plugins_log`).
6. **Resource / configuration error diagnostics** ("referenced EdgionPlugins not found", "wrong kind", etc.) should be surfaced in Controller preparse / status conditions; **do not** rely on body-phase runtime logs as a backstop — body phase silently no-ops, and per-chunk error logs are themselves a DoS risk.

**Self-checklist** (run through this for every body-phase change during PR review):

- [ ] No `&mut PluginLog` added to the `UpstreamResponseBodyFilter` trait or its impls.
- [ ] No heap allocations on the body-phase hot path (`PluginLog::new(...)`, `String::from(...)`, `format!(...)`, `Vec::new()`, etc.).
- [ ] Condition evaluation on the body path calls `CompiledPluginConditions::evaluate` (grep for `evaluate_detail` on the body path — must be absent).
- [ ] No `ctx.pending_edgion_plugins_logs.push` or `session.start_edgion_plugins_log` calls on the body path.

## Wasm runtime ownership

Each constructed `WasmPlugin` owns a private `WasmRuntime`, including its load
slot, atomic module publication, pool, shared data, tick, and callout grant.
`PluginGeneration` / `EntryCache` already shares constructed filters by Arc
within one immutable source generation and one stage/index entry. Multiple
route or policy views of that entry therefore reuse its runtime; separate
entries, stages, resources, and rebuilt generations do not share mutable Wasm
state, even with identical module bytes.

The global Wasm manager retains the Engine and epoch ticker. The existing bounded
OCI artifact byte cache remains a separate fetch optimization. There is no global
mutable runtime map, manual plugin refcount, grant aggregation, or 60-second
module eviction grace. Configuration reconstruction may reset shared data and
repeat compilation; in-flight requests may finish using their captured generation.

The private runtime owner closes background task admission/publication and aborts
preload, refresh, and tick on destruction. Background tasks retain execution state,
never the owner that issues cancellation. Late completion cannot publish or restart
work after closure. Cancellation is eventual at host await points or bounded guest
execution boundaries; old/new generations may briefly overlap. OCI refresh failures
retain that runtime's last successful entry without rollback choreography.

Warm callbacks read the private atomic publication directly. Cold work preserves
single-flight preload, negative-cache retry deadlines, and claim-before-copy
allocation discipline. Request and tick callouts use the same plugin's immutable
grant; separate plugins neither contribute authorization nor disable each other's
ticks. See [runtime isolation](../../04-review/wasm/regression-guards/wasm-module-cache-key-runtime-config.md)
and [background shutdown](../../04-review/wasm/regression-guards/tick-quiescence-on-zero-reference-eviction.md)
for evidence, history, and verification limits.

## HTTP plugin inventory

The schema enum `EdgionPlugin` in
`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` is the authoritative inventory.
Do not rely on a hard-coded total: adding or removing an enum variant must be reflected in the
gateway creation factory and stage support. The current inventory includes Gateway API filters and
custom plugins such as FaultInjection, RequestId, ExtProc, RequestAccessPolicy, Canary, and Wasm.
The category tables below are a conceptual guide, not an exhaustive registry.

### Authentication (8)

| Plugin | Module | Description |
|--------|--------|-------------|
| BasicAuth | `basic_auth` | HTTP Basic auth |
| JwtAuth | `jwt_auth` | JWT token verification |
| KeyAuth | `key_auth` | API key authentication (header/query) |
| LdapAuth | `ldap_auth` | LDAP directory authentication |
| HmacAuth | `hmac_auth` | HMAC signature verification |
| HeaderCertAuth | `header_cert_auth` | Authentication based on certificate info in request headers |
| OpenidConnect | `openid_connect` | OpenID Connect authentication |
| ForwardAuth | `forward_auth` | Forward auth requests to an external service |

### Security (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| Cors | `cors` | CORS cross-origin resource sharing policy |
| Csrf | `csrf` | CSRF token verification |
| IpRestriction | `ip_restriction` | IP allow/deny lists |

### Traffic control (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| RateLimitLocal | `rate_limit_local` | Local in-memory rate limiting |
| RateLimitRedis | `rate_limit_redis` | Distributed rate limiting based on Redis (Lua scripts) |
| BandwidthLimit | `bandwidth_limit` | Bandwidth limiting (UpstreamResponseBodyFilter phase) |

### Request / response transformation (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| ProxyRewrite | `proxy_rewrite` | Proxy path / Host / header rewriting |
| ResponseRewrite | `response_rewrite` | Response header modification (UpstreamResponseFilter phase) |
| RealIp | `real_ip` | Extract the real client IP from headers such as X-Forwarded-For |

### Routing (4)

| Plugin | Module | Description |
|--------|--------|-------------|
| DynamicExternalUpstream | `dynamic_external_upstream` | Dynamic external upstream (specify external address at runtime) |
| DynamicInternalUpstream | `dynamic_internal_upstream` | Dynamic internal upstream (select K8s Service at runtime) |
| DirectEndpoint | `direct_endpoint` | Specify a backend endpoint address directly |
| RegionRoute | `region_route` | Multi-region traffic routing. All logic AND the base `regions` table live in the plugin config: ordered routeRules (RouteByDirectRegion / RouteByKeyConfMatch / RouteByHashRange) + hashKeyGet passthrough + hashCalc writeBack + failoverTo (static redirect). An optional `overrideRef` names a `RegionRouteOverride` EdgionConfigData, looked up in EdgionConfigDataGatewayStore per request; it is data-only and never carries logic — the type declares `enable`/`active`/`regions` but only `regions` is consumed (whole-replace); the envelope `spec.enable` is what takes an overlay out of service. Fail-to-base on disabled/missing/type-mismatched/invalid overlay. No auto failover. |

### Other (8)

| Plugin | Module | Description |
|--------|--------|-------------|
| Mock | `mock` | Return a mocked response (no upstream forwarding) |
| DslPlugin | `dsl` | DSL script engine (Parser→AST→Compiler→Bytecode→VM, ~4700 lines; see [`dsl-engine.md`](dsl-engine.md) for the architecture) |
| CtxSet | `ctx_set` | Set request context variables |
| JweDecrypt | `jwe_decrypt` | Decrypt JWE-encrypted tokens |
| RequestMirrorPlugin | `request_mirror` | Request mirroring (clone the request to another backend) |
| RequestRestriction | `request_restriction` | Request restriction (by path, method, etc.) |
| AllEndpointStatus | `all_endpoint_status` | Return the status of all backend endpoints |
| DebugAccessLogToHeader | `debug_access_log` | Write access-log info to response headers (for debugging, UpstreamResponseFilter phase) |

## DSL script engine (DslPlugin)

EdgionDSL is a lightweight request/response rule engine built on a custom bytecode VM (`edgion-gateway/src/plugins/http/dsl/`, ~4700 lines). It implements both `RequestFilter` and `UpstreamResponseFilter`; the UpstreamResponseFilter path bridges the sync trait into the `async fn execute` VM via `tokio::task::block_in_place`. Panics are isolated by `vm::execute_safe` (`AssertUnwindSafe + catch_unwind`), and a layered sandbox caps source size (≤64 KB), AST nodes (≤2000), bytecode size (≤256 KB), and runtime budgets (steps / loop / call / stack / string / list / map).

The full architecture — data flow, source layout, plugin entry points, async/sync bridging, panic isolation, and `DslRuntime` lifecycle — lives in [`dsl-engine.md`](dsl-engine.md). User-facing syntax / builtin signatures / runtime tuning are documented in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/`.

`Value::Str` / `Value::List` type design (why `Arc<str>`, not SSO or `ImmutableString`): [`skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`](../../04-review/dsl/intentional-design/sso-migration-not-worth-it.md).

## Stream plugins

Stream plugins run at the TCP/UDP layer and use a two-stage architecture:

| Stage | Location | Timing | Plugins |
|-------|----------|--------|---------|
| Stage 1: connection context | `stream_plugin_runtime.rs`; `connection_filter_bridge.rs` for Gateway listeners | pre-TLS for TCP-based listeners; new-session admission for UDPRoute; IP/port context only | `spec.plugins` (`EdgionStreamPlugin`) |
| Stage 2: TlsRoute | `tls_route/` | post-TLS handshake, post-route match | `TlsRouteIpRestriction` |

- `StreamPlugin` trait: defines the `on_connection(ctx) -> StreamPluginResult` interface.
- `TlsRoutePlugin` trait: defines the TLS-route-level plugin interface.
- `StreamPluginRuntime`: manages the Stage-1 connection-context chain. Gateway attachments invoke it through Pingora's `ConnectionFilter`; TCPRoute invokes it per connection, and UDPRoute invokes it when admitting a new session. Gateway attachment does not cover UDP listeners because they run as independent Tokio tasks. The runtime derives `Clone` over `Vec<Arc<dyn StreamPlugin>>`, so cloning it copies the vector of `Arc` handles and shares the plugin instances.
- `TlsRoutePluginRuntime`: manages the plugin chain for the TLS-route stage. The runtime itself does not implement `Clone`, while its plugin-chain elements are `Arc<dyn TlsRoutePlugin>`.
- `StreamPluginStore`: forwards EdgionStreamPlugins source changes to `PluginPublisher`; `StreamPluginGeneration` shares demanded stage instances across class views. Listener admission compares serving Gateway views, while matched route admission captures its owning class.

## Directory layout

```
edgion-gateway/src/plugins/
├── http/                          # HTTP plugins; authoritative variants live in EdgionPlugin
│   ├── basic_auth/                # one subdirectory per plugin
│   ├── jwt_auth/
│   ├── cors/
│   ├── dsl/                       # DSL engine (with lang/ subdirectory: AST, compiler, VM)
│   ├── ...
│   ├── common/                    # shared plugin utilities (auth_common, http_client)
│   ├── conf_handler_impl.rs       # EdgionPlugins ConfHandler implementation
│   ├── plugin_generation.rs       # immutable source and demand-owned entry cache
│   └── plugin_store.rs            # source adapter and effective runtime assembly
├── publication/                   # canonical class/binding publication and captured views
├── stream/                        # TCP/UDP layer plugins
│   ├── ip_restriction/            # Stream IP-restriction implementation
│   ├── tls_route/                 # TLS-route-level plugins
│   │   ├── ip_restriction.rs      # TLS-route IP restriction
│   │   ├── tls_route_plugin_runtime.rs
│   │   └── tls_route_plugin_trait.rs
│   ├── connection_filter_bridge.rs # Pingora ConnectionFilter bridge
│   ├── stream_plugin_runtime.rs   # stream plugin runtime
│   ├── plugin_generation.rs       # demanded stage entry cache
│   ├── stream_plugin_store.rs     # stream source adapter
│   └── stream_plugin_trait.rs     # StreamPlugin trait definition
└── runtime/                       # plugin framework
    ├── plugin_runtime.rs          # PluginRuntime core (4-phase management + plugin creation factory)
    ├── conditional_filter.rs      # conditional wrapper (4 Conditional* types)
    ├── conditions/                # condition evaluation engine (KeyExist, Include, Probability)
    ├── session_adapter.rs         # PingoraSessionAdapter (Pingora Session → PluginSession adapter)
    ├── log.rs                     # plugin execution log (PluginLog, StageLogs)
    ├── traits/                    # trait definitions for the 4 phases
    │   ├── request_filter.rs
    │   ├── upstream_response_filter.rs
    │   ├── upstream_response_body_filter.rs
    │   ├── upstream_response.rs
    │   ├── session.rs             # PluginSession trait (unified session abstraction)
    │   ├── ai_session.rs          # AI session integration capability
    │   └── waf_session.rs         # Selected-WAF session integration capability
    └── gateway_api_filters/       # Gateway API standard filter implementations
        ├── request_header_modifier.rs
        ├── response_header_modifier.rs
        ├── request_redirect.rs
        ├── url_rewrite.rs
        ├── extension_ref.rs       # ExtensionRef (route/backend filter list referencing EdgionPlugins, Gateway API standard) — nested form removed; see docs/en/migrations/<version>-extension-ref.md
        ├── cors_filter.rs
        └── debug_access_log.rs
```

## RequestId attachment phases

W3C propagation is initialized by the core earliest parsed-header callback, independent of plugin attachment. `RequestId` is the only correlation-ID plugin; `RequestTraceId` is internal request state. Configured global and Gateway RequestId entries run after unambiguous logical Gateway ownership and scoped policy admission. Ambiguous shared listeners defer both tiers until canonical route ownership is known, before preflight; TLS handshake evidence alone cannot select a class. Route entries run in ordinary order and apply incremental patches. BackendRef RequestId is rejected.

The early projection preserves enabled/condition semantics and rejects unavailable sources, body and dye wrappers using resource-layer validation. The normal global/Gateway pass skips entries already assigned to the early phase; route occurrences of the same resource still execute normally. Resolve every tier from the captured class scope, including missing results and delayed backend references. A later route owned by a different Gateway fails closed before borrowing another scope. W3C state cannot be disabled or rewritten by custom rules.

See [Request identity lifecycle](../../04-review/observability/intentional-design/request-trace-id-lifecycle.md) and the [RequestId user schema](../../../docs/en/user-guide/http-route/filters/edgion-plugins/request-id.md).

### Actual managed HTTP plugin inputs

Enabled request-stage ForwardAuth, Guardrail and AiGuard evaluators borrow their
actual EdgionPlugins entry through `refs::plugin_fields`. The shared managed
client proves owner/Service identity, exact request-template Secret map presence
and local TLS material before entry-cache reuse or client preparation. Omitted Service namespace inherits the plugin owner;
explicit empty namespace is invalid. These finite custom fields accept omitted,
empty or `core` group, and preserve Service/ServiceEndpointSlice/ServiceClusterIp
transport modes while the resource identity remains core Service. Native Gateway
API backend references do not inherit this convenience.

The Service target has no added ReferenceGrant or dependency policy. Template and
TLS material references retain their separate Controller authorization. Invalid
owner/target/template/TLS input replaces prior success with a closed runtime; malformed URL
syntax is also a configuration rejection, not evidence of a permission decision.
Disabled/policy-masked entries and AiGuard without an evaluator remain inert.
The managed and internal Route adapters share one private cold presence check,
using the canonical template collector and exact `namespace/name:key` map keys.
Missing maps or keys fail before TLS/client/health preparation; present empty
values remain valid. This check neither authorizes Secret acquisition nor proves
freshness. Intrinsic syntax validation and the renderer's existing missing-value
policy remain separate; literal Value fields are not template references.

Service transport uses HTTPS only for enabled local TLS; absent/disabled local
TLS uses HTTP. An explicit HTTPS URL stays HTTPS with disabled local material,
and an HTTP URL with enabled TLS remains invalid. Client construction reuses the
existing egress, timeout, retry, bounded-response and health-task implementation.
Guard request POST construction and request/response companion ownership remain
unchanged. Internal HTTPRoute Edgion-auth uses the route-owned path below.
OAuth2 credential acquisition remains a separate migration boundary; native
ExternalAuth is unsupported.
No new global-default cache invalidation or immediate in-flight revocation fence
is implied by this cold owning-input validation.

### Internal HTTPRoute auth ownership

The filesystem/etcd Edgion `ExternalAuth` payload (`target` plus `decision`) uses
actual rule/backend/filter loans from `refs::route_auth_fields`. This is distinct
from unsupported native Gateway API `backendRef`/`protocol` ExternalAuth; mixed
payloads keep the native classification and fail closed. No vendored CRD is relaxed.

The Controller resolves each original declaring HTTPRoute's TLS/template fields
before delegation inheritance. Secret/ConfigMap and ReferenceGrant recovery keys
belong to the published root, while authorization uses the actual declaring route.
Incoming root and raw-child resolved material/locality is cleared. Effective
filters carry schema-hidden declaring namespace and failure state; that locality
cannot mint a child Grant query or claim a child UID/generation.

Gateway `PreparedRouteAuth` retains the actual effective config loan through TLS
proof and ForwardAuth construction, using the same captured global TLS defaults.
Target identity and any unresolved/material diagnostic precede client or health
task preparation. Service namespace defaults to the declaration, including
intermediate inherited and selected-backend filters; TLS enabled/scheme rules are
the same as the owned plugin path. Failures seal a generic 500 and do not silently
remove authentication. Existing route generation/captured-request behavior,
egress policy, body forwarding, retry and deadlines are unchanged. No route entry
cache, Service grant policy, new Service recovery edge or live conformance claim
is added.

### Live IP dependency dispatch

A callback-local PingoraSessionAdapter captures ConfigData once when IP/Selector
reads are needed. Object-safe scalar readers return availability, membership or
profile index, never a resource/snapshot loan. The complete effective runtime's
required entry/body and Deny-restriction roots are checked before any filter,
including early RequestId, backend dispatch and nested ExtensionRef loops. Dynamic
absence is recoverable and never a permanent cold seal; ordinary failures return
500, candidate/observational and optional RAP roles retain their own policies.
After response commitment, body failure clears current plus sink-queued output and
all body/trailer owners and conversion output/budgets before a non-retryable error.
The public body filter trait and streaming/backpressure boundary are unchanged.

## ResponseCache intent and certificates

ResponseCache's public PluginSession seam accepts only an opaque cold-compiled
policy and reports Admitted, AlreadyAdmitted, Bypassed or Unsupported. The default
capability is Unsupported with no state allocation. The real adapter delegates
to the canonical private RequestState admission owner; it exposes no mutable ctx
or Store API. Repeated equivalent intents allocate nothing; conflicts are sticky
bypass. Lookup occurs at the successful request tail, never during nomination.

Cold EntryBundle certificates include companion filters and propagate through
reuse, outcomes and policy selection. Conditions cannot erase unsafe ownership.
Monotonic runtime identities distinguish replacements even when filter Arcs are
shared. Publisher retirement clears index visibility and fences pending commits.
Actual discovery, TLS, traffic and eligible literal authority reports describe
consumed reads, without selecting another backend or loading another snapshot.
