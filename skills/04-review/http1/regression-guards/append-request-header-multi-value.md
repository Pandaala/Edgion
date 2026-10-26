---
name: append-request-header-multi-value
description: Read when a finding claims append_request_header only reads the first existing value, silently drops additional same-name request header entries, or collapses a multi-value request header to first-value,appended.
knowledge_type: regression-guards
---

# append_request_header drops additional same-name request header values (F-001)

**Status / verdict**: fixed (F-001, 2026-05-20)

When reviewing findings of the form "`append_request_header` only reads the first existing value / silently drops additional same-name request header entries / multi-value request header collapsed to `first-value,appended` (F-001)": **fixed (F-001, 2026-05-20)**. `PingoraSessionAdapter::append_request_header` (`edgion-gateway/src/plugins/runtime/session_adapter.rs`) now reaches `req_header_mut().append_header(name, value)` through the private append writer — adds a new header field line preserving every existing same-name value per RFC 7230 §3.2.2 — instead of the old read-`headers.get` (first value only)-then-`insert_header` (replaces all) pattern. Known callers inheriting the fix include Gateway API `RequestHeaderModifier.add`, `proxy_rewrite` add, and DSL `req.append_header` (the DSL api-reference already documented "appends a new same-name entry without overwriting" — the code now matches the contract). Conformance-safe: the Gateway API request-header-modifier suite joins received values with `,`, so both the old single-line and new multi-line forms compare equal. Integration regression: `request_header_append_preserves_multi_value` in `edgion-tests/src/client/suites/http_route/filters/header_modifier.rs` (route `/multi-value-append-test` sends two same-name values, route adds a third, asserts all three survive). Re-reports are not accepted; verify the current append call chain uses `append_header` before re-reporting.

## Resolution references

- `F-001` — 2026-05-20.


## Guarded append handoff

String and typed request append entries each retain the live sealed-RPC guard,
then share a private post-validation writer. The string entry checks protection
before parsing HeaderValue, so protected malformed values still return the sealed
RPC error and record the sticky owner conflict. Ordinary malformed values still
fail before name insertion; rejected writes leave existing field lines unchanged.
The writer appends the owned typed value and clears a matching deferred credential
only after successful insertion. Opaque bytes and sensitivity remain preserved.
The helper has no callers outside those two guarded entries. This removes a
repeated optional-plan check, plus a repeated protected-name scan when a plan is
active, for successful string appends; it makes no throughput claim.
