---
name: plugin-session-shutdown-h2-no-gate
description: Use when reviewing findings that PingoraSessionAdapter::shutdown() lacks the is_http2() gate that error_response.rs has, or that plugin deny paths abruptly close HTTP/2 connections. Pingora's H2 shutdown never touches TCP; the divergence is intentional.
status: false-positive
finding_id: gw-plugin-acl-rl-01
closed: 2026-08-03
---

# gw-plugin-acl-rl-01 — `PluginSession::shutdown()` needs no `is_http2()` gate

**Status:** ❌ false positive · 2026-08-03 · comment-only clarification landed, no behavior change

## Conclusion

`PingoraSessionAdapter::shutdown()` (`edgion-gateway/src/plugins/runtime/session_adapter.rs`)
skips the raw `Session::shutdown()` only for gRPC and lets plain HTTP/2 fall through. Its
sibling `shutdown_after_error_response` (`edgion-gateway/src/runtime/server/error_response.rs`)
gates on `!session.is_http2()`. **The divergence is intentional and harmless** — the two gates
are not guarding the same hazard, and neither is load-bearing on its normal path.

Fix suggestions of the form "mirror `error_response.rs` — add `|| self.inner.is_http2()` to
`PingoraSessionAdapter::shutdown()`" are **not accepted**: they fix nothing and make the one
branch that has real behavior strictly worse.

## Core Rationale

**1. Pingora's HTTP/2 `shutdown()` never performs a TCP-level close.**

The reported harm ("abrupt TCP-level close on HTTP/2 → transient EOF for other connections
being accepted on the same listener port") does not exist for H2. `HttpSession::shutdown`
dispatches per protocol (`pingora-core/src/protocols/http/server.rs`):

```rust
Self::H1(s) => s.shutdown().await,   // -> underlying_stream.shutdown(): a real TCP close
Self::H2(s) => s.shutdown(),         // see below
```

and the H2 arm (`pingora-core/src/protocols/http/v2/server.rs`) is stream-scoped only:

```rust
pub fn shutdown(&mut self) {
    if !self.ended {
        self.send_response.send_reset(h2::Reason::INTERNAL_ERROR);
    }
}
```

Worst case it emits one `RST_STREAM` on the offending stream. It cannot affect the TCP
connection, let alone unrelated connections on the listener.

**2. On every real caller path the H2 call is already a no-op.**

`ended` is set by `do_write_body` whenever `end == true`, including for an empty body
(`write_body` short-circuits to `send_data(remaining, end)` when the payload is empty).
All five callers of `PluginSession::shutdown()` write an END_STREAM body immediately before:

- `plugins/http/common/auth_common.rs::send_gateway_auth_error_response`
- `plugins/http/common/auth_common.rs::send_gateway_plain_error_response`
  (the deny paths of `IpRestriction` / `GlobalAccessControl`, i.e. the ones the finding named)
- `plugins/http/forward_auth/plugin.rs`
- `plugins/http/all_endpoint_status/plugin.rs`
- `plugins/http/openid_connect/callback.rs::send_plain_error`

so `ended == true` and `shutdown()` returns without sending anything.

**3. The one branch that does fire is correct as-is.**

`forward_auth` and `all_endpoint_status` swallow write failures with `let _ = ...`. If the
body write fails, `ended` stays false and the response is half-written on the wire; the
subsequent `RST_STREAM(INTERNAL_ERROR)` is exactly the right signal (the docs on
`protocols/http/v2/server.rs::HttpSession::finish` note that dropping without END_STREAM leaves
the client treating the session as bad or incomplete). Adding an `is_http2()` gate would
suppress that reset.

**4. The gRPC gate guards a different hazard, which is why it is spelled differently.**

The `is_grpc_request` early return exists so a `RST_STREAM` can never be read as
`Internal(13)` in place of an already-written `grpc-status`. That is a semantic concern about
gRPC status interpretation, not about connection teardown — it does not generalize to plain
HTTP/2, and `is_http2()` is not the right predicate for it.

## Fix Suggestions Not Accepted

- **Add `|| self.inner.is_http2()` to `PingoraSessionAdapter::shutdown()`** — no-op on every
  normal path, and a regression on the swallowed-write-error path (rationale 3).
- **Delete the now-dead `!is_http2()` gate in `shutdown_after_error_response` to converge the
  two sites** — evaluated and deliberately declined. It is genuinely unreachable for H2 there
  (`send_html_error` propagates the body-write error with `?` before reaching the shutdown), so
  removing it would be a pure error-path behavior change with zero observable benefit. It stays
  as an explicit statement of intent; the doc comment now says so.
- **Route plugin deny paths through `error_response.rs` instead** — those helpers own the
  nginx-style HTML body and `ServerHeaderOpts`; plugins need operator-configured status, content
  type, and body (see `plugins-http-F014`, which established `send_gateway_plain_error_response`
  as the plugin-side helper).

## What changed (comment-only, 2026-08-03)

No behavior change. The finding was produced by a **factually wrong doc comment**, now corrected:

- `error_response.rs::shutdown_after_error_response` previously claimed the H2 call would
  "trigger an abrupt TCP-level close via the Pingora framework, causing transient EOF errors for
  subsequent connections being accepted on the same listener port". That mechanism does not
  exist. The comment now states the real H2 semantics and flags the divergence as intentional.
- `session_adapter.rs::shutdown` gained a comment explaining why it has no `is_http2()` gate.

## Re-evaluation Triggers

Re-open only if:

- Pingora changes the H2 arm of `HttpSession::shutdown` to act on the connection rather than
  the stream (check `protocols/http/v2/server.rs::shutdown` on a fork rebase / version bump).
- A `PluginSession::shutdown()` caller is added that does **not** write an END_STREAM body
  first — then `ended` is false by construction and the reset semantics need re-deciding.
- An operator reports actual `RST_STREAM` / stream-reset errors on HTTP/2 deny paths, which
  would mean `ended` is false in a path not enumerated above.

## Reference Cases

- Original candidate: `tasks/fable-review/_candidates/G14/gw-plugin-acl-rl-01-*.md` (closed
  without code fix; the reachability chain it described is accurate, the harm mechanism is not)
- `plugins-http-F014` — established the shared plugin error-response helper and its
  `write_response_header` → `write_response_body` → `shutdown` sequence
- `edgion-gateway/src/plugins/runtime/session_adapter.rs` — `PingoraSessionAdapter::shutdown`
- `edgion-gateway/src/runtime/server/error_response.rs` — `shutdown_after_error_response`
