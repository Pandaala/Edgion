# B0069 — TLS Route-Table Store Stats

## Scope

- Baseline: `8e7df35d07ee624aba4e21564830b9450b40cace`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/tls/routes_mgr.rs`.
- Approved support files: `edgion-gateway/src/routes/tls/gateway_tls_routes.rs` and `edgion-gateway/src/api/mod.rs`.
- Existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- TLS store stats report the canonical route cache and listener count but not entries retained in independently published per-listener `TlsRouteTable` snapshots.
- Without that signal, cache-residue diagnostics cannot distinguish a valid empty listener manager from a stale hostname/candidate table. The TCP sibling already exposes its independently retained table count.

## Required invariants

- Add cold diagnostic visibility only; do not change route matching, winner selection, table publication, or manager lifecycle.
- Count current table candidate entries across hostname groups and name the field so it cannot be mistaken for unique resources or the canonical route cache.
- Stats may load each manager's immutable snapshot but must not mutate state or acquire a blocking lock.
- No metric, tracing, access-log, redaction, plugin-publication, or TLS-backend change.
- The existing frozen recursive cache-residue validator must consume the additive numeric field without modification.

## Verification plan

- Cover empty, populated multi-host/candidate, and active-listener-cleared table counts.
- Update the store-stats serialization fixture.
- Rerun TLS module and exact store-stats tests plus full campaign gates and frozen-path verification.

## Deferred findings

- None.

## Implementation

- Added a cold table accessor that sums retained candidate-vector lengths across hostname groups.
- Added `total_candidate_entries_in_tables` to TLS manager store stats by loading each listener manager's current immutable snapshot without the rebuild lock or mutation.
- Updated the admin serialization fixture; the existing frozen recursive cache-residue validator consumes the additive numeric field automatically.
- Added exact multiplicity, independently retained table, and active-listener clear regressions.

## Independent reviews

- The plan review approved the retained-entry semantics, unambiguous field name, cold ArcSwap reads, three-file scope, and existing validator integration.
- The final reviewer returned LGTM with no Must Fix, Should Fix, or deferred findings and confirmed no matching, publication, lifecycle, hot-path, or observability behavior changed.

## Verification

- Table tests passed 16/16, manager tests 5/5, the exact store-stats fixture 1/1, and the full TLS module 79/79.
- Formatting, diff, workspace check/clippy, agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, Gateway API transform contract, no-tracing, cargo-deny, English-only, and frozen-path gates passed.
- Clippy retained only the known unrelated Kafka and frozen integration-test warnings. No full test suite was requested or run.

## Handoff

- Implementation committed as `6da1baa5`.
- Next campaign anchor: `edgion-gateway/src/routes/tls/tests.rs`.
