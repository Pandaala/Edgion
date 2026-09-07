---
name: outbound-forward-all-preserves-duplicate-headers
description: Fixed — shared plugin outbound requests preserve repeated incoming header values when headers.forwardAll is enabled, while explicit origin/custom entries retain replace semantics.
status: fixed
finding_id: gw-plugin-outbound-02
closed: 2026-08-02
knowledge_type: regression-guards
---

# Outbound `forwardAll` preserves duplicate request headers (gw-plugin-outbound-02) — FIXED

## Conclusion

`CompiledRequest::collect_headers` now returns a `reqwest::header::HeaderMap`.
Incoming values selected by `headers.forwardAll` are appended, so repeated field
lines with the same case-insensitive name survive into ForwardAuth and webhook
subrequests. Explicit `origin` and `custom` fields still use replace semantics
and therefore override every forwarded value with the same name.

Re-reports that `headers.forwardAll` collapses duplicate incoming values through
an intermediate `HashMap<String, String>` are not accepted; verify the current
`HeaderMap::append` path before re-reporting.

## Core Rationale

The old collector stored forwarded values in `HashMap<String, String>`, making
the final occurrence silently replace all earlier occurrences. The subsequent
`HeaderMap::insert` during request assembly reinforced the same last-value-only
behavior. This contradicted the configured `forwardAll` contract and could make
an external authorization service observe a different header set from the
incoming request.

Using the outbound request's native multi-value header representation removes
the lossy intermediate shape. Append is correct for transparent forwarding;
insert remains correct for explicit operator overrides.

## Fix Suggestions Not Accepted

- Comma-joining repeated values is not a general replacement for separate field
  lines because not every header has list syntax.
- Changing only the final request assembly cannot recover values already lost by
  the old `HashMap<String, String>` collector.
- Appending explicit `origin` or `custom` entries would change their documented
  override behavior and is outside this fix.

## Re-evaluation Triggers

- The outbound request builder stops using `reqwest::header::HeaderMap`.
- Explicit header fields gain a new append operation in the resource schema.
- The session header snapshot no longer yields every repeated field value.

## Reference Cases

- Finding: `gw-plugin-outbound-02` (2026-08-02).
- Fix: `edgion-gateway/src/plugins/http/common/outbound/request.rs`.
- Regression: `forward_all_preserves_duplicate_header_values`.
- Related precedent: `append-request-header-multi-value.md` (F-001) and
  `wasm-set-header-map-pairs-replace-all.md` (runtime-03).

## Resolution references

- `gw-plugin-outbound-02` — 2026-08-02.
