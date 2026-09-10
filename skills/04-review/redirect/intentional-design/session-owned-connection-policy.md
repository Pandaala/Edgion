---
name: session-owned-connection-policy
description: Review the dedicated HTTP-to-HTTPS redirect listener's keepalive and Connection header ownership.
knowledge_type: intentional-design
---

# Redirect connection policy belongs to the session

## Conclusion and scope

The dedicated `EdgionHttpRedirectProxy::request_filter` retains Pingora's normal
connection policy for both 301 redirects and its two Host-validation 400 replies.
It does not force closure or manually set `Connection`. The user accepted this
choice while resolving `clarify-redirect-connection-policy.md` (D0108).
This decision does not cover the HTTPRoute RequestRedirect plugin or alter Host
validation, redirect destinations, or access-log redaction.

## Rationale and evidence

`runtime/server/listener_builder.rs` selects this handler for non-TLS listeners
with `edgion.io/http-to-https-redirect: "true"`.
The pinned Pingora revision `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e`
sets H1 session `keepalive_timeout` in `respect_keepalive`. Ordinary HTTP/1.1
requests default to reuse; a client `Connection: close` disables it.
`prepare_response_header_for_write` generates the response Connection header
from `will_keepalive`, and the downstream session's `finish`/`reuse` path
returns a reusable stream only when permitted.

Previously, all three handler branches inserted `Connection: close` without
disabling session keepalive. The H1 writer overwrote it. Removing those redundant
assignments makes the handler's intent agree with its existing wire behavior.
The original finding establishes misleading policy expression, not evidence that
the emitted header disagreed with the connection's reuse state.

## Alternatives and limits

Forcing closure through the session API was considered and rejected for this
task: no product requirement demanded it, and retaining normal reuse avoids
unnecessary reconnects for subsequent HTTP requests. The accepted cost is that
the HTTP connection may remain idle after a client follows the HTTPS redirect.
This is not a guarantee of reuse for every request: client closure, framing,
unread bodies, transport errors, and Pingora lifecycle policy can prevent it.
No new connection-policy configuration or request-body buffering is introduced.

## Verification and re-evaluation

`redirect_connection_policy_matches_wire_and_stream_reuse` exercises ordinary
HTTP/1.1 and client-close requests across 301, duplicate-Host 400, and empty
normalized Host 400 responses using in-memory streams. It checks wire headers
and `finish` results, serves a second request on reusable streams, and checks EOF
on client-close streams.
The existing wire test separately covers query preservation and IPv6/custom-port
Location formatting. These tests do not establish HTTP/2, HTTP/1.0, unread-body,
or listener-level idle-timeout behavior.

Current verification (2026-09-10):

- `cargo test -p edgion-gateway --lib routes::http::redirect_http::tests`: 10 passed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed; Clippy retains existing
  workspace warnings.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only review: LGTM. No full workspace test run or existing
  integration-file changes were required for this behavior-preserving cleanup.

Re-evaluate if Pingora changes header ownership or reuse semantics, this listener
gains another protocol path, or an explicit product requirement calls for forced
closure. Historical B0194 validation covered client-requested closure only.

## Resolution references

- D0108 / `clarify-redirect-connection-policy.md`: previously pending design
  decision, discovered during B0194; normal keepalive accepted, misleading header
  assignments removed, independently reviewed and verified; active task removed.
