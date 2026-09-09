# B0215 — UDP Route Manager

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/routes_mgr.rs`.
- Baseline: `aec5caf5`.
- Analyst: `b0211_final_review`, acting only as the B0215 analyst, not its implementer/reviewer.
- Investigate serialized publication, full/partial listener cleanup, unit identity,
  retained manager handles and stats through actual UDP and Gateway consumers.
- Existing manager-cache documentation task and D0120–D0122 remain separate.
- No implementation authorized; integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: examined-no-change. Target blob `88368cdef98c7ab67d972f7d8e6275c525db11f1` at `aec5caf5`; analyst freshly passed UDP manager 6/6.
- Full/partial and Gateway rebuilds share one mutex; affected helper caller is already locked. Canonical L4 helpers preserve pre-create, old/new listener union, empty buckets and stale/orphan retained-handle clearing without UDP-private duplication.
- Handler compiles units; table/manager reuse their Arcs and sibling WRR state. Existing sessions use pinned upstreams before table lookup, so table clearing does not immediately revoke their decisions. No new session policy is proposed.
- Both stats counters feed admin output; diagnostic counts do not require a new cross-field transaction lock. No local runtime/logging/transport defect found.
- Existing manager-cache documentation task extended with UDP per-packet versus new-session lookup, stale raw-schema/full-rebuild comments, UDP architecture from_routes and shared-index wording. No separate duplicate issue or technical allow-list.
- At promotion independently review and rerun manager, handler or complete UDP filters. Prior B0211 all-UDP results are historical, not this report's fresh full verification.
