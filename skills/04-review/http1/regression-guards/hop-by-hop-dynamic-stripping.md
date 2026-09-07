---
name: hop-by-hop-dynamic-stripping
description: Fixed — plugin sub-request hop-by-hop filtering now honours RFC 9110 §7.6.1 dynamic Connection-listed names in addition to the canonical set. Reject re-reports claiming the list is static-only.
knowledge_type: regression-guards
---

# http1-03 — Hop-by-hop filtering at plugin sub-requests (static set + dynamic Connection-listed)

**Status:** ✅ fixed · 2026-05-19
**RFC:** RFC 9110 §7.6.1 (MUST)

## Conclusion

Plugin sub-request callers that previously consulted only the canonical
`HOP_BY_HOP_HEADERS` array now also strip header names dynamically nominated by
the originating request's `Connection:` header value, per RFC 9110 §7.6.1.
The wrong RFC citations (RFC 2616 §13.5.1, RFC 7230 §6.1) in the doc-comment
have been replaced with RFC 9110 §7.6.1.

Suggestions of the form "`is_hop_by_hop` only checks a static 8-entry list — a
`Connection: X-Custom-Header` value lets a downstream client smuggle headers
to forward_auth / mirror / status / webhook upstreams" are **not accepted** —
the fix is in place.

## RFC clause

> "Intermediaries MUST parse a received Connection header field before a
> message is forwarded and, for each connection-option in this field, remove
> any header field(s) from the message with the same name as the
> connection-option, and then remove the Connection header field itself (or
> replace it with the intermediary's own connection options for the forwarded
> message)." — RFC 9110 §7.6.1

## Scope

Only the *plugin sub-request* paths are affected. The main Pingora proxy
forwarding pipeline strips hop-by-hop independently of this helper and was
never affected by the gap.

## Code shape today

- `edgion-gateway/src/plugins/http/common/http_client.rs`
  - `HOP_BY_HOP_HEADERS` — canonical 8-entry set (doc-comment now cites
    RFC 9110 §7.6.1).
  - `is_hop_by_hop(name)` — unchanged signature; consults the static set only.
  - `connection_listed_hop_by_hop(connection_value: &str) -> Vec<String>` —
    parses the request's `Connection:` value into a deduplicated lowercase
    token list. Empty tokens skipped; case-folded.
  - `is_hop_by_hop_or_listed(name, dynamic_listed)` — combined check used at
    every sub-request forwarding site.
  - Unit tests `test_connection_listed_hop_by_hop_parses_tokens` and
    `test_is_hop_by_hop_or_listed_combines_static_and_dynamic` lock in token
    parsing and the combined filter.

- Call sites that build sub-request `HeaderMap`s now derive
  `dynamic_listed` once per request and consult `is_hop_by_hop_or_listed`:
  - `forward_auth/plugin.rs::forward_all_headers` (Mode 1 — forward all).
  - `request_mirror/plugin.rs::build_headers`.
  - `all_endpoint_status/plugin.rs::fan_out_requests` (derived once before
    fan-out, cloned into each endpoint future). Also skips `Host` and inbound
    body headers (`Content-Length`, `Expect`) via `is_inbound_body_header`;
    `Transfer-Encoding` is already hop-by-hop.
  - `link_sys/providers/webhook/resolver.rs::resolve_webhook_key`
    (operator-named `forward_headers` whitelist).

## What is intentionally left as-is

- `forward_auth` **Mode 2** (operator-specified `request_headers` whitelist)
  still does not run any hop-by-hop filtering (neither static nor dynamic).
  The operator has explicitly named the headers to forward; expanding the
  dynamic-strip semantic into that mode would silently override operator
  intent. Re-reports targeting Mode 2 should be filed as a separate finding
  with operator-impact analysis.
- Static `is_hop_by_hop(name)` is **kept as a `pub` re-export** for any
  consumer that already understands the dynamic-vs-static distinction and
  only wants the canonical check.

## Known inconsistency (not introduced by this fix)

`webhook::resolver::resolve_webhook_key` and `forward_auth` Mode 2 are
structurally similar — both forward only operator-listed header names — but
they treat hop-by-hop differently:

| Site | Static hop-by-hop filter | Dynamic Connection-listed filter |
|---|---|---|
| `forward_auth` Mode 2 (`request_headers`) | no | no |
| `webhook::resolver` (`forward_headers`) | yes (already pre-fix) | yes (added by this fix) |

The webhook resolver's static filter pre-dates this fix; this fix only
extends it to the dynamic dimension to stay consistent with the static
behavior already present there. Reconciling the two sites is out of scope
for http1-03 and should be opened as a separate finding with explicit
operator-impact analysis (changing Mode 2 in either direction is a
behavior change for existing deployments).

## Known gap: multiple `Connection:` header lines

`session.header_value("connection")` and the equivalent
`original_headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("connection"))`
in `all_endpoint_status::fan_out_requests` both return only the **first**
occurrence of the header. RFC 9110 §5.3 explicitly allows list-based fields
to appear as multiple header lines semantically equivalent to one
comma-joined value. A client sending two `Connection:` lines (with a custom
name in the second line) would currently slip the second-line tokens past
dynamic stripping.

Closing this gap requires changes beyond `connection_listed_hop_by_hop`
itself: either iterate `request_headers()` and collect every `connection`
entry, or extend the `PluginSession` trait with a
`header_values(name) -> Vec<String>` accessor (single-value `header_value`
is insufficient). The fix is intentionally deferred — multi-line
`Connection:` is rare on the wire, and the current fix already closes the
common-case attack surface (single `Connection: X-Custom`).

## Re-evaluation triggers

- A new plugin sub-request path is added that builds an upstream
  `HeaderMap`/`reqwest::Request` from session headers — it must consult
  `is_hop_by_hop_or_listed`, not the old `is_hop_by_hop`.
- A real-world attack or operator report demonstrates exploitation via
  multi-line `Connection:` headers — escalate the "Known gap" above into a
  follow-up finding and extend the session trait with a `header_values`
  accessor.
- The discrepancy between `forward_auth` Mode 2 and `webhook::resolver`
  starts mattering operationally — reopen the "Known inconsistency" section
  as a standalone design decision.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a1.md → V5` (CONFIRM)

## Sibling rules — end-to-end headers withheld on purpose

The `forward_all` branch of `CompiledRequest::collect_headers` skips more than
the hop-by-hop set. Do not report those extra skips as over-broad stripping:

- `Content-Length` / `Transfer-Encoding` —
  [../forward-auth/outbound-subrequest-framing-owned-by-hop.md](../../forward-auth/regression-guards/outbound-subrequest-framing-owned-by-hop.md)
  (FA-BODY-001); also rejected at config time in `origin`/`custom`.
- `Expect` —
  [../forward-auth/outbound-expect-continue-not-relayed.md](../../forward-auth/regression-guards/outbound-expect-continue-not-relayed.md)
  (FA-BODY-002); an explicit `origin` listing still forwards it.

Both are end-to-end headers, so hop-by-hop filtering correctly leaves them
alone; they are withheld separately because they describe the inbound message
rather than the outbound one being built.

Scope: these two skips live in `collect_headers` and therefore cover ForwardAuth
and the LinkSys webhook. `request_mirror` strips `expect` with its own inline
check. `all_endpoint_status` builds outbound requests independently and applies
the same inbound-body skip via `is_inbound_body_header` (`Content-Length` and
`Expect`; `Transfer-Encoding` is already hop-by-hop). Re-reports that
AllEndpointStatus still relays those headers are **not accepted**.

## Resolution references

- `http1-03` — 2026-05-19.
