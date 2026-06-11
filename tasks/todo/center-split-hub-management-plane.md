# Design: Split Center into a thin Federation Hub + optional Management Plane

**Profile:** design / architecture decision
**Status:** todo (decided — boundary defined; extraction is future work)
**Supersedes the framing of:** `center-auth-rbac-design.md`, `center-audit-log.md`
(repositioned below).

## Decision

Stop growing `Center` into a monolith. **Split its responsibilities into two deployables:**

1. **Federation Hub** (= the lean core, keep the name "Center"): the irreducible federation
   job. Stays small.
2. **Management Plane** (new, **separate component / repo**, **optional**): the heavy,
   enterprise, fast-moving governance concerns. Extracted so it can **evolve freely** without
   bloating or destabilizing the core.

Rationale: matches industry layering (Gloo OSS relay vs enterprise; Tetrate control plane vs
management plane). The concern that "Center keeps getting heavier" is resolved by moving the
heavy parts out, not by piling them in.

## Boundary — what lives where

### Federation Hub (Center core) — KEEP THIN
- Controller relay (fed-sync gRPC; controllers dial in, mTLS)
- Resource aggregation + watch cache
- Admin request **proxy** (`/api/v1/proxy/{controller_id}/*`)
- Cross-region routing / failover / consistency
- Minimal/no persistent state beyond operational
- **Auth: just enough to protect its admin API** (see dual-mode below)
- Optional **embedded basic dashboard** (lightweight — the work already done in
  `center-build-in-frontend.md` stays here)

### Management Plane (separate, optional) — ITERATE FREELY
- User-facing **AuthN** (OIDC/Okta, or local users)
- **AuthZ / RBAC** (two tiers; OPA / OpenFGA / claims — per `center-auth-rbac-design.md`)
- **Audit** (per `center-audit-log.md`)
- Multi-tenancy
- **Multi-region sync + its own DB** (MySQL / Aurora Global / DynamoDB)
- **Rich governance UI**
- May be a different stack / release cadence / license (e.g. open hub + commercial MP).

## Trust model & contract (how they connect)

The Management Plane sits **in front of** one or more Hubs. It authenticates + authorizes +
audits the human, then forwards the **already-authorized** request to the target Hub's admin
API. The Hub **trusts the Management Plane** as a privileged caller (**mTLS / service token**)
— exactly the pattern the Hub already uses to be trusted by... no: it is **one level up** from
the existing chain:

```
Management Plane  ──mTLS──>  Center Hub  ──mTLS/fed──>  Controller  ──>  Gateway
   (per-user authz,            (federation relay,        (per-cluster      (data plane)
    RBAC, audit, UI)            proxy, failover)          control plane)
```

Identity/authz concentrate at the **edge the human hits** (the Management Plane) and are
enforced downward. The Hub→Controller ceiling (`center_role` / `FedRbacConfig`) still applies
independently.

## Dual-mode (the Hub must work with OR without the Management Plane)

- **Personal / lightweight:** deploy the **Hub only**. Direct access with its current
  `unified_auth` (**login = admin**), basic embedded UI, **no DB**. This is the lean default.
- **Enterprise:** deploy **Hub + Management Plane**. Per-user RBAC, audit, multi-region, rich
  UI all live in the MP; the Hub stays thin. The MP is **purely additive / opt-in**.

So the Hub must support both: direct human auth (personal) **and** being fronted by a trusted
MP (enterprise). Keep that switch simple in Hub config.

## Repositioning of existing tasks

- **`center-build-in-frontend.md`** — **stays valid.** The embedded basic dashboard belongs in
  the Hub (lightweight, makes the Hub usable standalone). Rich governance UI → Management Plane
  (out of Hub scope).
- **`center-auth-rbac-design.md`** — **moves to the Management Plane.** The Hub does **not** get
  an RBAC engine; it keeps `login = admin` (direct) or trusts the MP. All the two-tier / OPA /
  OpenFGA / multi-region-sync design is **MP-owned**.
- **`center-audit-log.md`** — **primarily a Management Plane concern** (the human's entry point
  is the MP). The Hub just emits structured `tracing` events for its own actions; rich
  audit storage/UI/retention live in the MP. Keep "no strong DB dependency in the Hub".

## Open questions

1. **Hub auth in fronted mode**: how does the Hub trust the MP — mTLS peer identity (reuse the
   fed mTLS machinery) or a service token? And does the Hub forward the end-user identity
   (for downstream audit/attribution) or only the MP identity?
2. **Repo/stack for the MP**: same Rust monorepo as a separate crate/binary, or a separate
   repo (possibly different stack/license)? "Iterate freely" leans separate.
3. **Naming**: keep "Center" = Hub; name the new component (Management Plane / Console /
   Governance Plane).
4. **Migration**: current `Center` already has admin business routes (region-routes, IP
   restrictions, etc.). Decide which of those are Hub-core vs MP-governance and whether any
   move out.

## References
- Industry patterns (control/data plane split, relay, PDP/PEP, replication): see the research
  summary in the conversation and `center-auth-rbac-design.md` references.
- Hub internals: `src/core/center/` (fed_sync, aggregator, proxy, api, watch_cache)
