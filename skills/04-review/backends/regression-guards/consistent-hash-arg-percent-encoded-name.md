---
name: consistent-hash-arg-percent-encoded-name
description: Consistent-hash ConsistentHashOn::Arg now decodes percent/plus-encoded query param names (via a fast-path-guarded url_decode) before comparing to arg_name, so an arg name like "session id" matches a client's "?session%20id=" / "?session+id=". Reject re-reports claiming the naive split('&') comparison silently misses encoded param names.
knowledge_type: regression-guards
---

# backends-F014 — Consistent-hash query-arg match decodes percent/plus-encoded param names

**Status:** ✅ fixed · 2026-05-24
**Severity:** Low (BadLogic) — encoded arg name silently fails to match → empty hash key → RR fallback for that request subset; requires an exotic arg name config; no crash / data corruption / security impact

## Conclusion

The `ConsistentHashOn::Arg(arg_name)` branch of `extract_hash_key` delegates to
`RequestQuery::current_hash_arg_raw`, preserving first-match current-query policy:
only pairs containing `=` qualify, raw-name equality OR canonical decoded-name
equality matches, and the returned value is borrowed raw bytes. The first matching
empty value terminates lookup and retains empty-key fallback; bare flags do not
qualify. Never replace this with frozen last-wins entry lookup.

The Query owner caches tokens and encoded-name decoding on demand. Raw equality
short-circuits decoding; plain names borrow original spans. Repeated reads and
structured auth deletions share the same backing and decoded cells. Genuine raw
replacement creates a new backing. This resolves uri-01 through reuse without a
second decoder, comparison-only parser, or copied raw LB value.

First use can allocate token storage even for plain names. The former standalone
scan was zero-allocation for plain names, so shared ownership is not a universal
allocation or CPU improvement. Measure first/last/missing keys, long unused tails,
repeated reads and combined edit/read paths before making performance claims.

## What was wrong (before the fix)

The branch did `if name == arg_name { Some(value...) }` over the raw, undecoded
query param name. `arg_name` is derived from the extensionRef name `arg.<key>`
(`http_route_preparse.rs::parse_consistent_hash_name`); Gateway API `ObjectName`
constrains only length (1–253), not character set, so an operator *can* configure
an arg key containing a space or non-ASCII char. When they do, a client's encoded
param name (`session%20id`, `foo+bar`) never equals the decoded `arg_name`
(`session id`, `foo bar`), the `find_map` returns `None`, the hash key is empty,
and selection falls back to round-robin — silently breaking cache affinity for
that request subset.

Reachability is low: real query param names (letters, digits, `-`, `_`) are never
percent/plus-encoded, so the mismatch only surfaces for an exotic arg-name config.

## Why the fix is correct

`uri_utils::url_decode` (lives in `edgion-resources/src/uri_utils.rs`; same helper
backing route-matching `parse_query_string`) is already exhaustively unit-tested
in `edgion-resources/tests/unit/uri_utils_test.rs` (`%20`→space, `+`→space, invalid `%XX` preserved literally,
multibyte UTF-8). The owner invokes it only for encoded components when decoding is needed. Backward compatible: a plain-identifier arg name still
matches raw via the `name == arg_name` fast path (including the case where an
operator previously worked around the bug by configuring the *encoded* form as
the arg name — that still matches raw and short-circuits before any decode).

## Fix Suggestions Not Accepted

- "Decode every param name unconditionally" — adds a per-request, per-param heap
  allocation on the data-plane hot path for an almost-never-triggered case; the
  marker guard keeps plain-name decoding allocation-free. Token initialization
  is a separate cost, and the complete first-use lookup can allocate.
- "Also percent-decode the param **value** for the hash key" — out of scope and a
  behavior change: it would alter the hash key (and therefore backend assignment)
  of every existing consistent-hash-by-arg deployment, not just the encoded-name
  edge case.
- "Just document that arg names must be raw-encoded / plain ASCII" (the issue's
  alternative) — the decode fix makes the natural config (`arg.session id`) work
  instead of pushing the encoding burden onto the operator.

## Re-evaluation Triggers

Re-evaluate when Query ownership, entry/current source selection, token lifetime,
canonical decoding, duplicate policy or raw value borrowing changes. Verify the
distinct current/entry, first/last and raw/decoded policies; shared ownership must
not collapse them.

## Reference Cases

- backends-F014 (review finding, fixed 2026-05-24)
- `uri_utils::url_decode` unit tests (`edgion-resources/tests/unit/uri_utils_test.rs`)
- Existing integration scenario `HTTPRoute_default_lb-ch-arg.yaml` (`arg.user_id`,
  plain-identifier no-regression path)
