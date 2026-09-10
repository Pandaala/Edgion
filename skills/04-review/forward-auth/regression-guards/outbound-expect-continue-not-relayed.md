---
name: outbound-expect-continue-not-relayed
description: Use when reviewing plugin-created outbound HTTP requests that forward inbound headers; covers why `forwardAll` withholds the end-to-end `Expect` header.
status: fixed
finding_id: FA-BODY-002
closed: 2026-08-02
knowledge_type: regression-guards
---

# `forwardAll` Withholds `Expect` Even Though It Is End-to-End — FIXED

## Conclusion

`CompiledRequest::collect_headers` deliberately skips `Expect` in its
`forward_all` branch, alongside the body-framing headers. `Expect` is an
end-to-end header and RFC 9110 §7.6.1 hop-by-hop filtering correctly leaves it
alone, so an audit that only compares the skip list against the hop-by-hop set
will read this as an over-broad strip.

Re-reports of the form "`Expect` is not hop-by-hop, so `forward_all` must not
remove it" and "the skip belongs in `HOP_BY_HOP_HEADERS`" are **not accepted**.

## Core Rationale

**1. The expectation describes the inbound message, not the outbound one**

`Expect: 100-continue` is a statement about the request content the client is
about to send on *its* connection. A ForwardAuth or LinkSys webhook call is a
different message with an independently built body — usually none at all. The
expectation cannot be satisfied on the outbound exchange because there is no
content there to withhold or release.

**2. Relaying it converts into an auth denial**

The outbound client (reqwest/hyper) does not implement the client half of the
100-continue interaction, so the header goes out verbatim on a request with no
body. RFC 9110 §10.1.1 lets a server omit the interim response when framing
shows no content, so many servers cope — but a server that enforces the
expectation answers `417 Expectation Failed`, and ForwardAuth treats any
non-2xx as a denied request. The failure is client-triggerable without being
client-visible: curl adds `Expect: 100-continue` automatically for larger
request bodies.

**3. The inbound expectation is already settled elsewhere**

When a body consumer runs, `capture_request_body`
(`edgion-gateway/src/plugins/runtime/session_adapter.rs`) writes the
`100 Continue` response itself and then removes `EXPECT` from the inbound
header, precisely so the expectation is not answered twice. Without this skip
the two ForwardAuth modes disagree: `forwardBody: true` sends no `Expect`
(capture stripped it) while the default `forwardBody: false` relays it.

**4. `request_mirror` already established this behavior**

`RequestMirror::build_headers`
(`edgion-gateway/src/plugins/http/request_mirror/plugin.rs`) has stripped
`expect` inline, next to its hop-by-hop filter, since before this finding. The
fix aligns the shared outbound builder with the precedent rather than inventing
a rule. AllEndpointStatus later adopted the same inbound-body skip on its
independent builder; the three spellings remain deliberate duplication — see
"Independent-builder consumer".

**5. Same family as the framing decision, different mechanism**

`FA-BODY-001` ([outbound-subrequest-framing-owned-by-hop.md](outbound-subrequest-framing-owned-by-hop.md))
rejects `Content-Length`/`Transfer-Encoding` at config time *and* strips them at
runtime. `Expect` is only skipped under `forward_all`; naming it in `origin`
stays legal and forwards it, because unlike framing it cannot desynchronize the
connection — it is at worst a wasted expectation.

## Fix Suggestions Not Accepted

- Add `expect` to `HOP_BY_HOP_HEADERS` — that constant is the literal RFC 9110 §7.6.1 set and `is_hop_by_hop` answers a factual question about it, so adding an end-to-end header makes it return a wrong answer. It would also silently change every other consumer of the set, including `all_endpoint_status`, rather than only the path where the decision is justified.
- Fold the check into `is_body_framing_header` — `Expect` is not framing, and that predicate's name is load-bearing for the schema-side rejection rationale.
- Reject `Expect` in `origin`/`custom` like the framing headers — an explicit listing is a deliberate operator act with no desync risk.
- Have the outbound builder answer or honour the expectation itself — the outbound body is fully buffered before dispatch, so there is nothing to negotiate.

## Independent-builder consumer — AllEndpointStatus

`AllEndpointStatus::fan_out_requests`
(`edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs`) builds its own
bodyless `reqwest` calls and does not go through `CompiledRequest::collect_headers`.
It already applies hop-by-hop plus `Host` filtering, and separately skips inbound
body headers (`Content-Length`, `Expect`) via `is_inbound_body_header`.
`Transfer-Encoding` is hop-by-hop, so it is not relayed either. Re-reports that
AllEndpointStatus still forwards inbound `Content-Length`, `Transfer-Encoding`,
or `Expect` are **not accepted**.

The three sites still spell the Expect skip independently (`collect_headers`,
`request_mirror`'s inline check, and `is_inbound_body_header`). Extracting one
shared predicate is optional and is not required to keep this finding closed.

## Re-evaluation Triggers

Re-open this decision only if:

- The shared outbound path gains a streaming request body whose bytes are produced after the headers are sent, making a real 100-continue negotiation useful.
- The outbound HTTP client starts implementing the client half of 100-continue.

## Reference Cases

- Historical provenance: `FA-BODY-002` was split from the now-removed
  `forward-auth-content-length-leak` task; the `Content-Length` half is
  `FA-BODY-001`, maintained in
  [the outbound framing guard](outbound-subrequest-framing-owned-by-hop.md).
- `edgion-gateway/src/plugins/http/common/outbound/request.rs` — `is_hop_owned_expectation_header`, unit tests `forward_all_strips_expect_continue` and `explicitly_listed_origin_expect_is_still_forwarded`.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs` — `capture_request_body` answers and strips the inbound expectation.
- `edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs` — independent builder; `is_inbound_body_header` plus unit test `inbound_body_headers_are_content_length_and_expect`.
- ForwardAuth user guides: [English](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/forward-auth.md)
  and [Chinese](../../../../docs/zh-CN/user-guide/http-route/filters/edgion-plugins/forward-auth.md).

## Resolution references

- `FA-BODY-002` — 2026-08-02.
