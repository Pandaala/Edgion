---
name: abandoned-request-body-terminal-event
description: Use when reviewing request-body hooks after an upstream stops receiving; distinguishes a real downstream EOF from a proxy-abandoned body prefix.
status: fixed
finding_id: pingora-review-18
closed: 2026-08-11
knowledge_type: regression-guards
---

# Abandoned Request Bodies Have a Distinct Terminal Event — FIXED

## Conclusion

Pingora request-body hooks now receive `RequestBodyEvent::Data`, `Complete`, or
`Abandoned`. Only `Complete` means the downstream transport's real EOF was
observed; `Abandoned` is terminal but marks all previously delivered bytes as
an incomplete prefix. Edgion cancels ExtProc `STREAMED` and RequestMirror work
on `Abandoned` instead of publishing a clean end-of-stream.

If Pingora's response pipeline selected a final response before delivering
`Abandoned`, Edgion performs the same incomplete-work cleanup and returns
`RequestBodyAction::Terminate` without attempting a local WAF/handler
replacement. At call sites that independently prove the selected response
complete, Pingora interprets that action as request-side termination and keeps
draining the response. Selection alone is not completion proof: writer-error
abandonment or nonterminal termination explicitly aborts downstream without a
clean response terminator. Edgion suppresses a local replacement for any
pipeline-selected response, not only for `Abandoned`. This ownership contract
is recorded in the fork's
`edgion-changes/review/h2-grpc/h2-abandonment-terminate-preserves-selected-response.md`.

Fix suggestions that retain `end_of_stream: bool` and expose a separate
session flag are **not accepted**.

## Core Rationale

**1. Terminal delivery and body completeness are independent facts**

The H1/H2 futile-read branches stop polling a declared-empty downstream after
the upstream response is complete. The H2 upstream-stop branch also stops
polling after an origin finishes its response and refuses a mid-upload DATA
write. Both owe filters one final notification, but neither observed the real
downstream EOF. Encoding both as `end_of_stream=true` made a prefix
indistinguishable from a complete body.

The event terminates one delivery sequence. Pingora retry attempts may replay
buffered body events through the same hooks; consumers that model the original
downstream body may treat its first terminal event as final, as they did with
the former Boolean API.

**2. The event belongs in the public hook contract**

The same enum is delivered to `ProxyHttp` and `HttpModule`; no consumer must
inspect hidden session state or depend on callback ordering. Normal H1, H2,
custom, and subrequest paths produce `Data`/`Complete`, while only explicit
proxy abandonment sites produce `Abandoned`. Trailer filters remain gated on
`Complete` because abandonment establishes no trailer-presence fact.

**3. Edgion consumers fail incomplete work visibly**

ExtProc drops its request-side `Process` stream without sending
`HttpBody.end_of_stream=true`. RequestMirror signals its owner task to cancel
the in-flight HTTP send future, so a backend cannot accept a clean chunked EOF
for the prefix. Its cancellation guard is armed by default and is disarmed on
exactly three facts: the downstream body reached its real EOF; the mirror owner
task has already determined its own result (`TrySendError::Closed`), which must
survive rather than be masked; or the transport ended on the request headers
(`PluginSession::request_headers_end_stream() == Some(true)`), in which case
streaming state is never installed because zero delivered bytes *are* the
complete mirror body. Every other early return fails closed; `Some(false)` and
`None` stay conservative and keep the armed path. The guard is the
first field dropped from streaming state so cancellation precedes body-channel
EOF. The access log and `edgion_mirror_requests_total` use deterministic
`body_abandoned` or `channel_full` outcomes.

## Fix Suggestions Not Accepted

- Add `Session::request_body_abandoned()` beside the Boolean hook argument —
  leaves `HttpModule` blind and turns an event fact into a timing-sensitive
  side channel.
- Omit the terminal callback on abandonment — leaks per-delivery filter state
  and leaves consumers without a completion cause.
- Close ExtProc or RequestMirror normally — repeats the original defect at the
  application boundary by certifying a prefix as complete.
- Disarm the mirror guard whenever no body event was ever delivered — a request
  that *did* declare a body and was short-circuited before the pump ran has
  delivered zero of N bytes, which is a truncated prefix; disarming there
  reproduces the original defect at the mirror target. Only a transport-level
  EOS proves that a zero-byte body is complete.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora replaces per-event request-body hooks with a different typed stream
  abstraction that carries completion cause directly.
- ExtProc adds an explicit protocol-level request-body cancellation message;
  Edgion may then send it before dropping the stream.

## Reference Cases

- Pingora `Abandoned` delivery sites (the fix for review issue 18; prefer the
  named function/branch over a line number):
  `pingora-proxy/src/proxy_h1.rs` — the `downstream_body_read_is_futile` branch
  of `proxy_handle_downstream`, and that loop's `tx.reserve()` arm on
  `tx.is_closed()`, the H1 counterpart of `finish_downstream_body_side`: the
  upstream half returned and closed the request-body pipe while the downstream
  body was only partially read.
  `pingora-proxy/src/proxy_h2.rs` — the `downstream_body_read_is_futile` branch
  of `bidirection_down_to_up`, and `finish_downstream_body_side`.
- Pingora event enum: `RequestBodyEvent` in
  `pingora-core/src/modules/http/mod.rs`
- Pingora regression:
  `pingora-proxy/tests/test_h2_upstream_no_error_reset.rs`
- Edgion consumers:
  `edgion-gateway/src/plugins/http/ext_proc/streamed.rs` and
  `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs`

## Resolution references

- `pingora-review-18` — 2026-08-11; ownership correction 2026-09-01.
