# B0216 — UDP Route Table

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/udp_route_table.rs`.
- Baseline: `3e28c944`.
- Analyst: `b0211_final_review`, acting only as the B0216 analyst.
- Investigate listener table selection, immutable unit publication and actual
  new-session consumers against the shared selection and lifecycle contracts.
- No implementation authorized; integrations and user-owned paths remain protected.

## Analysis Outcome

- Decision: examined-no-change. Target blob `a9bd78ad61a2cc1f6602273cbe9acc1f39f44262` at `3e28c944`; analyst freshly passed target table 4/4 without warnings.
- Table only clones compiled unit Arcs, sorts by complete namespace/name and returns the first owned unit in O(1); compilation and selector initialization belong to the handler. No HashMap-order dependence or changed failure fallback.
- Cold-path sorting allocation is not sufficient local value to replace the algorithm. Winner-only storage would require extra route-count state and change its current meaning without demonstrated benefit.
- Atomic manager snapshots and existing session fast-path pinning remain separate; no new logging, schema, transport or integration requirement.
- Existing manager-cache documentation task receives table compilation wording and resource-side UDP per-packet/validation drift. Technical allow-list remains empty; at promotion independently review and run full UDP smoke coverage.
