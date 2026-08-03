---
name: strip-query-param-no-match-preserves-query
description: Use when reviewing whether auth query credential hiding may rewrite a query that no longer contains the credential parameter.
status: fixed
finding_id: change-fix-07
closed: 2026-08-02
---

# Auth Query Stripping Preserves an Unmatched Query — FIXED

## Conclusion

`auth_common::strip_query_param` now calls `set_request_query` only when it
actually removes at least one matching parameter. If the live query no longer
contains the credential key, the request target remains byte-identical.

## Core Rationale

**1. Extraction and stripping can observe different query snapshots**

Auth extraction reads the frozen entry query through `get_query_param`, while
credential stripping reads the live query through `get_query`. An earlier
request plugin can therefore remove or rewrite the credential parameter before
the auth plugin reaches its hide-credentials step.

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
- Change auth extraction to read the live query — extraction intentionally uses
  the frozen entry query so plugin reads remain order-independent.

## Re-evaluation Triggers

Re-open this decision only if query extraction and mutation move to one atomic
API, or `set_request_query` gains a byte-identical no-op guarantee for every
query representation.

## Reference Cases

- `change-fix-07` (2026-08-02) — fixed with a shared-helper unit regression.
- `skills/04-review/key-auth/hide-credentials-query-cookie-strip.md`
- `skills/04-review/jwt-auth/hide-credentials-query-cookie-strip.md`
