---
name: radix-path-specificity
description: Use when reviewing HTTP Exact/Prefix precedence, named-parameter identifiers, Unicode units, escaped colons, and the build-time path key.
knowledge_type: regression-guards
finding_id: http-route-named-parameter-precedence
---

# HTTP path specificity excludes parameter identifiers

## Conclusion and scope

Gateway HTTP radix candidates use the immutable `RadixPath::specificity`
(`PathSpecificity`) as the first input to `assign_precedence_ranks`. Descending
field order is:

1. `is_exact`: every Exact precedes every PathPrefix.
2. `static_segment_count`: nonempty static segments across the entire path.
3. `static_char_count`: Unicode static characters, including slashes, after
   radix `/::` escapes are resolved.
4. `segment_count`: nonempty segments, including one for each parameter.

A parameter marker and identifier contribute zero static characters.
Changing a capture identifier cannot change path specificity. Equal keys pass
to method, header count, query count, older creation timestamp, ascending
namespace/name, rule index, match index, and finally `path_idx`.

The aggregate ordering for Edgion parameters was explicitly approved by the
user during this task's Fix Impact gate on 2026-09-12. It is an extension
policy, not a Gateway API mandate. It does not prefer static segments merely
because they appear earlier. For `/a/b/c`, `/:x/b/c` beats `/a/:x/:y`,
while `/a/:x/c` and `/:x/b/c` tie at the path level. For prefixes,
`/a` beats `/:x/:y`: static constraints precede parameter depth.

This guard owns corrected path semantics, not a general restriction on future
policy design. Parameter capture, Regex precedence, and the request-time integer
sort are unchanged. Prefix trailing-slash eligibility is corrected below.

## Rationale and evidence

### Structural buckets preserve parameter aliases

`ISSUE-001` (`run-e6cbd068`, parameter-alias hostname outage) exposed a
separate insertion identity defect: the Gateway bucketed literal spellings,
but the leaf parser discards parameter names. Inserting 256 aliases therefore
exceeded the leaf's 255-value bound and withdrew the whole hostname group.

Gateway publication now buckets by `edgion_algo::matcher::radix_path::PathRadixKey`.
This opaque identity reuses the insertion parser, resolves escapes, and merges
adjacent static fragments. Static bytes and parameter markers remain different
variants; escaped literal colons cannot collide with parameters. Each authored
candidate retains its path, Exact/Prefix policy, capture names and precedence.
Key construction and insertion share empty-path and byte-length validation;
an alias cannot bypass input bounds by reusing an existing bucket.
The leaf capacity and hostname build-error policy are unchanged. Identity
construction happens only at publication, never during request matching.

Regression coverage: `structural_keys_preserve_the_insertion_grammar` checks
alias and escape identity; `parameter_aliases_survive_full_and_incremental_publication`
checks 255/256 aliases, unrelated literal/prefix/regex route retention, candidate
ordering, capture names, full publication, incremental addition and removal,
and Exact/Prefix consumption within a shared bucket. The insertion-bound and
alias-length regressions preserve validation independent of insertion order.
Re-evaluate if the insertion grammar, candidate ownership or bucket expansion
changes. This correction does not establish isolation for other build failures.

Verification for this correction:
- `cargo test -p edgion-algo matcher::radix_path::`: 27 passed.
- `cargo test -p edgion-gateway --lib routes::http:: -- --test-threads=1`: 587 passed.
  The first parallel run passed 586 and failed the GraphQL response-cache
  `response_cache_actual_graphql_tail_shares_final_bytes_across_matches_and_body_generations_only`
  test at its cache entry-count assertion. That test passed in isolation and in
  the serial suite; the concurrent failure's cause was not established here.
- Workspace all-target `cargo check` and `cargo clippy` passed; Clippy reported
  existing warnings outside this diff. Formatting and all repository quick-check
  guards passed. Independent review approved the final code and knowledge scope.

The original `http-route-named-parameter-precedence` task identified
`normalized.chars().count() * 4 + type_bonus` as the root cause. For the same
request `/a/b`, static Exact `/a/b` scored 19, parameter Exact
`/a/:identifier` scored 58, and parameter Prefix scored 56. The latter two
incorrectly won due to identifier length; type bonuses could not enforce
Exact-first ordering.

`edgion-algo/src/matcher/radix_path/builder.rs::parse_path` discards the
identifier when constructing a parameter node. The new key follows that
existing grammar without changing the shared parser:

