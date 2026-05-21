---
name: fed-proxy-mtls-fail-close
description: Use when reviewing Controller federation client / HttpProxy / admin-router-without-auth findings; covers why fed_client fails closed on a non-mTLS channel and why no allow_plaintext opt-out exists on the Controller side.
---

# Federation HttpProxy: Controller Client Fails Closed Without mTLS — FIXED

## Conclusion

**FIXED (controller-01, 2026-05-20).** The Controller federation client
(`src/core/controller/fed_sync/fed_client/mod.rs`) now **fails closed** unless
its gRPC channel to Center is mTLS-authenticated. `run()` calls
`resolve_mtls_or_refuse(&config.security)`: it returns the active cert set only
when one is configured **and** `skip_tls` is not set; otherwise `run()` logs
`tracing::error!`, marks `system_error_state{component=fed_sync_client,
reason=no_tls_configured}` (when no TLS block is configured) or
`reason=skip_tls` (when skip_tls=true), and **does not start federation** (degrade-not-crash —
federation is auxiliary to the data plane). The base admin router is still passed
to fed_client **without** the unified-auth middleware *on purpose*: the
mTLS channel plus the existing SPIFFE self-check is the auth boundary for any
request the `HttpProxy` path forwards into that router.

Fix suggestions of the form "wrap the fed router with `compose_admin_routes` /
unified auth", "add a Controller-side `allow_plaintext` / `allow_insecure_federation`
opt-out", or "the `connect_and_run` `skip_tls` plaintext branch is still a hole"
are **not accepted**.

## Core Rationale

**1. The mTLS channel is the auth boundary, and it is now mandatory.**

Before the fix, the startup self-check was fail-open: a `None` (no TLS block) or
`skip_tls=true` channel skipped the SPIFFE check and connected in plaintext, so a
compromised/malicious Center or a MITM on the plaintext channel could drive
arbitrary admin endpoints (mutating CRUD, `/api/v1/reload`, test-mode
`/api/v1/admin/wipe-all`) with no credentials. The fix makes mTLS a hard
precondition; combined with the Controller's SPIFFE self-check and the Center-side
peer-identity binding (common-center-01), both ends fail closed.

**2. Both ends enforce mTLS — no plaintext opt-out on either side.**

The Center server no longer has an `allow_plaintext` escape hatch — `CenterConfig`
removed it entirely. Non-mTLS or `skip_tls=true` on either side causes fail-close:
the Center refuses to start (`run()` returns `Err` → process exits); the Controller
degrades (log + metric, does not federate, data plane keeps running). The operator
decision (2026-05-20) was "federation must be mTLS on both ends; no plaintext
escape on either side."

**3. Wrapping the fed router with unified auth would require a protocol change.**

`compose_admin_routes` enforces a Bearer/cookie session; Center supplies no such
credential over the `HttpProxy` wire format. Adding one (defense #1 in the issue)
is a cross-layer protocol change (Center + Controller + proto), out of scope for
the channel-level fix the operator chose.

## Fix Suggestions Not Accepted

- Wrap the fed_client admin router with `compose_admin_routes` / unified-auth — needs Center to carry a per-request federation principal (wire-protocol change); the mTLS channel already authenticates the peer.
- Add a `allow_plaintext` / `allow_insecure_federation` opt-out on either side — explicitly rejected by the operator; both the Controller and the Center insist on mTLS with no escape hatch.
- "`connect_and_run` still has a `skip_tls` plaintext branch" — `run()` fails closed before `connect_and_run` is ever reached, so that branch is unreachable defensive code; verify the `resolve_mtls_or_refuse` gate in `run()` before re-reporting.

## Re-evaluation Triggers

Re-open this decision only if:

- A federation transport mode is introduced that legitimately cannot use in-process mTLS (e.g., an external LB terminates mTLS and forwards plaintext over a trusted hop).
- A per-request federation principal model lands (defense #1), at which point the router could carry its own auth instead of relying solely on the channel.

## Reference Cases

- `controller-01` (P1, Security), fixed 2026-05-20: `resolve_mtls_or_refuse` + fail-close in `fed_client::run()`; regression test `federation_requires_mtls_channel`.
- Sibling P0 `common-center-01` (Center fail-close + SPIFFE peer-identity binding): `docs/superpowers/specs/2026-05-20-center-fed-peer-auth-design.md`.
- Related: [admin-cors/admin-cors-same-origin-rule.md](../admin-cors/admin-cors-same-origin-rule.md) — its "unified auth applied to every consumer" premise is what the old fed path violated.
