---
name: fed-proxy-header-forwarding
description: Use when reviewing Center `proxy_handler` / federation HttpProxy findings that claim verbatim client-header forwarding (Authorization/Cookie/hop-by-hop/X-Forwarded-For/Host) couples Center and Controller credentials or smuggles headers; explains why the blanket forward is an accepted tradeoff.
---

# Center `proxy_handler` Forwards the Full Client Header Map — DESIGN-TRADEOFF

## Conclusion

The Center `proxy_handler` (`/api/v1/proxy/{controller_id}/*rest`,
`src/core/center/api/mod.rs`) converting the inbound `HeaderMap` to a
`HashMap<String, String>` with no allow/deny list and shipping it whole to the
target controller over the federation gRPC stream is an **accepted tradeoff, not
a credential-coupling vulnerability**. The "the Center operator credential is
replayed against the controller's `unified_auth`, so the two components are
implicitly coupled at the credential level / operators must keep Center and
Controller passwords identical / a stolen Center cookie is replayed against the
controller's admin auth" premise is **refuted by the RBAC layer** (see
[fed-proxy-mtls-fail-close.md](fed-proxy-mtls-fail-close.md)).

Fix suggestions of the form "build a forwarded-header allow list because the
forwarded `Authorization`/`Cookie` is validated by the controller", or "document
that controller and Center must share the same auth principals", are **not
accepted** (the controller fed path consults neither). A minimal hygiene strip
remains available as **optional** defense-in-depth, not a required fix.

## Core Rationale

**1. The controller federation path does not consult the forwarded credentials.**

`src/core/controller/cli/mod.rs` builds the fed router from the **base** admin
router (`create_admin_router`) — *not* `compose_admin_routes`, so `unified_auth`
never runs on this path — then wraps it with `route_layer(authz_layer)` +
`.layer(inject_center_identity)`. `inject_center_identity`
(`src/core/controller/api/authz_middleware.rs:178-185`) unconditionally inserts
`authorizer.center_role()` (`CenterRestricted`) into the request extensions; it
**never reads** the forwarded `Authorization` or `Cookie`. The authorization
boundary is therefore *mTLS peer identity* + *default-deny RBAC role*, not the
replayed credential. Consequence: there is **no credential coupling** — the two
binaries do **not** need identical `[local_auth]` principals, and a stolen Center
cookie is bounded by `CenterRestricted` RBAC on the controller side (the same
default-deny posture that already closes "Center can read Secrets via the blind
proxy" in `fed-proxy-mtls-fail-close.md`).

**2. The headers land in an in-process `oneshot`, so hop-by-hop headers are inert.**

The controller dispatches the rebuilt request via
`admin_router.oneshot(request)` (`tower::ServiceExt`,
`src/core/controller/fed_sync/fed_client/mod.rs`) — an in-process service call,
not a real HTTP connection. `Connection`, `Keep-Alive`, `Transfer-Encoding`,
`TE`, `Trailer`, `Upgrade`, `Proxy-*` carry no transport semantics into a
`oneshot`: there is no socket to upgrade, close, or chunk. The "hop-by-hop header
smuggling" surface is latent (a *future* admin handler that processes WS upgrades
would inherit it), not live.

**3. `X-Forwarded-For` / `Host` reach the admin REST API, not the data plane.**

The forwarded request is served by the controller's **admin REST API** (operator
tooling: edgion-cli, dashboard, ops scripts), whose RFC-proxy conventions are
explicitly out of scope for this project's review process (see `SKILL.md`). The
controller's `RealIp` / `X-Forwarded-For` interpretation is a **data-plane**
concern and does not run on the admin path; there is no host-based admin routing
that `Host` could subvert. The fed peer is also mTLS-trusted, so any header it
relays comes from an authenticated source.

## Fix Suggestions Not Accepted

- "Build a forwarded-header allow list / strip `Authorization`/`Cookie` because the controller validates them" — the controller fed path does not validate them (rationale 1); the security justification is absent.
- "Document that controller and Center must share the same auth principals" — factually wrong; the RBAC layer means they are independent.
- "Hard-reject or treat as a live header-smuggling / RealIp-injection vulnerability" — no exploit path exists today (rationale 2 and 3); admin REST RFC-proxy conventions are out of scope.

## Re-evaluation Triggers

Re-open this decision only if:

- The federation protocol begins carrying a **per-request principal** that the controller actually validates from the forwarded headers (would reintroduce a real coupling — re-open and add the allow list).
- An admin handler is added that **processes WebSocket upgrades** or **trusts `X-Forwarded-For`/`Host`** from the `oneshot` request (the latent surface in rationale 2/3 would become live — add a deny-list strip then).

Optional, operator-discretion defense-in-depth (NOT required, per the project's
"document the risk, don't unilaterally add in-process defenses" stance): strip
the hop-by-hop set + `Authorization`/`Cookie` inside `proxy_handler` before
forwarding, since the controller fed path ignores them anyway. This minimizes
credential exposure to the (already trusted) controller without changing behavior.

## Reference Cases

- `common-center-04` (P2, Architecture), closed 2026-05-24 as an accepted tradeoff (this entry).
- [fed-proxy-mtls-fail-close.md](fed-proxy-mtls-fail-close.md) — the controller-side mTLS fail-close + `CenterRestricted` RBAC layer that refutes the credential-coupling premise.
- Source: `src/core/center/api/mod.rs` (`proxy_handler`), `src/core/center/proxy/mod.rs` (`ProxyForwarder::forward`), `src/core/controller/cli/mod.rs` (fed router build), `src/core/controller/api/authz_middleware.rs:178-185` (`inject_center_identity`), `src/core/controller/fed_sync/fed_client/mod.rs` (`oneshot` dispatch).
