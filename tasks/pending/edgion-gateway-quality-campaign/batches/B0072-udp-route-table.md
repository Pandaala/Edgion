# B0072 — UDP Route Table

## Scope

- Baseline: `b6792e7f3c68505ca1e0d95948ceb4cc5f7feec6`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/udp/udp_route_table.rs`.
- Code allow-list: none; existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Analysis verdict: `examined-no-change`.
- The table is a small immutable Arc-shared snapshot, atomically published per listener and consulted only for new UDP client sessions.
- Deterministic namespace/name ordering preserves defined fallback behavior for invalid over-bound topology; returning an owned unit Arc correctly carries the selected generation across asynchronous admission.
- Candidate representation, sorting, and borrowing changes were behavior changes or cold-path micro-optimizations below the campaign value bar.
- B0071's cold store statistic consumes the existing `route_count()` accessor without changing table construction, matching, publication, or lifetime semantics.

## Required invariants

- Preserve deterministic first-match ordering, immutable Arc ownership, and ArcSwap publication.
- Preserve accepted-parent/controller filtering, listener scope validation, dynamic plugin lookup, backend authorization, and established-session pinning.
- Preserve canonical UDP traffic logging/redaction and avoid new hot-path tracing, metrics, allocations, or locks.
- Do not modify code for this batch unless independent review disproves the no-change conclusion and the batch is replanned.

## Verification plan

- Independently review the full table and its production builder/consumer chains against original rules and knowledge.
- Rerun focused table tests and validate compatibility with B0071 manager/API tests.
- Run full campaign gates and frozen-path verification before recording completion.

## Deferred findings

- None.

## Outcome

- Independent review confirmed `examined-no-change` with no code allow-list, corrections, or deferred findings.
- The immutable Vec preserves deterministic invalid-topology fallback, the selected unit Arc carries the exact generation across bounded asynchronous admission, and established sessions remain pinned outside the table.
- B0071's store statistic correctly uses the existing per-table count without changing construction, matching, publication, or lifetime semantics.
- Reviewed HEAD: `b6792e7f`.

## Verification

- UDP table tests: 4 passed.
- UDP manager tests: 6 passed.
- UDP handler tests: 11 passed.
- Store-stats serialization regression: 1 passed.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- Next ordered anchor: `edgion-gateway/src/runtime/matching/tls.rs` (B0073), after current-HEAD revalidation and mandatory plan review for the queued complex change proposal.
