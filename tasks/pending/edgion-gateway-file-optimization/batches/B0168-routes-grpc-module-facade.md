# B0168 — gRPC Route Manager Facade

## Scope

- Candidate analysis HEAD: `bc47034c9d355a3510e5fe1d15784dd4763aed4d`; freshness-revalidated and promoted unchanged at `11843ea1`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/mod.rs`
- Baseline blob: `d77815182ce59b4faf42ff034e3f85aaebff2fa7`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/grpc/routes_mgr.rs`
  - `edgion-gateway/src/routes/grpc/mod.rs`
  - `edgion-gateway/src/routes/mod.rs`
  - `edgion-gateway/src/api/mod.rs`
  - `skills/04-review/routing/intentional-design/route-architecture-consistency.md`
  - `skills/04-review/routing/regression-guards/http-route-manager-legacy-alias-removed.md`

## Handoff

- Analysis: `b0164_final_review` read the module facade, every re-export/caller, route managers, admin stats and existing review precedent, then revalidated the call chain after B0167.
- Decision: simple change. Remove the self-declared backward-compatibility singular `get_global_grpc_route_manager` wrapper, its two re-exports, and switch the only caller to the canonical plural getter. Correct the two knowledge pages that inaccurately leave the gRPC residue open/misnamed.
- Preserved behavior: both names address the same singleton; no manager, publication, request, protocol, error, metric, access-log, body or TLS behavior changes.
- Testing: compilation and a zero-occurrence symbol search fully guard the mechanical removal; no integration gap or edit.
- Implementation: removed the singular wrapper and its two re-exports, switched admin store statistics to the canonical plural getter, and corrected both routing knowledge pages to record that the HTTP and gRPC legacy aliases are gone.
- Review: fresh independent reviewer `b0168_final_review` verified the complete six-file diff, same-`OnceLock` caller substitution, zero remaining singular references and preserved historical knowledge. Verdict: LGTM with no Must Fix, Should Fix or deferred finding.
- Verification: gRPC tests 38/38 and admin store-stat tests 2/2; `cargo fmt --all`; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,113-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `656cf2c7`; never pushed by the campaign.
