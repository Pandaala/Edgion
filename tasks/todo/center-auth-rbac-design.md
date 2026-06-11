# Design: Center auth & RBAC — two tiers + multi-Center

> **Repositioned:** this is now owned by the **Management Plane**, a separate optional
> component — NOT the lean Federation Hub. See `center-split-hub-management-plane.md`. The Hub
> keeps `login = admin` (direct mode) or trusts the Management Plane (fronted mode); it does
> not embed an RBAC engine. Everything below describes the Management Plane.
> (Note: revise the peer-replication conflict policy from last-write-wins to **revoke-wins** —
> LWW can lose a revocation and cause privilege escalation; see the Policy-CRDT research.)

**Profile:** design / future
**Status:** todo (design draft — not started)
**Relates to:** `center-build-in-frontend.md` (decided "login = admin" for now),
`center-audit-log.md` (audit is the compensating control).

## Goal

A single Center codebase that supports two deployment tiers, plus a story for running
**multiple independent Centers** without a shared single point of failure:

- **Enterprise tier** — authn via OIDC (Okta); authz **stateless** (IdP claims + declarative
  config). Multi-Center needs **no sync**.
- **Personal / self-managed tier** — DB-backed (MySQL or similar); **feature-rich** in-product
  management of users / roles / bindings. Multi-Center needs a **sync mechanism**.

## Key insight (why the tiering resolves the hard problem)

The tier that actually scales to **multiple Centers** is the **stateless** one (OIDC + config):
both Centers validate the same issuer independently and load the same policy config, so they
are consistent with **zero runtime sync and no DB**. The **stateful** tier (DB-backed,
runtime-mutable RBAC) is typically **single-Center / small-scale**. The "sync permissions
across Centers" problem only exists at the intersection "multi-Center **and** DB-backed", which
is the narrow case the sync mechanism below targets.

## Unifying abstraction (one codebase, two tiers)

A pluggable store selected by config (`auth.mode: oidc | db`):

```
trait IdentityProvider { fn resolve(&self, token) -> Principal; }      // who
trait AuthzStore       { fn permissions_for(&self, p: &Principal) -> PermissionSet; }  // what
// DB tier additionally implements admin CRUD:
trait AuthzAdmin       { create/update/delete users, roles, bindings; }
```

Implementations:
- `OidcPolicyStore` (enterprise): identity from OIDC (`UnifiedAuthClaims`); permissions from
  the `groups`/`roles` claim mapped through static config. **Read-only, stateless.**
- `DbPolicyStore` (personal): identity + permissions from SQL; mutable via UI.

The Center authz middleware calls `AuthzStore::permissions_for(principal)` uniformly; the rest
of Center is tier-agnostic. (Authn already exists via `unified_auth`: OIDC + local_auth. This
adds the **authz** layer Center currently lacks — see `center-build-in-frontend.md`.)

## Tier A — Enterprise (OIDC / Okta), stateless

- AuthN: OIDC, each Center validates the same Okta issuer independently (no sync).
- AuthZ: role from `groups`/`roles` claim; `group → role → permissions` mapping is
  **declarative config** (file / ConfigMap), deployed identically to all Centers via GitOps.
- **No DB, no inter-Center sync.** Changing access = change Okta group membership or the
  config (redeploy). **No runtime-mutable RBAC state** (this is the constraint that keeps it
  stateless — do NOT add UI-driven ad-hoc grants in this tier).
- Multi-Center: trivially consistent.

## Tier B — Personal / self-managed (DB-backed), feature-rich

- AuthN: local users in DB (password hash, optional API tokens).
- AuthZ: `users`, `roles`, `role_bindings`, fine-grained `permissions` in DB; **full CRUD in
  the UI** (create users/roles/grants at runtime).
- DB: **MySQL** (or Postgres) for the multi-Center / larger case; **SQLite** acceptable for a
  single-node personal install. Abstract behind the storage trait so the backend is swappable
  (candidate: `sqlx` — one async API over sqlite/mysql/postgres; note current `CenterDb` uses
  `rusqlite`/SQLite only, so this is a storage-layer addition, not a rewrite of existing tables).
