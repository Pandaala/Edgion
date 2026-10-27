# Request Cookie management implementation plan

## Status and authority

- Status: implemented and independently accepted. See the final verification record for passed checks and explicit environment/measurement limits.
- Requested: deep multi-agent investigation, Envoy and Kong comparison, and an implementation plan.
- Source: `task/review-data-panel/auth-01-strip-survivor-storage.md`, Cookie portion only.
- Baseline: `6069963764e2435aaa834aaa8aa56028b0dec67a`. Concurrent deadline changes
  were inspected and do not alter Cookie semantics. Leave `impl/` untouched.
- The user approved full implementation, all related path migrations and multiple
  independent review/fix rounds. No commit, push, dependency/config/schema changes
  or response Set-Cookie redesign. Close the selected finding only after acceptance.
- Latest requirements: untouched Cookie headers pass through without parsing or
  reassembly; actual edits may use normal Cookie formatting and need not preserve
  original whitespace or field boundaries. Correctness takes priority over caching.

## Selected design

Keep Pingora's live HeaderMap as the single raw authority. Unify Cookie lookup and
multi-name deletion behind one canonical parser. Apply successful edits immediately.
The initially proposed two-pass planner and full index cache failed isolated
performance gates. The reviewed implementation amendment retains a single-pass
borrowed survivor Vec, with exact output built only on a match. Consider a
bounded lazy metadata cache only after its costs pass independent adoption gates.
Do not make cache integration a prerequisite for the safe canonical API.

A deferred Cookie overlay is rejected for this scope. Existing
`request_header_map(&self) -> &HeaderMap` feeds raw borrowed readers and intermediate
webhook forwarding. It cannot safely synchronize a mutable Pingora session. A
shadow HeaderMap or replacing all header access with a virtual view adds avoidable
cost and migration risk. Query's deferred projection is not a Cookie template.

Cookie reads remain live, including named webhook/template lookups. No frozen
entry Cookie copy is introduced. A constructed outbound request owns its snapshot;
subsequent parent edits do not retroactively modify it.

| Operation | Parsing | Publication |
|---|---|---|
| No Cookie consumer; raw forwarding/header inspection | None | Original HeaderValue fields |
| Explicit named Cookie read | Canonical scan, or admitted cached metadata | Never serializes or rewrites |
| Empty removal-name list | None | No change |
| Nonempty removal request, interpretable fields, no matching name | Scan required to establish absence | No header rewrite |
| Actual removal | Parse and calculate surviving output | One normalized Cookie replacement, or whole-header removal |
| Generic Cookie set/append/remove | Existing header validation; no eager Cookie parsing | Immediate live-header update |

No claim of zero parsing is made when a plugin explicitly asks for a named value
or requests deletion. No consumer means no parse, regardless of Cookie presence.

## Envoy and Kong evidence

Research was source-pinned; neither implementation is a universal Cookie spec.

