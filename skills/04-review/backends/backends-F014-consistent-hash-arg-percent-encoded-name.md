---
name: backends-F014-consistent-hash-arg-percent-encoded-name
description: Consistent-hash ConsistentHashOn::Arg now decodes percent/plus-encoded query param names (via a fast-path-guarded url_decode) before comparing to arg_name, so an arg name like "session id" matches a client's "?session%20id=" / "?session+id=". Reject re-reports claiming the naive split('&') comparison silently misses encoded param names.
---

# backends-F014 — Consistent-hash query-arg match decodes percent/plus-encoded param names

**Status:** ✅ fixed · 2026-05-24
**Severity:** Low (BadLogic) — encoded arg name silently fails to match → empty hash key → RR fallback for that request subset; requires an exotic arg name config; no crash / data corruption / security impact

## Conclusion

The `ConsistentHashOn::Arg(arg_name)` branch of `extract_hash_key`
(`src/core/gateway/backends/selector.rs`) now matches a query parameter when its
name equals `arg_name` **byte-for-byte** OR — only when the raw name contains `%`
or `+` — when `HttpRouteRuleUnit::url_decode(name) == *arg_name`. So a client
sending `?session%20id=abc` (or `?session+id=abc`) now matches an `arg_name` of
`session id`. The matched **value** is still extracted raw (not decoded) — decoding
it would change the hash key of every existing deployment.

The fast path keeps the data-plane hot path allocation-free in the common case
(real arg names are plain identifiers — `user_id`, `sid` — that match raw); the
`url_decode` allocation only happens for a param whose name actually carries a
`%`/`+` and does not already match raw.

Re-reports of the form "consistent-hash `extract_hash_key` Arg branch uses
`split('&')` / `split_once('=')` and compares the raw param name to `arg_name`
without percent-decoding", "`?session%20id=` / `?foo+bar=` fails to match a
spaced arg name", or "the hash key is silently empty and the request falls back
to round-robin" are **not accepted** — verify the current Arg branch carries the
`name.contains(['%', '+']) && HttpRouteRuleUnit::url_decode(name) == *arg_name`
guard before re-reporting.

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

`HttpRouteRuleUnit::url_decode` (exposed `pub(crate)` for this call; same helper
backing route-matching `parse_query_string`) is already exhaustively unit-tested
in `match_unit.rs` (`%20`→space, `+`→space, invalid `%XX` preserved literally,
multibyte UTF-8). The new branch is a thin wrapper over it, gated so the decode
runs only when needed. Backward compatible: a plain-identifier arg name still
matches raw via the `name == arg_name` fast path (including the case where an
operator previously worked around the bug by configuring the *encoded* form as
the arg name — that still matches raw and short-circuits before any decode).

## Fix Suggestions Not Accepted

- "Decode every param name unconditionally" — adds a per-request, per-param heap
  allocation on the data-plane hot path for an almost-never-triggered case; the
  `name.contains(['%', '+'])` guard makes the common path allocation-free and
  byte-for-byte equivalent to the old behavior.
- "Also percent-decode the param **value** for the hash key" — out of scope and a
  behavior change: it would alter the hash key (and therefore backend assignment)
  of every existing consistent-hash-by-arg deployment, not just the encoded-name
  edge case.
- "Just document that arg names must be raw-encoded / plain ASCII" (the issue's
  alternative) — the decode fix makes the natural config (`arg.session id`) work
  instead of pushing the encoding burden onto the operator.

## Re-evaluation Triggers

Re-open only if:

- `extract_hash_key`'s signature is refactored to receive the already-decoded
  query (e.g. via `ctx.cached_query()`), at which point the inline fast-path
  decode here becomes redundant and should be removed.

## Reference Cases

- backends-F014 (review finding, fixed 2026-05-24)
- `HttpRouteRuleUnit::url_decode` unit tests (`src/core/gateway/routes/http/match_unit.rs`)
- Existing integration scenario `HTTPRoute_default_lb-ch-arg.yaml` (`arg.user_id`,
  plain-identifier no-regression path)