- Permission model: reuse the Controller's proven `Role` / verb×kind policy concepts
  (`src/core/controller/authz`) so Center and Controller speak the same authorization
  vocabulary (and it lines up with the fed ceiling — see below).

## Multi-Center sync (only the DB tier needs it)

Three mechanisms, with the recommendation:

1. **gRPC peer replication (RECOMMENDED).** Reuse the existing fed-sync gRPC relay, extended to
   a **Center↔Center** channel. Each Center keeps its **own local DB**; RBAC mutations are
   written locally and emitted as replication events tagged with an **HLC timestamp + origin
   Center id**. Peers apply with **last-write-wins per entity** (deletes as tombstones); on
   reconnect/startup run **anti-entropy** (exchange version vectors, pull missing). RBAC is
   **low-write and usually single-admin**, so LWW conflicts are negligible.
   - Pros: **no shared DB**, Centers stay independent ("互不干扰"), fits the existing relay
     architecture, no new external dependency.
   - Cons: must build a small eventually-consistent replication layer.
2. **Shared HA DB.** Both Centers point at one HA MySQL (Galera / Group Replication / managed
   multi-AZ). Simplest code (no replication protocol).
   - Cons: reintroduces a **strong single-DB dependency** (mitigated only by DB-level HA);
     couples the Centers.
3. **External KV (etcd/Consul).** RBAC state in a consensus KV both Centers use.
   - Pros: strong consistency. Cons: trades MySQL for another central store + etcd ops.

**Recommendation:** Tier A (stateless) for anything multi-Center at scale → no sync at all.
For the multi-Center **DB** case, prefer **(1) gRPC peer replication** to honor "no strong
single-DB dependency + independent Centers"; offer **(2) shared HA DB** as a simpler opt-in for
operators who accept a DB cluster.

## Effective-permission model (unchanged by tier)

A user's effective power on a downstream controller =
**(Center authz grant for that user)  ∩  (controller→center fed ceiling `center_role`)**.

The fed ceiling (`FedRbacConfig` on the controller side) is an **independent inter-system
layer** and is **not** synced between Centers — it lives on each controller. Center's own authz
(this doc) is the per-user layer; the ceiling caps it.

## Audit interaction (multi-Center)

Audit (see `center-audit-log.md`) is **per-Center, append-only** — it does **not** need
syncing. Each Center records its own actions; aggregate for viewing by shipping the structured
audit events to a central log sink / SIEM. To avoid a strong DB dependency, the **primary audit
sink should be the structured `tracing` pipeline**, with local SQLite as an optional
browse-cache (revise `center-audit-log.md` accordingly).

## Open decisions

1. **Storage layer**: adopt `sqlx` (multi-backend) for the DB tier, or keep `rusqlite` for
   SQLite and add a separate MySQL path? (Affects how much of `CenterDb` is refactored.)
2. **Sync mechanism for multi-Center DB tier**: build (1) gRPC peer replication now, or ship
   (2) shared HA DB first and add (1) later?
3. **Permission granularity**: simple (admin / read-only) first, or full verb×kind×scope
   (per resource / per namespace / per controller) reusing the Controller's `Authorizer`?
4. **Is multi-Center + DB tier actually required?** If the DB tier is always single-Center,
   the sync mechanism is unnecessary and can be dropped entirely.

## References
- Existing authn: `src/core/common/unified_auth/` (`UnifiedAuthClaims`, OIDC + local_auth)
- Controller authz pattern to reuse: `src/core/controller/authz/` (`Authorizer`, `Role`)
- Fed ceiling: `src/core/controller/fed_sync/default_policy.rs` (`FedRbacConfig`, `center_role`)
- Existing Center DB (SQLite/rusqlite): `src/core/center/db/mod.rs`
- Fed-sync gRPC relay (vehicle for peer replication): `src/core/center/fed_sync/`,
  `src/core/common/fed_sync/proto/`