- Only `/:` starts a parameter; it consumes the whole segment, including any
  subsequent colons. `/:id::tail` is one parameter.
- `/::` removes exactly one escaped colon. `/:::x` is static `/::x`.
- Internal `::`, as in `/foo::x`, is literal. A bare `:id` without a
  preceding slash is static in the radix parser.
- Repeated slashes are normalized before counting. A non-root PathPrefix trailing
  slash is then removed before both insertion and counting; only Exact retains
  its trailing slash as a static character without another segment. Empty parameter names retain existing
  parser eligibility and do not receive identifier weight.

Canonical authority: [Gateway API v1.5.0 HTTPRouteRule](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go#L170-L192)
requires Exact first, then longest-character Prefix and non-path precedence.
Among simultaneously matching standard literal prefixes, each longer
candidate extends the same path: segment count increases, or at equal segment
count character count increases. Thus the new key preserves that ordering.
Named-parameter ordering is the approved Edgion extension documented in
[route architecture](../../../01-architecture/02-gateway/03-routes/SKILL.md)
and the English/Chinese HTTPRoute path guides.

The shortest consumer chain is `extract_paths -> RadixPath::new ->
assign_precedence_ranks -> ordered_candidates -> deep_match`. Only the build
phase compares the larger fixed-size key. See the separate
[precomputed-rank guard](radix-candidate-precedence-rank-precomputed.md).

## Alternatives and limits

Subtracting identifier length from the scalar alone would leave Exact/Prefix
and multi-parameter ordering underdefined. Left-to-right static precedence
would choose `/a/:x/:y` over `/:x/b/c`; it was not selected. A redundant
parameter-count field is unnecessary because total and static segment counts
already determine it.

The new fields add fixed per-path storage; they introduce no additional
per-request allocation, pattern parsing, string comparison, or lookup.
No throughput or cache-footprint performance claim has been measured.

Historical `gw-http-match-02` was fixed by `8ec9dbe1` on 2026-07-28: it
changed UTF-8 byte length to Unicode character count. That was a unit correction,
not proof of the whole scalar formula. The scalar and its identifier-counting
semantics are now replaced; the Unicode unit remains required for static text.
Historical raw-Unicode examples apply to File/etcd extension inputs, not a claim
that Kubernetes accepts raw Unicode through the vendored structural CRD.
Do not use the previous formula or its old expected winners as current policy.

### Escaped static fragments retain their full prefix

Follow-up review of the structural-bucket correction found an existing insertion
defect: `/::literal/test` failed to match `/:literal/test` and instead matched
`/test`. Parsing an escape followed by another segment produced consecutive
`Static` fragments. Recursing into the next fragment from the same build node
could split its prefix before its parent attached it under the original child
key, losing the intended path prefix.

`PathRadixBuilder::insert` now consumes `PathRadixKey`'s canonical segments too.
Joining adjacent static bytes before recursive insertion makes the actual tree
and indexing identity share one interpretation, without changing parameter
boundaries, input bounds, terminal capacity or frozen traversal.

`escaped_segments_preserve_the_complete_path` and
`distinct_escaped_prefixes_keep_all_terminal_values` both failed before this
correction and pass afterward. They cover the wrong-suffix regression, an
independent `/test` route, multiple escaped segments, parameters before/after
escapes, descendant consumption, 256 distinct prefixes, and both insertion
orders. The focused radix suite passes 29 tests. Re-evaluate if parsing or
insertion bypasses the canonical segment representation.

The `HttpRouteNamedParameterPrecedence` dynamic scenario now also generates
255 independently conditioned aliases on a dedicated hostname, adds the 256th,
and removes it again through the Controller Admin API. Each phase probes all
256 selectors through real HTTP requests; p254/p255 additionally assert the
selected capture and the unresolved alternative via ProxyRewrite. Unrelated
Exact, Prefix and Regex requests stay live, and `/escaped/:literal/tail` must
succeed while `/tail` remains 404. Generated resources use the canonical apply
helper and deterministic cleanup names; no generated fixture files are stored.
The existing Kubernetes parent/phase contract is retained, but local validation
must not be reported as a fresh Kubernetes run.

Verification of the escaped-segment and capacity follow-up:
- Radix unit suite: 29 passed; HTTP routing suite (serial): 587 passed;
  RequestAccessPolicy suite: 23 passed; test-client dynamic registration tests:
  5 passed.
- Canonical `prepare.sh` succeeded. Local
  `start_all_with_conf.sh --suites base,HTTPRoute/Basic,HTTPRoute/NamedParameterPrecedence`
  and `test_client -g --dynamic-scenario HttpRouteNamedParameterPrecedence`
  passed against the rebuilt FileSystem Controller/Gateway/test-server stack.
  This executes the absent -> 255 -> 256 -> 255 -> absent phases, including
  all-selector and capture assertions. Scenario resource cleanup and process
  cleanup succeeded. Kubernetes execution was not rerun.
- Workspace all-target check and Clippy passed (existing warnings outside the
  diff), as did formatting, repository quick-check guards and integration
  preflight guards. Fresh independent review approved production semantics,
  dynamic lifecycle and actual test-server response assumptions.

## Verification and re-evaluation

Historical verification during the named-parameter repair (not rerun evidence for CAND-P1-001):

- `cargo check -p edgion-gateway --lib`: passed.
- `cargo test -p edgion-gateway --lib routes::http::`: 415 passed, 0 failed.
- `cargo fmt --all` applied; `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with existing workspace
  warnings. The complete diagnostic log was checked against changed line ranges:
  none of the diagnostics in the four modified Rust files point to modified lines.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  and gateway no-tracing guards: passed.
- Two fresh, separate read-only code-review subagents: LGTM in both rounds.
  They independently reviewed code, test replacement coverage, and knowledge;
  their conclusions do not substitute for the executed checks.

The old arithmetic-only assertions were replaced with key and behavioral
coverage because preserving their expected weights would preserve the defect.
Normalization, metadata, root, trailing slash, escaping, static Unicode units,
and Exact/Prefix ordering remain covered in `radix_path::tests`.

The real-engine tests in `conf_handler_impl::tests` assert expected winners
independently of the key: all four match categories, each aggregate level,
equal aggregate keys with different static positions, renamed and multibyte
identifiers, each non-path tie-break, reversed registration, candidate subsets,
dense ranks, and old/new snapshot capture names. Algebraic tests in
`radix_route_match::tests` cover every comparison level, all fixture subsets,
antisymmetry, and transitivity. These are in-process tests, not fresh evidence
of HTTP traffic, control-plane synchronization, or live hot reload.

The follow-up `http-route-named-parameter-precedence-integration` added the
`HttpRouteNamedParameterPrecedence` dynamic scenario. In the real FileSystem
Controller -> Gateway -> test-server topology it exercises initial -> updated ->
initial publication and verifies eight externally visible request outcomes in
each generation:

- static Exact over named-parameter Exact;
- named-parameter Exact over a more static overlapping Prefix;
- aggregate static-character and multi-parameter specificity;
- escaped-colon literal handling;
- method precedence after equal named-path specificity;
- a live static-constraint change that moves the winning backend from 30001 to
  30002 and back to 30001;
- short/long parameter renaming without changing the path key; and
- ProxyRewrite visibility of the current capture name while the inactive name
  remains unresolved.

Every generation uses `wait_phase`; there is no fixed sleep. A bootstrap sentinel
route keeps the Gateway's plugin publication scope available while the scenario
deletes every tested route and requires all eight paths to return 404 before the
initial generation and again during final cleanup. The scenario then deletes the
sentinel, plugin, EndpointSlices, and Services. FileSystem mode also deletes the
Gateway; Kubernetes mode retains its shared Gateway fixture between runs. Its
dedicated Gateway listener must be present when the Gateway process starts. For a focused local run, preload
`base,HTTPRoute/Basic,HTTPRoute/NamedParameterPrecedence` with
`start_all_with_conf.sh`, then invoke the dynamic scenario. Selecting
`HTTPRoute/NamedParameterPrecedence` as an ordinary static suite is invalid
because it has no static suite registration.

Fresh verification for that integration follow-up on 2026-09-14:

- `cargo test -p edgion-tests --bin test_client dynamic --no-fail-fast`: passed
  (3 tests).
- `cargo test -p edgion-tests --test conf_corpus_unknown_fields`: passed.
- `start_all_with_conf.sh --suites
  base,HTTPRoute/Basic,HTTPRoute/NamedParameterPrecedence`: passed, including
  Controller/Gateway resource convergence.
- `target/debug/test_client -g --dynamic-scenario
  HttpRouteNamedParameterPrecedence`: passed against that live local stack with
  all eight cases in all three generations.

On 2026-09-23, the scenario was registered in the Kubernetes dynamic inventory
with dedicated fixtures. It passed twice consecutively and passed in a nine-scenario
run against the OrbStack Kubernetes stack. Deleting and recreating its Gateway
between Kubernetes runs left the Gateway configuration snapshot populated but
all eight route requests returned 404 until a Gateway process restart; the
scenario now retains that shared Gateway fixture in Kubernetes mode, following
the [Gateway recreation lifecycle boundary](../../listeners/intentional-design/gateway-recreation-requires-restart.md). This
cross-process scenario uses ASCII-only fixtures. Raw-Unicode semantics have only
in-process coverage; File/etcd can carry that extension input, while the
Kubernetes Gateway API structural schema may reject it before Edgion sees it
and was not tested here. The full workspace test suite was not run during the
original repair, the FileSystem follow-up, or this Kubernetes follow-up.

Agent-doc validation initially found an unrelated pre-existing broken reference
to the UDP shutdown wire-coverage task. The then-deferred task was restored from
its owning [UDP knowledge page](../../listeners/regression-guards/udp-process-shutdown.md),
and the guard passed on rerun; no UDP implementation or verification claim was
changed by the routing repair.

Re-evaluate if parameter/escape grammar, normalization, the extension policy,
or the canonical Gateway API precedence changes. Request-dependent ordering
would also invalidate global pre-ranking and requires a separate design.

## Resolution references

- `CAND-P1-001` / `http-route-path-prefix-trailing-slash`: the earlier guard and
  tests incorrectly counted the Prefix trailing slash. See the correction below.

- `http-route-named-parameter-precedence`: identifier-dependent route selection;
  fixed and verified on 2026-09-12 under the explicitly approved aggregate policy.
  The active source task was removed; its integration follow-up is verified above.
- `gw-http-match-02`: historical byte-to-character correction, 2026-07-28;
  static Unicode character coverage retained under the new key.

## PathPrefix trailing-slash correction

Gateway API v1.5.0 `PathMatchPathPrefix` explicitly ignores a configured trailing
slash. `/abc/` must match `/abc`, `/abc/`, and `/abc/def`, excluding `/abcd` and
`/Abc`; `/abc` and `/abc/` have equal path specificity. Root `/` remains a
catch-all, while Exact `/abc/` still excludes `/abc` and `/abc/def`.

The previous resource preprocessing and radix constructor preserved the slash:
the longer radix key omitted `/abc` from candidates and the extra static
character incorrectly defeated non-path tie-breaks. Historical passing tests
asserted that erroneous precedence and did not disprove this defect.

`path_normalization::normalize::canonicalize_path_prefix` now removes the slash
from an already-normalized non-root Prefix at build time. Both
`HTTPRoute::normalize_path_matchers` (declared and resolved rules) and
`RadixPath::new` use it. `tree_key` and `PathSpecificity` consume the same
canonical value. No request URI is trimmed; no request-time parsing, allocation,
or logging is added. Prefix rewrite counts nonempty elements, and parameter
capture ignores empty segments, so these consumers retain their semantics.

Regression coverage includes resource preprocessing idempotence, normalization
ordering, root/Exact/Regex boundaries, real radix candidate sets, reversed
registration, non-path precedence, snapshot replacement, and parameter capture.
The `HttpRoutePathPrefixTrailingSlash` dynamic scenario verifies backend identity
across equivalent prefix spellings with an observable backend generation change.

Verified for CAND-P1-001 on 2026-09-12:

- HTTP route library tests: 417 passed; resource preparse tests: 27 passed.
- Local HTTPRoute Basic integration: 6 passed.
- `HttpRoutePathPrefixTrailingSlash`: passed with the shared FileSystem-backed
  Controller/Gateway/test_server topology, including initial absence, backend
  30001 to 30002 to 30001, segment/case negatives, and final cleanup. Backend
  changes prove winner publication; this does not independently prove both
  competing route updates arrived atomically. Unit tests cover both spellings.
- Format, workspace all-target check and clippy passed. Clippy reports existing
  warnings; no diagnostic points to changed lines or the new scenario.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  and Gateway no-tracing guards passed. Final doc validation required restoring
  an already-referenced, missing ExtProc wire-verification task; no ExtProc
  behavior or verification result was changed.
- Independent read-only review: LGTM, including the final cleanup implementation.

The first dynamic run used a client built before the cleanup change and failed
final absence checking. A fresh client build and complete rerun passed; the
failed run is not counted as successful verification. Kubernetes wire testing
and the full workspace test suite were not run.

Re-evaluate if normalization ordering, prefix matching, snapshot ranking,
parameter parsing, or prefix rewrite semantics change.
