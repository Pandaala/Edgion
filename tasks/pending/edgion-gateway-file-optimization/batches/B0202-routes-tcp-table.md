# B0202 — TCP Route Table

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/tcp_route_table.rs`.
- Baseline: `b833f400`.
- Analyst: `b0202_tcp_table_analysis`.
- Investigate deterministic construction/order, owned match results, empty and
  compiled-unit snapshots, callers and related architecture documentation.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Runtime target decision: examined-no-change; simple documentation correction required. Target blob `0f79e8a4090a9013c6c9b27327a9527fd66e1cf4`; TCP tests 22/22 passed.
- Table construction reuses compiled unit Arcs, publishes a full snapshot, preserves real table counts, and matches in O(1). Sorting-key allocations are cold-path and too weak a value case; storing only the winner would change statistics/representation without sufficient benefit.
- Owning resource architecture incorrectly says per-port registration-order selection and validation. Current Gateway architecture and historical gateway-routes-05 evidence establish deliberate deterministic resource-key fallback, not full Gateway API conflict arbitration.
- Exact proposed edit scope: `skills/01-architecture/05-resources/06-tcp-route.md` only. Clarify resolved ListenerKey scope, schema Arc compilation to unit, and namespace/name ordering; link the existing Gateway page. Do not amend unrelated Controller history or create another knowledge page.
- No source changes or new integration coverage needed. Independent review must verify both no-change value and documentation correctness.
