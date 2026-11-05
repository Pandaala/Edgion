---
name: redis-cluster-seed-loss-recovery
description: Use when reviewing Redis Cluster discovery, seed loss, MOVED limits, or recovery backoff.
knowledge_type: regression-guards
---

# Redis Cluster recovery survives loss of the configured seed

## Conclusion and scope

A published Cluster client refreshes and rebuilds from the configured seeds plus
the primaries it already learned. Known primaries are deduplicated, limited to
16, and ordered first. Configured seeds are always retained. Both the redis-rs
initial-node list and Edgion's `CLUSTER SLOTS` lookup use that candidate list,
through the canonical resolver and TLS policy. Discovery dials at most eight
candidates at a time and keeps the first valid slot map.

An older `establish` attempt cannot publish after a newer attempt starts or
after retirement. Partial pool recovery doubles the 100 ms–30 s backoff. Only a
complete recovery returns to 100 ms. Jitter stays at the wait site.

Ordinary commands still have one deadline. A lost reply is not replayed. Only
MOVED is followed, and only up to `maxRedirects`. redis-rs
`RedisError::redirect_node` returns `(host:port, slot)`. The slot is not a TCP
port; the host and port are parsed from the address before the canonical dial.

## Rationale and evidence

redis-rs 1.8.1 reconnects a Cluster connection from the initial nodes supplied
at construction. Those nodes succeed when any one connects. Edgion previously
passed only `transport.endpoints`, and `cluster_primaries` dialed the same list.
After the only configured seed was removed, a live snapshot could still name
healthy primaries, but the next rebuild and slot refresh could not reach them.

`cluster_discovery_candidates` builds the candidate list.
`Transport::cluster` receives it. `next_recovery_delay` keeps partial recovery
on the backoff curve. `establish_epoch` drops a stale publication.

## Alternatives and limits

The 16-primary cap can omit a later primary when the first 16 known primaries
and every configured seed are unreachable. Slot refresh still returns the full
primary inventory from whichever candidate answers. ASK, TRYAGAIN, and
CLUSTERDOWN remain unretried. This page does not claim every cell of the
original acceptance matrix has a live fixture; seed loss, MOVED limits, lost
INCR/EVAL replies, Sentinel failover, and cross-slot DEL are the live checks.

## Verification and re-evaluation

Unit tests cover candidate order, the primary cap, configured-seed retention,
and recovery delay. `edgion-tests/tests/redis_migration.rs` covers seed loss,
MOVED limits, lost replies, Sentinel failover, and cross-slot DEL against
`docker-compose.topologies.yaml`. Re-evaluate on a redis-rs upgrade or if
Cluster recovery starts persisting peers across process restart.
