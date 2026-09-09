# B0198 — TCP Configuration Handler

## Scope

- Pipeline state: terminal.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/conf_handler_impl.rs`.
- Baseline: `56cda803`.
- Freshness confirmed at `9200617b`; target, schema/Arc consumers, configuration callers and knowledge unchanged.
- Analyst: `b0198_tcp_handler_analysis`.
- Investigate full/delta publication, route initialization, listener ownership,
  and shared rebuild boundaries before selecting a value outcome.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: change; simple. Target blob: `886f2dcefea854fffaa220fbaa306d861aa04fc0`.
- Full-set clones entire TCPRoute values; partial updates unconditionally deep-copy through `arc_map_into_owned` and allocate a new schema Arc. `TcpRouteUnit::from_route` already accepts an immutable shared Arc, so preserve the incoming Arc directly.
- Change only initialization's argument, clone the Arc handle for full-set, and pass delta maps unchanged. Keep the Result adapter, shared helper, locks, listener selection, compiled-unit state, and atomic snapshots unchanged.
- This reduces cold configuration-path copy/allocation work under the rebuild lock, not request-path latency.
- Proposed scope: target; TCP route architecture page; optional focused Arc-sharing regression page and routing index. Verify full/delta pointer identity, old generation immutability, listener migration, and unaffected sibling state; preserve all existing tests.
- No new integration gap: Arc identity is an in-process invariant. No tests run during analysis.
- Unrelated `route-architecture-consistency.md` has stale claims about Gateway-triggered rebuild coverage and raw route caches; defer documentation correction. Similar TLS/UDP clones remain for their own batches.

## Implementation

- Implementer: `b0198_implementation`; exact four-file scope respected: target, TCP architecture page, new schema-Arc regression page and routing index.
- Full/add/update paths preserve incoming immutable schema Arcs. Result adapter, shared helper, locking, per-listener atomic publication and compiled selector state remain unchanged.
- Tests verify schema pointer identity, retained old generation, listener migration/new backend selection and unaffected sibling unit/selector identity. Existing tests retained.
- Related shared architecture-index drift was appended to the existing documentation task; no runtime or integration scope expansion.

## Independent Review and Verification

- Reviewer: `b0198_independent_review`; LGTM, no Must Fix or Should Fix. Independently verified the full CacheData-to-proxy chain and reran TCP tests 18/18; knowledge conclusions and preserved ownership/publication boundaries confirmed.
- TCP tests 18/18; all Gateway route tests 581/581. Workspace format, locked check/Clippy, agent-doc, SSA, metrics, Pingora, Gateway tracing, English-only, diff and frozen checks passed.
- Locked Gateway binary build passed.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
