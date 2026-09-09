# B0216 — UDP Route Table

## Scope

- Pipeline state: verified examined-no-change; awaiting local commit.
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

## Promotion and Verification

- Original analyst revalidated at `f01f3c86`: target/UDP/shared helpers unchanged; B0214 knowledge correction agrees with table generation/matching semantics. Fresh analyst target 4/4 passed without warnings.
- Independent reviewer: `b0216_table_review`; Approved, no Must Fix or Should Fix at `f01f3c86`, target blob unchanged. Personally verified complete-key sorting, O(1) first-unit owned lookup without implicit fallback, route-count semantics, real manager/handler/session consumers and existing documentation task. Fresh independent table 4/4 passed.
- No source/integration edits. Fresh coordinator table 4/4, UDP 34/34, workspace format/check/Clippy, docs (1,265 files), SSA, metrics, Pingora source, Gateway tracing, English-only, diff and 3,120-file freeze passed. Commit pending; no push.
