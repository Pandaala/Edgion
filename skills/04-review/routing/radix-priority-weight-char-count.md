---
name: radix-priority-weight-char-count
description: Use when reviewing HTTP route precedence findings about RadixPath::priority_weight and whether the path length term should be UTF-8 bytes or Unicode characters.
status: fixed
finding_id: gw-http-match-02
closed: 2026-07-28
---

# `RadixPath::priority_weight` counts Unicode characters, not UTF-8 bytes — FIXED

## Conclusion

`RadixPath::new` (`edgion-gateway/src/routes/http/match_engine/radix_path.rs`) derives the
path-length term of `priority_weight` from `normalized.chars().count()`. Before commit
`8ec9dbe1`, it used `normalized.len()`, i.e. the UTF-8 **byte** length, while the local variable
was named `path_char_count`, the adjacent comment quoted the Gateway API spec's "largest number
of **characters**", and `skills/01-architecture/02-gateway/03-routes/SKILL.md` documented the
same character-count contract. The implementation now uses the documented unit.

Fix suggestions of the form "revert to `str::len()`, it is O(1) and byte length is close enough"
are **not accepted**.

## Core Rationale

**1. The spec wording being modeled is literally "characters"**

Gateway API's `HTTPRoute` precedence order (`httproute_types.go` godoc) is: `Exact` path match >
`Prefix` path match with the largest number of characters > method match > header-match count >
query-param-match count. `priority_weight` uses a path-length term, so that term must use the
same unit as the contract it claims to implement. This finding is limited to that unit choice;
it does not prove that the complete `path_char_count * 4 + type_bonus` formula implements every
Exact/Prefix precedence case, especially when Edgion named parameters participate.

The vendored Gateway API v1.5 CRD restricts Exact/PathPrefix values to ASCII RFC 3986 characters
or `%HH` escapes. Consequently, byte and character counts are identical for Kubernetes-admitted
standard values. The old implementation was not a demonstrated Kubernetes conformance failure;
the practical difference is confined to File/etcd inputs that can carry Edgion's named-parameter
extension together with raw multi-byte text.

**2. The divergence was real but ordering-only**

It never affected *whether* a request matches a rule: the radix tree matches the actual path
content and that was always correct. For standard literal Exact/PathPrefix matches, two
simultaneously matching prefixes are ordered the same way by byte and character length because
one literal prefix must extend the other; two different literal Exact paths cannot both match.

An observable reversal is possible outside Kubernetes admission with Edgion's named-parameter
extension. For example,
`/:x/€` (5 characters, 7 UTF-8 bytes) and `/ab/:x` (6 characters and bytes) both match
`/ab/€`. The former byte count incorrectly outranked the latter before the fix, while character
count gives the intended order. The old implementation therefore violated its stated unit and
could change candidate ordering in File/etcd extension-aware configurations, but the original
`/café` versus `/caféx` discussion did not by itself demonstrate two simultaneously matching
routes and raw multi-byte literals are rejected by the Kubernetes CRD.

**3. `chars().count()` costs nothing where it runs**

`RadixPath::new` is called from `RadixRouteMatchEngine::initialize_internal` during route-table
(re)build, once per extracted path — config time, not the per-request path. The single consumer
is `cmp_candidate` in `radix_route_match.rs` (`weight_b.cmp(&weight_a)`), which reads the
already-computed `usize`; since 2026-09-02 that comparator also runs at build time only, inside
`assign_precedence_ranks` (see
[radix-candidate-precedence-rank-precomputed.md](radix-candidate-precedence-rank-precomputed.md)). The extra O(n) scan is invisible; there is no
data-plane cost argument for keeping the byte-length form.

## Fix Suggestions Not Accepted

- Revert to `normalized.len()` for O(1) — the length is computed at route-table build time and
  stored; the O(n) pass never touches the request path.
- "Fix the docs instead" — rewriting `03-routes/SKILL.md` / `01-http-route.md` to describe byte
  length would codify the divergence from the spec wording rather than remove it.
- Normalize or reject non-ASCII path patterns at shared resource ingest — a much wider
  cross-mode admission-policy change that would remove existing File/etcd extension inputs;
  it is not required to correct this local unit mismatch.

## Re-evaluation Triggers

Re-open this decision only if:

- Gateway API changes the precedence wording away from "number of characters" (e.g. to
  segment count or to an explicitly byte-based measure).
- `RadixPath::new` moves onto a per-request path, at which point the cost model for
  `chars().count()` needs re-examining.

## Reference Cases

- `gw-http-match-02` (fixed by `8ec9dbe1` on 2026-07-28) — original finding, Severity Low.
- `edgion-gateway/src/routes/http/match_engine/radix_path.rs` —
  `tests::test_priority_weight_counts_chars_not_bytes` locks the character-count semantics on a
  2-byte UTF-8 literal.
- `skills/01-architecture/02-gateway/03-routes/SKILL.md` — the documented formula, now matched
  by the implementation.
- Distinct from `skills/04-review/dsl-builtins/string-len-bytes-vs-chars-contract.md`, where the
  DSL's byte/character split is deliberate (`max_string_len` is a memory budget in bytes,
  `len()` is a script-facing logical length in characters). No such competing design rationale
  exists here — code and comment both asserted characters.
- The separate question of whether a long named-parameter Prefix can outrank a shorter Exact
  match is outside this finding and must not be treated as closed by this decision.
