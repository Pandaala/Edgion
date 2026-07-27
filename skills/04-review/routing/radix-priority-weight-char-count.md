---
name: radix-priority-weight-char-count
description: Use when reviewing HTTP route precedence findings about RadixPath::priority_weight and whether the path length term should be UTF-8 bytes or Unicode characters.
status: fixed
finding_id: gw-http-match-02
closed: 2026-07-27
---

# `RadixPath::priority_weight` counts Unicode characters, not UTF-8 bytes — FIXED

## Conclusion

`RadixPath::new` (`edgion-gateway/src/routes/http/match_engine/radix_path.rs`) derives the
path-length term of `priority_weight` from `normalized.chars().count()`. It previously used
`normalized.len()`, i.e. the UTF-8 **byte** length, while the local variable was named
`path_char_count`, the adjacent comment quoted the Gateway API spec's "largest number of
**characters**", and `skills/01-architecture/02-gateway/03-routes/SKILL.md` documented the same
character-count contract. For a path-match pattern containing a multi-byte literal the computed
specificity therefore diverged from the documented and specified one.

Fix suggestions of the form "revert to `str::len()`, it is O(1) and byte length is close enough"
are **not accepted**.

## Core Rationale

**1. The spec wording being modeled is literally "characters"**

Gateway API's `HTTPRoute` precedence order (`httproute_types.go` godoc) is: `Exact` path match >
`Prefix` path match with the largest number of characters > method match > header-match count >
query-param-match count. `priority_weight = path_char_count * 4 + type_bonus` is the encoding of
the first two tiers, so the length term has to be the same unit the spec names. Gateway API does
not forbid non-ASCII literals in a path-match `value` — it is a config-time route pattern, not a
wire-encoded request path.

**2. The divergence was real but ordering-only**

It never affected *whether* a request matches a rule: the radix tree matches on actual byte
content and that was always correct. It only skewed the *relative* order of two competing rules
that both match the same request, and only when a pattern contained a multi-byte character —
e.g. `/café` (5 characters, 6 bytes) was weighted `6 * 4 + 3 = 27` instead of `5 * 4 + 3 = 23`,
so it could out-rank an ASCII competitor it should have tied with or lost to. Severity Low,
but a genuine spec divergence rather than a documentation nit, because the code, its own
variable name, and two architecture docs all asserted behavior the implementation did not
deliver.

**3. `chars().count()` costs nothing where it runs**

`RadixPath::new` is called from `RadixRouteMatchEngine::initialize_internal` during route-table
(re)build, once per extracted path — config time, not the per-request path. The single consumer
is the comparator at `radix_route_match.rs` (`pb.priority_weight.cmp(&pa.priority_weight)`),
which reads the already-computed `usize`. The extra O(n) scan is invisible; there is no
data-plane cost argument for keeping the byte-length form.

## Fix Suggestions Not Accepted

- Revert to `normalized.len()` for O(1) — the length is computed at route-table build time and
  stored; the O(n) pass never touches the request path.
- "Fix the docs instead" — rewriting `03-routes/SKILL.md` / `01-http-route.md` to describe byte
  length would codify the divergence from the spec wording rather than remove it.
- Normalize non-ASCII path patterns (percent-encode / reject at ingest) so the two units
  coincide — a much wider admission-policy change for an ordering-only, Low-severity gap, and
  Gateway API permits those literals.

## Re-evaluation Triggers

Re-open this decision only if:

- Gateway API changes the precedence wording away from "number of characters" (e.g. to
  segment count or to an explicitly byte-based measure).
- `RadixPath::new` moves onto a per-request path, at which point the cost model for
  `chars().count()` needs re-examining.

## Reference Cases

- `gw-http-match-02` (2026-07-27) — original finding, `Disposition: confirmed`, Severity Low.
- `edgion-gateway/src/routes/http/match_engine/radix_path.rs` —
  `tests::test_priority_weight_counts_chars_not_bytes` locks the character-count semantics on a
  2-byte UTF-8 literal.
- `skills/01-architecture/02-gateway/03-routes/SKILL.md` — the documented formula, now matched
  by the implementation.
- Distinct from `skills/04-review/dsl-builtins/string-len-bytes-vs-chars-contract.md`, where the
  DSL's byte/character split is deliberate (`max_string_len` is a memory budget in bytes,
  `len()` is a script-facing logical length in characters). No such competing design rationale
  exists here — code and comment both asserted characters.
