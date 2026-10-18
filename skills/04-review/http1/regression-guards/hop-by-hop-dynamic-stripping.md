---
name: hop-by-hop-dynamic-stripping
description: Fixed — plugin sub-requests and main proxied responses strip RFC 9110 §7.6.1 static and dynamic Connection-listed fields at their forwarding boundaries.
knowledge_type: regression-guards
---

# http1-03 — Hop-by-hop filtering at forwarding boundaries

**Status:** ✅ fixed · plugin sub-requests 2026-05-19 · main response 2026-09-20
**RFC:** RFC 9110 §7.6.1 (MUST)

## Conclusion

Plugin sub-request callers strip the canonical `HOP_BY_HOP_HEADERS` array and
header names dynamically nominated by the originating request's `Connection:`
value. The main proxied response path applies the same canonical static list and
token parser to every repeated response `Connection` field before forwarding.
Both directions implement RFC 9110 §7.6.1 at their owned forwarding boundary.

The response sanitizer additionally strips legacy `Proxy-Connection` on ordinary
responses; 101 still retains its handshake metadata. This response-only addition
does not expand the shared request list. Ordinary response producers are separately
constrained by the [response mutation policy](../../plugin-runtime/regression-guards/response-header-mutation-policy.md).

Suggestions that either path checks only the static eight-entry list are not
accepted. Re-evaluate a report against the exact forwarding site and repeated
field handling described below.

## RFC clause

> "Intermediaries MUST parse a received Connection header field before a
> message is forwarded and, for each connection-option in this field, remove
> any header field(s) from the message with the same name as the
> connection-option, and then remove the Connection header field itself (or
> replace it with the intermediary's own connection options for the forwarded
> message)." — RFC 9110 §7.6.1

## Scope

This rule covers two distinct forwarding surfaces:

- plugin-generated outbound sub-requests derived from a downstream request;
- the main proxied response header forwarded to the downstream client.

The helper is shared tokenization policy. Each forwarding owner remains
responsible for collecting all field lines available at its boundary and
removing the nominated fields before forwarding.

## Code shape today

- `edgion-gateway/src/plugins/http/common/http_client.rs`
  - `HOP_BY_HOP_HEADERS` — canonical 8-entry set (doc-comment now cites
    RFC 9110 §7.6.1).
  - `is_hop_by_hop(name)` — unchanged signature; consults the static set only.
  - `connection_listed_hop_by_hop(connection_value: &str) -> Vec<String>` —
    parses the request's `Connection:` value into a deduplicated lowercase
    token list. Empty tokens skipped; case-folded.
  - `connection_option_tokens` — canonical borrowed split/trim/nonempty
    iterator used by the owned-list helper and consumers that do not need an
    intermediate owned list. It preserves original case and repeated options.
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
    fan-out, borrowed by each locally awaited endpoint future). Also skips `Host` and inbound
    body headers (`Content-Length`, `Expect`) via `is_inbound_body_header`;
    `Transfer-Encoding` is already hop-by-hop.
  - `link_sys/providers/webhook/resolver.rs::resolve_webhook_key`
    (operator-named `forward_headers` whitelist).

- `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`
  - `sanitize_forwarded_response_headers` inspects every repeated response
    `Connection` line before mutation, uses `connection_option_tokens`, then
    validates each nomination as an RFC `tchar` header name. It deduplicates
    borrowed options case-insensitively before allocating retained `HeaderName`
    values; `HeaderName` normalizes their case. The retained list is removed
    only after all fields have been inspected.
  - The first non-text `Connection` value fails closed without inspecting the
    remaining suffix. No header mutation has happened, so the operation never
    leaves a partially sanitized map. Successful mutation still requires all
    repeated fields to validate. The former full scan after a definitive failure
    was unnecessary for atomicity and could allocate unused nominations. Invalid
    ASCII nomination tokens are ignored while valid sibling tokens are honored.
  - Interim 100/102/103 responses are sanitized before their early return. A
    101 upgrade response bypasses sanitization and keeps its handshake fields.
  - Ordinary final responses are sanitized before async response plugins and
    again afterward. Both passes are skipped once a response-stage denial or
    local reply owns the outcome, preserving that settled response's error and
    delivery precedence.
  - Whether either pass observed `Transfer-Encoding` is retained for
    compression eligibility after the field itself is removed; Pingora owns
    downstream framing.

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

## Known plugin-sub-request gap: multiple `Connection:` header lines

`session.header_value("connection")` and the equivalent
`original_headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("connection"))`
in `all_endpoint_status::fan_out_requests` both return only the **first**
occurrence of the header. RFC 9110 §5.3 explicitly allows list-based fields
to appear as multiple header lines semantically equivalent to one
comma-joined value. A client sending two `Connection:` lines (with a custom
name in the second line) would currently slip the second-line tokens past
dynamic stripping.

This gap does not apply to the main response sanitizer, which reads every
repeated field line. Closing it for the listed plugin sub-request callers
requires changes beyond `connection_listed_hop_by_hop`
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
  multi-line request `Connection:` headers in a plugin sub-request — escalate
  the "Known plugin-sub-request gap" above into a follow-up finding and extend
  the session trait with a `header_values` accessor.
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
- `B0061` main response boundary — 2026-09-20.
- `unify-response-header-mutation-policy` — response-only `Proxy-Connection` cleanup;
  current verification is recorded in the shared policy page.

## Response nomination temporary storage

The response sanitizer retains up to two owned HeaderNames inline in a SmallVec;
larger unique lists spill without a new limit. Token validation, case-insensitive
deduplication and complete-field validation still precede every header mutation.
This trades two inline slots of stack storage for removal of the collection
allocation on small lists; custom header-name construction may still allocate.
A prepared two-standard-name fixture checks zero sanitizer allocations, while
three/eight-name cases verify spill removal and atomic rejection of a final
opaque field. Existing repeated-name allocation parity, invalid ASCII, 101 and
both-boundary tests remain. No general zero-allocation or throughput claim follows.