- Envoy v1.40.0, commit `9e5580933739775e8bce696a57a2e1bc99368f23`:
  [utility parsing](https://github.com/envoyproxy/envoy/blob/9e5580933739775e8bce696a57a2e1bc99368f23/source/common/http/utility.cc#L303-L375)
  scans live headers per call. `parseCookieValue` returns the first value;
  `parseCookies` creates a fresh first-duplicate-wins map. The inspected utility
  and OAuth2 paths do not establish a request-wide parsed Cookie cache.
  [Removal](https://github.com/envoyproxy/envoy/blob/9e5580933739775e8bce696a57a2e1bc99368f23/source/common/http/utility.cc#L627-L646)
  rebuilds survivors and immediately replaces the live Cookie header. Its parser
  strips paired quotes and ignores bare segments, which differs from Edgion.
- Envoy's [HTTP/2 Cookie buffer](https://github.com/envoyproxy/envoy/blob/9e5580933739775e8bce696a57a2e1bc99368f23/source/common/http/http2/codec_impl.cc#L243-L255)
  and [HeaderMap separator](https://github.com/envoyproxy/envoy/blob/9e5580933739775e8bce696a57a2e1bc99368f23/source/common/http/header_map_impl.cc#L25-L32)
  implement raw-field coalescing with `; `, not a parsed parameter cache.
- Kong 3.9.1, commit `b724fc7154de3a9971e33490097d5ea2c1bae93b`:
  [Request Transformer](https://github.com/Kong/kong/blob/b724fc7154de3a9971e33490097d5ea2c1bae93b/kong/plugins/request-transformer/access.lua#L163-L231)
  removes/replaces whole headers, not individual Cookie names. Its header-table
  rewrite is not a reason to add unconditional rewriting here.
  [PDK setters](https://github.com/Kong/kong/blob/b724fc7154de3a9971e33490097d5ea2c1bae93b/kong/pdk/service/request.lua#L314-L334)
  immediately update Nginx headers. [JWT Cookie lookup](https://github.com/Kong/kong/blob/b724fc7154de3a9971e33490097d5ea2c1bae93b/kong/plugins/jwt/handler.lua#L47-L53)
  uses `ngx.var["cookie_" .. name]`, not a generic Cookie-map round trip.
- Kong's [pinned OpenResty version](https://github.com/Kong/kong/blob/b724fc7154de3a9971e33490097d5ea2c1bae93b/.requirements#L1-L4)
  corresponds to an upstream [Nginx Cookie parser](https://github.com/nginx/nginx/blob/release-1.25.3/src/http/ngx_http_parse.c#L1964-L2030)
  with case-insensitive names and additional delimiter behavior. This upstream
  comparison is not a verification of every Kong binary patch. Do not import its
  name matching into Edgion's case-sensitive contract.

Take the common ownership/formatting principles: current headers, immediate edits,
explicit parsing, semicolon separation. Preserve Edgion's existing supported token
semantics rather than selecting incompatible details from different gateways.
Do not serialize a key/value HashMap: duplicate names and order must survive.

Envoy's [Cookie/header memory advisory](https://github.com/envoyproxy/envoy/security/advisories/GHSA-22m2-hvr2-xqc8)
shows why separate Cookie storage must be included in decoded-memory accounting.
This motivates explicit cache bounds; it is not evidence Edgion has that flaw.

## Canonical semantics

Current evidence: `plugins/runtime/session_adapter.rs::cookie_value_ref`,
`plugins/http/common/auth_common.rs::plan_cookie_strip`, the KeyAuth/JwtAuth
credential-stripping knowledge, and OIDC session-cookie ownership knowledge.

- Lookup walks all Cookie fields in order; the first matching pair with `=` wins,
  including an empty value. Bare flags do not supply a value. No lowercase,
  percent decode, URL decode, value unquoting or additional value trim.
- Pair processing splits on `;`, trims the complete segment, then uses the first
  `=`. Consequently `sid =x` does not match `sid`; `sid="x"` returns quotes.
- Lookup retains `HeaderValue::to_str()` eligibility, which in pinned http 1.5.0
  is visible ASCII plus TAB, not every valid UTF-8 string. A non-text field before
  a match stops lookup; one after a successful match does not undo that match.
- Deletion matches raw names, including bare flags, and removes every matching
  occurrence across all fields. UTF-8 text that is not lookup-text eligible remains
  supported by the deletion planner. Keep these two policies explicit.
- On actual deletion, preserve surviving trimmed segments, their order, repeated
  names, embedded equals, literal percent bytes and quotes. Join with `; `; omit
  empty segments. Multiple input fields may become one. No raw-layout fidelity
  promise applies after an actual edit.
- If no selected names match, preserve all original fields and sensitivity without
  a write. This intentionally changes the old no-match normalization behavior.
- OIDC deletes Gateway session/state cookie names together, independently of
  hideCredentials. Response Set-Cookie remains a separate owner.
- LB currently inspects only the first Cookie field. Preserve that explicit field
  scope in this refactor. Its discrepancy with all-field readers is recorded for
  separate assessment; do not silently change endpoint selection.
- Native WAF Cookie collections retain their byte-level inspection grammar and
  budgets. Only the Gateway header snapshot must reflect edits immediately.

## Malformed input and publication safety

Do not silently preserve an opaque field during credential removal. For example,
`Cookie: sid=good` followed by a field containing `sid=good` and invalid UTF-8 can
pass first-field authentication. Keeping the second field would forward the secret.
Old stripping dropped opaque fields as a side effect of rewriting; that also
silently discards unrelated content.

Selected conservative policy: a nonempty Cookie-removal request prevalidates all
Cookie fields as UTF-8 before editing. Any invalid UTF-8 field produces
`Unsanitizable`; it is not a proven no-match. Leave headers unchanged and reject
further forwarding with a fixed 400. Ordinary untouched passthrough and named
lookup keep their existing behavior; this is not global strict Cookie admission.
No value, name, header content or parser detail appears in the response/log.

The typed removal API returns `Result<CookieEditOutcome, CookieEditError>`:

- Outcomes: Absent, NoChange, Removed, Replaced.
- Errors: Unsanitizable input; internal output-size/validation/publication failure.
- Internal failure maps to the existing fixed 500 plugin failure mechanism;
  Unsanitizable maps to fixed 400. No new failure-policy knob or retry.
- KeyAuth, JwtAuth and every OIDC continuing-upstream branch must propagate these
  failures. No `let _ = strip_cookies(...)` or unconditional GoodNext on failure.
  This explicitly replaces the current ignored Cookie-setter-error behavior.
- Preserve existing Query/header cleanup policies; do not widen this error change
  to unrelated credential sources.

Publication uses the existing typed sensitive-header setter, after protected-header
validation. All parsing, checked sizing and HeaderValue validation happen first.
Borrowing ends before mutation; errors before publication leave old headers intact.
A matched strip produces one replacement/removal, avoiding a fallible sequence of
remove-all followed by multiple appends. Verify pinned Pingora's constant-Cookie
insert path and failure tests; do not assume arbitrary batch header edits atomic.

Deferred credentials require explicit treatment: the old successful no-match
normalizing write canceled a deferred Cookie header mutation. Preserve that
cancellation for NoChange with existing interpretable Cookie fields, without a
physical header write; cover this with a real-adapter regression. Absent retains
its historical no-op. Empty names intentionally change the old normalizing behavior
to zero-parse/no-op, preserving both live headers and any pending deferred mutation.
Never read the derived main-header cache against a detached outgoing header.

## Stateless planner and optional lazy cache

Add one crate-private `request_cookie` module. A pure canonical tokenizer supplies
borrowed pair spans; caller policy controls field scope, text eligibility, empty
values and bare flags. PluginSession gains a borrowed current-cookie accessor and
a typed multi-name removal operation. Owned access remains for values crossing
awaits or mutation; mocks must not dictate production allocations.

Primary candidate for auth-01: explicitly change planner input from arbitrary
Iterator to a repeatable borrowed HeaderMap view. Validate fields, determine
whether removal occurs and calculate exact surviving bytes in pass one. If absent
or no-match, return without output allocation. For a match, pass two fills one
exact-sized String using the same tokenizer. This removes the survivor Vec but
repeats scanning/membership checks; measure against the current Vec+join baseline.
Preserve capacity-equals-length, sensitive HeaderValue conversion and payload
pointer transfer. Requested capacity is not proof of actual capacity: retain
allocation/pointer tests and measure the allocator result. An arbitrary iterator
is never assumed rewindable.

Optional cache candidate, isolated until measured:

    RequestCookieCache { parsed: OnceLock<Option<Box<CookieIndex>>> }

- Target at most 16 bytes inline, zero allocation/copy/scan when unused.
- Store flat field classifications and pair offsets only, no Cookie payloads,
  HeaderValue clones, name strings, arbitrary-lookup-name map, Arc history or pointers.
- Shared-read initialization is safe; values returned borrow live header storage.
  Keep cache+header access inside the main-request owner, not a free API accepting
  arbitrary unrelated HeaderMaps.
- Classify lookup-text eligibility separately from UTF-8 edit eligibility.
- Bound metadata at 8 KiB including index object, actual allocated capacity and construction overlap;
  check before allocation. On overflow release partial storage and memoize uncached
  mode. Use the same streaming tokenizer; never reject/truncate the HTTP input.
- No cache on raw passthrough. Named reads may initialize it only after adoption.
  Reuse parsing within an unchanged admitted generation. Successful relevant edits
  clear it; fallback and changed generations may need another scan.
- No remapping across edits in the first implementation. Full indexing can make a
  first-key/long-tail lookup slower; it is not accepted merely for warm-read gains.

## Mutation and export ledger

| Surface | Contract |
|---|---|
| Generic string/typed set, append helper, remove | Immediate publication; invalidate after successful case-insensitive Cookie mutation |
| Auth strip | One typed publication; preserve sensitivity, new errors and deferred cancellation |
| Dye anti-spoof removal | Direct configured-name removal can target Cookie; add invalidation |
| Rendered dye, DSL, Wasm, ExtProc, Gateway/backend header filters | Covered by generic setters; prove each call chain |
| AI whole-header baseline restore | Clear metadata after header replacement; existing AI snapshot is sole retry-history owner |
| Query URI snapshots/synchronization | Preserve Cookie bytes/order; no cache invalidation required |
| RequestId reconciliation | Cookie forbidden by blocked-header validation; preserve/test exemption |
| Deferred credential apply | Detached outgoing header only; not main HeaderMap mutation |
| Expect/Host/method/path changes | Fixed non-Cookie operations, exempt |
| HeaderExt, whole-map, byte/text/list accessors | Always current; raw reads do not initialize Cookie parser |
| Webhook/ForwardAuth origin, templates and forward_all | See successful edits immediately through accessor/live map |
| Mirror, ExtProc, Wasm, WAF, AllEndpointStatus | Owned exports snapshot the current headers, not a deferred overlay |
| Response cache admission/key/vary, logging, affinity/LB | Audit direct reads; preserve their explicit policy and redaction |

Add a focused ownership guard for parser bypasses and raw dynamic header mutation
sites. Include self-tests for multiline syntax and bulk replacement; the guard is
not a replacement for reviewing aliases and actual callback order.

## Implementation boundaries and steps

Freeze the exact edit manifest before implementation. Expected production homes:

- New `edgion-gateway/src/request_cookie.rs` or a small module directory, `lib.rs`.
- `plugins/runtime/traits/session.rs`, `plugins/runtime/session_adapter.rs`,
  `plugins/http/common/auth_common.rs`.
- Borrowed readers: `plugins/http/csrf/plugin.rs`, `plugins/http/jwt_auth/request_stage.rs`,
  `plugins/http/dsl/lang/builtins.rs`, `routes/http/session_persistence.rs`,
  `observe/access_log/log_ext.rs`, `backends/selector.rs` (explicit first-field scope).
- Failure callers: `plugins/http/key_auth/plugin.rs`, `plugins/http/jwt_auth/plugin.rs`,
  `plugins/http/openid_connect/session_cookie.rs` and `request_filter_impl.rs`.
  Review all five OIDC continuation sites, including refresh fallbacks.
- Optional cache only: `ctx.rs`, `ai/context.rs`, backend context plumbing where
  needed, and the already-listed writer surfaces. No cache integration before its gate.
- Matching unit modules, explicit benchmark target, focused integration fixtures,
  ownership guard, and existing semantic knowledge pages.

Execution after implementation approval:

1. Freeze named-read compatibility and new cleanup contracts with tests; capture
   benchmark baseline. Review exact files and failure-response mapping independently.
2. Implement canonical stateless parser and synchronous typed cleanup, including
   NoChange and safe errors. Run semantic and actual adapter regressions.
3. Compare Vec+join and two-pass exact output; choose based on full callback cost.
   Independently prototype bounded cache only if still justified; do not integrate
   a failed candidate. Preserve the unified API even if caching is rejected.
4. Integrate any accepted cache with complete mutation/export ledger coverage.
5. Two independent reviews (core/semantics and transport/outbound), corrections,
   then an independent recheck. Repeat for real findings, not ceremonial reviews.
6. Update knowledge only with verified results. Close auth-01 only when remaining
   Cookie work is accepted; retain any unresolved optimization instead of claiming it.

## Verification and performance gates

The following gates were frozen during planning; execution outcomes and reviewed
amendments are recorded below and in the final verification record. Add source-
mounted unit tests under `tests/unit/`; no production visibility widening.

Separate oracles: old named-read compatibility; new no-edit identity; existing
matched UTF-8 deletion output; new invalid-UTF-8 failure. Do not compare all new
behavior against an obsolete normalization oracle.

Cover duplicate names/fields, empty first value, bare flags, empty names, nested
`=`, quoted/percent-containing values, TAB/Unicode whitespace, empty segments and
fields, leading/trailing separators, ASCII/UTF-8/invalid bytes before/after matches.
Test no-match untouched fields and flags, mixed-case Cookie header names, same-
length set, append/remove, dye removal, failed mutations and AI restore.

A real adapter chain must prove: authentication -> cleanup -> intermediate webhook
forward_all/explicit Cookie header/named origin/custom template -> next reader ->
mirror -> backend echo. A cleanup failure must execute no downstream plugin,
webhook, mirror or upstream dispatch. Test all OIDC continuation branches, borrowed
values, sensitivity, deferred cancellation and zero-copy output conversion.

Measure release-mode CPU and allocations separately with alternating runs and
reported dispersion. Include current headers and old/new output overlap in peak
memory. Matrix: untouched/absent, 1/4/32/256 pairs, many short fields, long values,
first/last/missing lookup, repeated same/different names, dense pairs, high deletion
ratio, no-match deletion, budget overflow, post-write reads and AI retry. Include
whole adapter and webhook construction costs, not only pure helpers.

Freeze proposed gates before coding: unused path zero Cookie allocations/copies/
scans; optional cache inline size <=16 bytes and metadata <=8 KiB; cold first-hit
CPU regression <=5% when measurable; representative repeated read/edit sequence
CPU and peak-live regression <=5%/20%; repeatable intended-case benefit beyond
noise; exact-size owned HeaderValue transfer unchanged. Noisy or failed results
are not PASS. Do not average away a regression or relax gates without review.

Run scoped KeyAuth/JwtAuth/OIDC, CSRF/DSL, webhook/LinkSys/ForwardAuth, ExtProc,
WAF, mirror, response cache, H1/H2, LB/affinity and AI retry verification. Keep
existing Query regressions. Follow prepare/runtime phases and one shared topology.
Run format, workspace check/clippy, doc/layout/source/ownership and applicable
project guards. Do not run `cargo test --all` without an explicit request.
No new request tracing, metrics, global cache, unsafe production code or business
limit is required. No claim of universal performance improvement is made.

## Review record

Initial independent audits agree on live HeaderMap ownership. V1 review blocked
unmodified-header normalization, an unrequested LB behavior change and undefined
opaque-field handling. V2 removes LB changes, allows standard formatting only on
actual edits, and specifies fail-closed opaque cleanup with caller propagation.
Envoy/Kong research is comparative evidence; no source edits or benchmarks were
performed by the research agents. V2 independent reviews completed:

- `query_core_fresh_review`: core semantics and memory/correctness design PASS.
  Its final clarification explicitly distinguishes the new empty-name no-op from
  historical Absent behavior and preserves deferred cancellation for nonempty
  NoChange.
- `query_writers`: transport, intermediate outbound visibility, pinned Pingora
  publication ordering and all five OIDC continuation branches PASS.
- `query_core_implementation`: Envoy/Kong primary-source facts and independent
  benchmark/adoption design cross-check PASS. Actual capacity/peak accounting
  remains an implementation acceptance gate, not a planning measurement.

Planning verification: agent-doc consistency and whitespace checks passed. Only
this plan was added; runtime code, original auth-01 and unrelated `impl/` contents
were not modified. Runtime regression and benchmark results remain unmeasured.
The subsequent user instruction authorized implementation and repeated independent
review/fix rounds. Planning PASS is not implementation verification.

## Reviewed execution amendment: retain single-pass survivor storage

Fresh release measurements rejected the full cache and two-pass planner; see the
verification record. Independent core and transport reviewers approved one scan
that validates every field, retains borrowed survivors and checked output length,
then returns without output on NoChange or builds one sensitive HeaderValue on a
match. No-match may allocate metadata; it performs no header write. The two-pass
proposal above is retained as design history, not current production behavior.
No production cache is installed. Raw/unused requests remain parse-free; named
reads use the canonical stateless parser. Do not claim that the cache-first
candidate or removal of all intermediate storage shipped.

The optional candidate adoption gates are not claimed as a production cache or
end-to-end performance acceptance. Both alternatives failed isolated gates and
were rejected before integration. Independent final review accepts retaining the
existing single-pass representation with canonical ownership, no-match no-render
and typed failure propagation. The helper/HeaderMap sequence is limited evidence;
complete adapter/webhook CPU and peak memory remain unmeasured. Real adapter and
live export tests verify correctness separately. This supersedes the earlier
candidate requirement to demonstrate full-adapter performance before adoption;
no alternative representation was adopted and no whole-Gateway gain is claimed.
