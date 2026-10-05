# B0071 — UDP Route-Table Store Statistics

## Scope

- Baseline: `678c21381a3ba8dfe6995e781f31fd0328e7c39f`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/udp/routes_mgr.rs`.
- Approved support file: `edgion-gateway/src/api/mod.rs`.
- Existing `edgion-tests/**`, protected user paths, UDP table/proxy/publication files, and shared L4 helpers remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `moderate`, so plan-first is not required.
- UDP store statistics expose only the canonical route cache and listener-manager count even though each listener independently retains an ArcSwap route table.
- This leaves the admin store-stats endpoint and recursive residue checker unable to observe stale UDP listener-table entries after the route cache becomes empty.
- The canonical `UdpRouteTable::route_count()` accessor already exists, matching the TCP and TLS diagnostic precedent.

## Required invariants

- Count entries in each current listener table, not unique resources; multi-listener routes count once per table.
- The diagnostic remains cold, lock-free, bounded by listener count, and explicitly does not claim a cross-manager atomic snapshot.
- Do not change route selection, ownership filtering, publication, rebuild/clearing, manager identity, unit generations, established sessions, traffic logs, metrics, or tracing.
- No sensitive data, new label surface, or duplicated cross-cutting mechanism.

## Verification plan

- Cover empty state, retained table content while the canonical route cache is empty, and active-listener clearing back to zero.
- Update the API serialization fixture and assert the additive field.
- Run the UDP module tests, full campaign gates, and frozen-path verification.

## Deferred findings

- None.

## Outcome

- Added `total_routes_in_tables` to the UDP route-manager statistics and the admin store-stats response.
- The cold diagnostic sums each independently published listener table, explicitly counts per-table entries rather than unique resources, and makes no cross-manager atomicity claim.
- Added regression coverage for empty state, two retained listener tables with an empty canonical cache, active-listener clearing, and API serialization.
- Independent final review approved the implementation with no required corrections or deferred findings.
- Source commit: `47bfe647` (`Expose retained UDP route table entries`).

## Verification

- UDP route-manager tests: 6 passed.
- Store-stats serialization regression: 1 passed.
- UDP module tests: 46 passed.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- Next ordered anchor: `edgion-gateway/src/routes/udp/udp_route_table.rs` (B0072), after revalidating its queued no-change analysis against current HEAD and the new stats consumer.
