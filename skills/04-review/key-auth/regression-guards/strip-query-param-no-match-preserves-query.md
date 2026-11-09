---
name: strip-query-param-no-match-preserves-query
description: Use when reviewing whether auth query credential hiding may rewrite a query that no longer contains the credential parameter.
status: fixed
finding_id: change-fix-07
closed: 2026-08-02
knowledge_type: regression-guards
---

# Auth Query Stripping Preserves an Unmatched Query — FIXED

## Conclusion

`auth_common::strip_query_param` delegates structured removal to the session
Query owner; it changes the logical query only when a matching parameter exists. If the live query no longer
contains the credential key, the request target remains byte-identical.

## Core Rationale

**1. Extraction and stripping share the logical current Query**

Ordinary extraction uses `get_query_param`, which reads the current Query owner.
A successful earlier removal is immediately visible to later authentication and
outbound consumers. No-match deletion remains safe and preserves raw syntax.
The historical frozen-extraction rule is superseded by this read-after-edit contract.

**2. A no-op must not normalize unrelated query syntax**

The stripping path intentionally drops empty query pairs while rebuilding a
query that contained the credential. Before this fix, the same reconstruction
also ran without a match, changing forms such as `a=1&&b=2&` even though there
was no credential left to remove. Tracking whether a decoded key matched keeps
the no-match path free of side effects while preserving the established matched
path.

## Fix Suggestions Not Accepted

- Always rebuild the query for implementation simplicity — this changes raw
  query bytes when no credential was removed.
- Decode and re-encode all surviving pairs — this would normalize unrelated
  operator-supplied encoding and can invalidate raw-query signatures.
- Fall back to entry credentials after a current lookup misses — that would
  re-expose a value removed by an earlier plugin.

## Re-evaluation Triggers

Re-open this decision only if query extraction and mutation move to one atomic
API, or `set_request_query` gains a byte-identical no-op guarantee for every
query representation.

## Reference Cases

- `change-fix-07` (2026-08-02) — fixed with a shared-helper unit regression.
- `skills/04-review/key-auth/regression-guards/hide-credentials-query-cookie-strip.md`
- `skills/04-review/jwt-auth/regression-guards/hide-credentials-query-cookie-strip.md`

## Resolution references

- `change-fix-07` — 2026-08-02.

## Shared lazy Query owner

The session structured-removal operation now owns the split/decode/selection work.
The auth helper no longer creates a survivor vector and immediately joins it into
a setter input. Current raw reads and ordinary single-key extraction use the same current owner. A no-match operation keeps the original raw syntax and
revision; a match removes all decoded-equivalent names, including bare flags, and
drops empty separators only as part of the changed selection.

Surviving raw ranges and decoded cells stay in the backing document. Consecutive
removals do not create a history chain or eagerly render; raw demand renders the
current selection once. Protected transcoder requests reject mutation before
publication. Since 2026-11-09 `strip_query_param` returns the removal result and a
rejected removal denies the request (KeyAuth invalid-key 401, JwtAuth `invalid_token`
401, tag `credential removal failed`, diagnostic `AuthHeaderMutationRejected`) instead
of ignoring the error; a no-match on a protected request still returns `Ok(false)` and
continues. Because a client can send the credential parameter to a transcoded route,
this diagnostic is client-triggerable and is alerted on by rate, not per event. The eventual concrete-header barrier preserves target form and fails
locally rather than forwarding stale credentials.

Names without `%` or `+` compare as borrowed raw spans. Encoded names always use
canonical decoding, even if their raw spelling equals the configured auth name;
LB's raw-equality exemption is deliberately different. Reuse addresses auth-01's
Query survivor work and uri-01's repeated encoded-name work, not Cookie storage or
a promise of zero first-use allocation. Verify shared-auth wiring and real-adapter
semantics alongside owner tests; mock setter calls alone no longer prove transport
publication. Performance costs are separately measured and disclosed.
