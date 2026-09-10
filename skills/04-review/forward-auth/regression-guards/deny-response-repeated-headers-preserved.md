---
name: deny-response-repeated-headers-preserved
description: Use when reviewing whether ForwardAuth deny responses preserve repeated configured client headers such as WWW-Authenticate and Set-Cookie.
knowledge_type: regression-guards
---

# ForwardAuth Deny Responses Preserve Repeated Headers

## Conclusion

`ForwardAuth::send_error_response` preserves every valid text value of each
operator-selected `decision.clientHeaders` name. The first retained value uses
`ResponseHeader::insert_header` to replace any existing values, and later values
use `append_header` so repeatable fields remain separate and ordered.

Case-equivalent duplicate names in `decision.clientHeaders` select the source
field only once. Duplicate source values are not deduplicated: distinct
`WWW-Authenticate` challenges and `Set-Cookie` field lines carry independent
semantics and must reach the client unchanged rather than being comma-joined.

## Root Cause and Fix

The previous implementation selected each configured name with `HeaderMap::get`
and wrote it with `insert_header`. `get` exposed only one value, so later auth
service response values were discarded before the downstream response was built.

The corrected path iterates `HeaderMap::get_all` in source order, filters values
through the existing `HeaderValue::to_str` boundary, and applies set-first then
append-rest semantics directly to the real Pingora `ResponseHeader`. Failed
insert or append operations add the existing `invalid response header` PluginLog
entry. A failed write does not advance the set/append state; values successfully
written before a later append failure remain present.

## Regression Coverage

`deny_copies_client_headers_and_shortcircuits` exercises the private
`handle_outbound_response` path with `MockPluginSession` and inspects the actual
`ResponseHeader` supplied to `write_response_header`. It verifies ordered
multi-value `WWW-Authenticate`, separate `Set-Cookie` fields, case-insensitive
configured-name deduplication, invalid-text filtering, unselected exclusion,
status and content type, zero-copy body ownership, end-of-stream, and shutdown.

## Scope and Re-evaluation

This guard covers only non-success ForwardAuth responses and only names selected
by `decision.clientHeaders`. The success-side `decision.upstreamHeaders` path
retains its existing first-value request-header replacement semantics. Re-evaluate
if the decision schema gains explicit set/append operations or the deny path no
longer writes a Pingora `ResponseHeader` directly.

## Resolution References

- B0290 — ForwardAuth deny-response repeated-header preservation.
