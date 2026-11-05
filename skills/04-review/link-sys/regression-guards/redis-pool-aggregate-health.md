---
name: redis-pool-aggregate-health
description: Review Redis aggregate health, coverage and recovery.
knowledge_type: regression-guards
---

# Redis health belongs to the published generation

Redis uses active probes at a nominal five-second cadence. Standalone/Sentinel
coverage counts successful members against configured pool.size; Cluster counts
successful primaries against validated known primaries. Any success is usable,
not proof that every slot or the next command succeeds. Empty topology is never
healthy. Standalone and Sentinel member selection prefers the last successful
members. A missing pool member schedules recovery, and recovery reuses healthy
connections rather than requiring capacity for a second complete pool. Cluster
recovery runs when the slot map changes or every probed primary is unreachable.
One missed Cluster primary does not rebuild the connection while another primary
still answers. A Sentinel ROLE reply that is not master clears coverage and
schedules recovery. A ROLE timeout or I/O error keeps the PING coverage. A
command I/O or READONLY error schedules recovery only while its snapshot is
still the published one. Cluster probe addresses come from CLUSTER SLOTS on a
node already in that generation's connection map, so an empty or wildcard
hostname uses that node's address. A Cluster PING counts as live only when
the reply is PONG.

Sentinel discovery deduplicates parsed configured and learned host/port pairs
before dialing or counting votes. Repeating a configured endpoint must not
turn a tie between distinct Sentinel peers into a unique plurality. Distinct
peers voting for the same primary still count separately. Backend unit tests
cover equivalent scheme/default-port spellings and configured/learned overlap.

Probe concurrency is 64, individual timeout at most 500 ms, Cluster collection
budget two seconds plus bounded topology refresh, and the shared driver deadline
three seconds. Rotate the Cluster start index; unvisited primaries are
conservatively unavailable. Detection includes probe/scheduling latency, so five
seconds is not an SLA. Coverage is probe evidence, not a socket census.

Use PublishedHealth for metadata-only transitions. Retirement prevents late
publication. A probe applies coverage and `needs_recovery` only while its
snapshot is still the published one. A probe of a replaced snapshot reports
the published live count and must not schedule another recovery. A successful
Cluster establish publishes one live connection against the learned primary
count, so the generation stays on the handover path until that snapshot's own
probe replaces the count. An empty primary map is not published. Candidate
init proves connectivity before the persistent runner can publish. Ordinary failed
replacement retains the old publication; forced refresh
keeps its explicit retire-first semantics. Shutdown cancels and joins background
owners and releases connections; request cancellation never authorizes replay.

Re-evaluate when changing publication, probing, recovery or connection ownership.
