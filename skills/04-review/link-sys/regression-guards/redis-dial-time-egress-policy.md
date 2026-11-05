---
name: redis-dial-time-egress-policy
description: Review Redis seed, recovery, discovery and redirect dial ownership.
knowledge_type: regression-guards
---

# Redis dials use the canonical egress policy

`providers/redis/transport.rs` constructs every redis-rs connection with the
adapter in `resolver.rs`. It delegates to the Gateway EgressResolver using the
system resolver and shared TTL/private-address policy. IP literals, configured
seeds, learned Sentinel peers, discovered primaries, the current known Cluster
primaries used for recovery, other Cluster nodes and MOVED
connections must all pass this path. Test mode preserves the shared loopback
exception; production does not bypass classification.

Do not replace multiplexed slots with ConnectionManager: redis-rs 1.8.1 builds
its private AsyncConnectionConfig without the resolver. Do not use its Sentinel
helper, which likewise opens discovery connections without this injection.
Cluster builder uses async_dns_resolver; direct connections use set_dns_resolver.
Canonical outbound_tls preparation supplies roots and client identity at build
 time. Custom roots replace defaults; system mode uses compiled webpki roots.
The dependency retains non-zeroizing private-key PEM copies; Edgion's prepared
material remains zeroizing. Never log connection settings or discovered targets.

Revalidate these source assumptions on every redis-rs upgrade. Adapter unit
checks alone do not prove constructor wiring; integration must cover seeds,
recovery, Sentinel discovery and Cluster redirects. Historical D0015 established
this dial-time invariant; the migration replaces its client-specific mechanism.
