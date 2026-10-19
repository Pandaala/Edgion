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

`auth_common::strip_query_param` now calls `set_request_query` only when it
actually removes at least one matching parameter. If the live query no longer
contains the credential key, the request target remains byte-identical.

## Core Rationale

**1. Extraction and stripping can observe different query snapshots**

Auth extraction reads the frozen entry query through `get_query_param`, while
credential stripping reads the live query through `request_query`. An earlier
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
- `skills/04-review/key-auth/regression-guards/hide-credentials-query-cookie-strip.md`
- `skills/04-review/jwt-auth/regression-guards/hide-credentials-query-cookie-strip.md`

## Resolution references

- `change-fix-07` — 2026-08-02.

## Borrow the live query while planning replacement

The shared helper borrows the current raw query through `request_query` instead
of cloning it through `get_query`. The real adapter reads the same live URI for
both accessors. Surviving slices are joined into an owned replacement before the
mutable setter runs, so no borrow crosses mutation. Frozen entry extraction,
percent-decoded key matching, matched empty-pair removal and ignored setter errors
retain their existing behavior.

The helper regression matrix forbids the owned accessor and covers absent/empty
queries, unmatched separators, encoded/duplicate keys, raw surviving bytes and
setter failures. Real-adapter regressions preserve absolute request targets and
HTTP/2 authority; the live-query accessor regression checks pointer identity,
zero read allocations and parity with the owned getter across URI changes.
Only the whole-query copy is removed. Key decoding, the temporary slice Vec and
the joined replacement still allocate; no whole-helper zero-allocation or
throughput claim follows.
