---
name: fed-client-connect-timeout
description: fed_client now sets .connect_timeout(10s) before endpoint.connect(). Reject re-reports claiming federation client has no connect timeout.
knowledge_type: regression-guards
---

# h2-grpc-07 — `fed_client` connect timeout added

**Status:** ✅ fixed · 2026-05-18
**RFC:** gRPC over HTTP/2 spec; RFC 9113 §3.2 (internal consistency, not a MUST)

## Conclusion

`edgion-controller/src/fed_sync/fed_client/mod.rs:193` now applies
`.connect_timeout(Duration::from_secs(10))` before calling `.connect().await?`,
matching the 10 s value used by the `conf_sync` client (`grpc_client.rs:55`).

Suggestions of the form "fed_client has no connect timeout" or "federation
gRPC channel missing connect_timeout" are **not accepted** — the setting is
now present.

**Note on `.timeout()`:** only `.connect_timeout()` was added. `.timeout()`
sets a per-RPC deadline and would kill the long-lived bidirectional `sync()`
stream after 10 s — that is wrong for this use case. The fix is intentionally
limited to the TCP handshake phase.

## RFC clause

> "The server connection preface consists of a potentially empty SETTINGS
> frame that MUST be the first frame the server sends upon a new connection."
> — RFC 9113 §3.2

The RFC does not mandate a connect timeout (no MUST). The finding was an
internal consistency gap: `conf_sync` capped TCP connection establishment;
`fed_client` did not, relying on the OS default (~75–130 s on Linux).

## Code shape today

- `edgion-controller/src/fed_sync/fed_client/mod.rs:193` — `endpoint`
  chain now includes `.connect_timeout(Duration::from_secs(10))` before
  `.connect().await?`.
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:55` — reference
  client (unchanged; 10 s from caller-supplied `timeout`).

## Re-evaluation triggers

- Operator reports 10 s is too short / too long in their network environment
  → make `connect_timeout_secs` a field in `CenterClientConfig`.
- tonic changes its channel-builder API.

## Resolution references

- `h2-grpc-07` — 2026-05-18.
