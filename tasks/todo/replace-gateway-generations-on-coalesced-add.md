# Replace Gateway Generations on Coalesced Add

## Meta

- Status: pending
- Type: bug
- Priority: P2
- Severity: medium

## Evidence

At `aa178a25`, CacheData dispatch and local apply paths can compress same-key
delete/add into an add-only final generation. GatewayHandler's prospective
snapshot contains the replacement, but partial_update first updates its backend
TLS identity and then calls add_gateway, which rejects an already present key.
The old Gateway subsequently supplies PGIS and TLS matcher rebuilds. A later
update/full-set may recover, but this flush does not guarantee one.

This is source-proven reachability, not a newly executed handler failure test or
exploit. Choose replacement data with unchanged socket semantics when reproducing,
so the existing fatal PROXY policy guard is not assumed bypassed. The divergence
persists beyond the accepted brief cross-store publication window.

## Acceptance and Plan Boundary

- Plan first: treat Gateway add as replacement/upsert where final-event
  coalescing permits it. Keep CacheData compression, not forced delete replay.
- Accepted final generation must reach raw Gateway, PGIS identity/hostname/
  allowedRoutes, TLS matcher and backend identity consistently.
- Prospective policy evaluation must match the eventual published generation;
  RejectFatal must leave business stores unchanged.
- Cover ordinary/existing-key add, compressed delete/add, update/remove and empty
  delta with local stores and existing compressed-event test seams, not global
  state pollution. Preserve unchanged-listener route-table rebuild avoidance.
- Candidate support: runtime handler, Gateway store only for necessary mutation/
  local-test seam, and CacheData compressed-event tests. Freeze final paths in
  the reviewed plan; do not create a new merged global store.
- [Authoritative backend TLS identity replacement](replace-gateway-backend-tls-identities-authoritatively.md)
  owns omitted-key removal during full snapshots. This task owns coalesced delta
  replacement; neither fix substitutes for the other. Immutable key precomputation
  is unrelated and must not be expanded here.
- Defer delete/recreate wire integration coverage; existing integration tests
  remain unchanged until the exact test paths are separately authorized.
