---
name: fed-proxy-mtls-fail-close
description: Use when reviewing Controller federation client / HttpProxy / admin-router-without-auth findings; covers why fed_client fails closed on a non-mTLS channel and why no allow_plaintext opt-out exists on the Controller side.
knowledge_type: regression-guards
---

# Federation HttpProxy: Controller Client Fails Closed Without mTLS — FIXED

## Conclusion

**FIXED (controller-01, 2026-05-20 + RBAC layer 2026-05-21).** The Controller
federation client (`edgion-controller/src/fed_sync/fed_client/mod.rs`) now
**fails closed** unless its gRPC channel to Center is mTLS-authenticated. `run()`
calls `resolve_mtls_or_refuse(&config.security)`: it returns the active cert set
only when one is configured **and** `skip_tls` is not set; otherwise `run()` logs
`tracing::error!`, marks `system_error_state{component=fed_sync_client,
reason=no_tls_configured}` (when no TLS block is configured) or
`reason=skip_tls` (when skip_tls=true), and **does not start federation**
(degrade-not-crash -- federation is auxiliary to the data plane).

The federation path now **also routes through the Admin API authorization layer**
(RBAC, added 2026-05-21): the fed router carries `route_layer(authz)` +
`inject_center_identity`, assigning the `CenterRestricted` role to every request.
An absent `center.rbac` block installs the built-in policy from
`edgion-controller/src/fed_sync/default_policy.rs`: read access to every
`ResourceKind` except `Secret`, writes only to `EdgionConfigData`, the required
RegionRoute operations, and read-only server-info. An explicit `center.rbac`
fully replaces that default, including the option to configure deny-all. This
replaces the old bare unauthenticated router and closes the residual
authorization gap that mTLS alone did not cover.

Fix suggestions of the form "wrap the fed router with the local admin token
authn layer", "add a Controller-side `allow_plaintext` / `allow_insecure_federation`
opt-out", or "the `connect_and_run` `skip_tls` plaintext branch is still a hole"
are **not accepted**.

## Core Rationale

**1. The mTLS channel is the auth boundary, and it is now mandatory.**

Before the fix, the startup self-check was fail-open: a `None` (no TLS block) or
`skip_tls=true` channel skipped the SPIFFE check and connected in plaintext, so a
compromised/malicious Center or a MITM on the plaintext channel could drive
arbitrary admin endpoints (mutating CRUD, test-mode
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

**3. The fed router uses a dedicated RBAC layer, not the local admin token authn.**

The local Admin API authenticates with a pre-shared Bearer token (`cli_token_pre_authn`
+ the 503 token gate); Center supplies no such credential over the `HttpProxy` wire
format. Instead, the fed router uses a `route_layer(authz)` + `inject_center_identity`
pair: `inject_center_identity` stamps every federation request with a `Role::Restricted`
carrying the Center policy (no Bearer needed -- mTLS authenticates the peer); `authz_layer`
then evaluates `(Restricted, verb, kind)` against the explicit policy or the
built-in default. This
provides per-endpoint authorization without a token protocol change, and the fed router
deliberately omits the token authn layer and the 503 token gate.

## Fix Suggestions Not Accepted

- Wrap the fed_client admin router with the local admin token authn — not needed and would require a protocol change (a pre-shared Bearer token over the HttpProxy wire); the RBAC `route_layer` + `inject_center_identity` is the correct mechanism.
- Add a `allow_plaintext` / `allow_insecure_federation` opt-out on either side -- explicitly rejected by the operator; both the Controller and the Center insist on mTLS with no escape hatch.
- "`connect_and_run` still has a `skip_tls` plaintext branch" -- `run()` fails closed before `connect_and_run` is ever reached, so that branch is unreachable defensive code; verify the `resolve_mtls_or_refuse` gate in `run()` before re-reporting.
- "Center can read Secrets via the default policy" -- the exhaustive default
  policy explicitly excludes `ResourceKind::Secret`. An operator-supplied
  policy replaces the default and must be reviewed on its own merits.

## Re-evaluation Triggers

Re-open this decision only if:

- A federation transport mode is introduced that legitimately cannot use in-process mTLS (e.g., an external LB terminates mTLS and forwards plaintext over a trusted hop).
- The RBAC model needs to extend to a per-request principal beyond the fixed `CenterRestricted` role (would require a protocol change to carry a per-request identity).

## Reference Cases

- `controller-01` (P1, Security), mTLS fail-close fixed 2026-05-20; RBAC layer fixed 2026-05-21: `resolve_mtls_or_refuse` + fail-close in `fed_client::run()`; fed router now carries `route_layer(authz)` + `inject_center_identity`; `center_policy_from(None)` supplies the built-in policy.
- Sibling P0 `common-center-01` (Center fail-close + SPIFFE peer-identity binding): `docs/superpowers/specs/2026-05-20-center-fed-peer-auth-design.md`.
- Related: [admin-cors/admin-cors-same-origin-rule.md](../../admin-api/intentional-design/admin-cors-same-origin-rule.md) -- its "unified auth applied to every consumer" premise is what the old fed path violated; the RBAC layer closes this gap for the federation channel.
