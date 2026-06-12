---
name: crc32-session-affinity-not-a-security-boundary
description: Use when reviewing session-affinity token findings claiming the CRC32 token is pre-computable/forgeable with no HMAC/MAC protection; the token is an intentional stickiness hint (not a security boundary), the threat model is now documented in source, and adding HMAC is not accepted unless affinity is promoted to a routing restriction.
---

# lb-F011 — CRC32 session-affinity token is a stickiness hint, not a security boundary — DOCUMENTED

**Status:** 📝 documented · 2026-05-24 · threat-model comments added, no behavior change
**Severity:** Low (BadLogic) — undocumented security boundary; self-affecting only, no cross-user impact

## Conclusion

`generate_session_token` (`edgion-gateway/src/routes/http/session_persistence.rs`) and
`build_sp_affinity_map` (`edgion-gateway/src/lb/runtime_state/mod.rs`) compute the session-affinity
token as `format!("{:08x}", crc32fast::hash(addr_str))` — a plain CRC32 of the backend `IP:port`
with no per-process secret and no MAC. This is **intentional**: session affinity is a
stickiness/performance feature, not an authentication or access-control boundary. The defect was
the *absence of a comment saying so*, now fixed by adding a threat-model doc comment to both
sites. The token computation itself is unchanged.

Findings of the form "CRC32 token is trivially pre-computable / forgeable / no HMAC / no MAC
protection" should be closed against this entry. Adding HMAC is **not accepted** as a fix unless
session affinity is ever promoted to enforce routing restrictions (see Re-evaluation Triggers).

## Core Rationale

**1. The forgery is self-affecting only.**

The token identifies a *backend*, not a *user session*. A forged token only changes which backend
the forging client's *own* requests land on (canary bypass / load concentration on a chosen pod).
There is no cross-user session hijacking — a forged token cannot affect any other client's
traffic.

**2. A forged token cannot inject an arbitrary address.**

The affinity map (`sp_affinity_get_or_build_key`) is built solely from the service's *real*
backends. `sp_resolve_target` (`pg_upstream_peer.rs`) does `affinity_map.get(token)`, so a forged
token can only ever select an already-registered backend of that service — never an attacker-chosen
address. The resolved sticky target then still passes the normal SSRF / DNS validation in
`get_peer_with_sp_fallback` (loopback / link-local rejected). So the blast radius is bounded to
"pick one of the service's existing backends for my own requests".

**3. Severity is Low and matches the Gateway API posture.**

Gateway API does not document `sessionPersistence` as a security control, and Edgion does not
either. The token is a performance hint. The risk only rises to High if the system is ever
repositioned as enforcing access-control-via-affinity ("only this client may reach pod X") — which
it is not.

## Fix Suggestions Not Accepted

- "Introduce a per-process / per-service HMAC secret so the token becomes
  `HMAC-SHA256(secret, addr_str)[..16]`" — **not accepted** as a current fix. It is the correct
  upgrade *only if* affinity is ever used to enforce routing restrictions (e.g. tenant isolation
  by pod). The doc comment records this explicitly as the future path. Adding it now buys nothing:
  the value is self-affecting and cannot inject arbitrary addresses regardless of token
  predictability.

## Re-evaluation Triggers

Re-open this decision (and implement the HMAC fix) only if:

- Session affinity is ever documented or used as an access-control boundary
  ("only client X may reach pod Y", tenant isolation by pod).
- A feature lets the affinity token select a target *not* drawn from the service's real backend
  set (which would let a forged token point at an arbitrary address).

## Reference Cases

- lb-F011 (review finding, 2026-05-24)
- `edgion-gateway/src/routes/http/session_persistence.rs` — `generate_session_token` (threat-model comment)
- `edgion-gateway/src/lb/runtime_state/mod.rs` — `build_sp_affinity_map` (threat-model comment),
  `sp_affinity_lookup`, `sp_affinity_get_or_build_key`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` — `sp_resolve_target`,
  `get_peer_with_sp_fallback` (SSRF validation on the resolved sticky target)
