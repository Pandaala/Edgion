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
policy design. Matching eligibility, parameter capture, Regex precedence,
and the request-time integer sort are unchanged.

## Rationale and evidence

The original `http-route-named-parameter-precedence` task identified
`normalized.chars().count() * 4 + type_bonus` as the root cause. For the same
request `/a/b`, static Exact `/a/b` scored 19, parameter Exact
`/a/:identifier` scored 58, and parameter Prefix scored 56. The latter two
incorrectly won due to identifier length; type bonuses could not enforce
Exact-first ordering.

`edgion-resources/src/matcher/radix_tree/builder.rs::parse_path` discards the
identifier when constructing a parameter node. The new key follows that
existing grammar without changing the shared parser:

- Only `/:` starts a parameter; it consumes the whole segment, including any
  subsequent colons. `/:id::tail` is one parameter.
- `/::` removes exactly one escaped colon. `/:::x` is static `/::x`.
- Internal `::`, as in `/foo::x`, is literal. A bare `:id` without a
  preceding slash is static in the radix parser.
- Repeated slashes are normalized before counting; a trailing slash adds one
  static character but no segment. Empty parameter names retain existing
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

## Verification and re-evaluation

Fresh verification during this repair:

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

Cross-process route-selection and dynamic integration evidence remains tracked
in [the integration follow-up](../../../../tasks/todo/http-route-named-parameter-precedence-integration.md).
Existing `edgion-tests/**` files are outside the approved repair scope.
The full workspace test suite was not run.

Agent-doc validation initially found an unrelated pre-existing broken reference
to `tasks/todo/udp-process-shutdown-wire-coverage.md`. The already-deferred
verification task was restored from its owning UDP knowledge page, and the
guard passed on rerun; no UDP implementation or verification claim was changed.

Re-evaluate if parameter/escape grammar, normalization, the extension policy,
or the canonical Gateway API precedence changes. Request-dependent ordering
would also invalidate global pre-ranking and requires a separate design.

## Resolution references

- `http-route-named-parameter-precedence`: identifier-dependent route selection;
  fixed and verified on 2026-09-12 under the explicitly approved aggregate policy.
  The active source task was removed; integration evidence remains tracked above.
- `gw-http-match-02`: historical byte-to-character correction, 2026-07-28;
  static Unicode character coverage retained under the new key.
