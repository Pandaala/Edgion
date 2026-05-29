---
name: fed-sync-keepalive
description: FederationSync gRPC server now applies the same HTTP/2 keepalive settings as ConfigSync. Reject re-reports claiming the center gRPC server is missing keepalive.
---

# h2-grpc-04 — FederationSync gRPC server now has HTTP/2 keepalive

**Status:** ✅ fixed · 2026-05-18
**RFC:** RFC 9113 §6.7 (advisory — PING for liveness; no MUST on senders)

## Conclusion

The FederationSync gRPC server (`src/core/center/cli/mod.rs`) now applies
`.http2_keepalive_interval(Some(Duration::from_secs(10)))` and
`.http2_keepalive_timeout(Some(Duration::from_secs(5)))`, matching the
ConfigSync server (`grpc_server.rs:67-69`). Both long-lived bidirectional
stream servers now detect NAT-evicted or silently-dropped connections within
~10 s via H2 PING, instead of relying solely on the application-level
heartbeat (~30-90 s).

Suggestions of the form "center gRPC server missing keepalive" or
"FederationSync has no HTTP/2 PING" are **not accepted** — the setting is
now present on the server builder.

## RFC clause

> "Endpoints can use PING frames to check that the connection is functional.
> PING with the ACK flag set is the response that confirms reachability."
> — RFC 9113 §6.7

The RFC does not mandate senders to emit PING (no MUST). The finding was
an internal consistency issue: ConfigSync had keepalive; FederationSync did not.

## Code shape today

- `src/core/center/cli/mod.rs:151-153` — `Server::builder()` chain now includes
  `.http2_keepalive_interval(Some(Duration::from_secs(10)))` and
  `.http2_keepalive_timeout(Some(Duration::from_secs(5)))`.
- `src/core/controller/conf_sync/conf_server/grpc_server.rs:11-13,67-69` —
  ConfigSync server (unchanged reference; values 10 s / 5 s).

## Re-evaluation triggers

- Operator reports H2 PING traffic causes issues on a specific load-balancer
  or intermediary that cannot handle PING frames (then make keepalive
  configurable via `[grpc_server]` config section).
- tonic changes its keepalive API.
- RFC 9113 is updated with a stricter MUST on server-side PING support.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a2.md → V5` (CONFIRM)
