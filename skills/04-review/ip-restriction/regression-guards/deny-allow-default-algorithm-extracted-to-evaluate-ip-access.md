---
name: deny-allow-default-algorithm-extracted-to-evaluate-ip-access
description: Current reference-only IpList ownership, actual-owner binding, canonical borrowed matching and preserved historical review evidence.
knowledge_type: regression-guards
---

# Canonical borrowed ACL evaluation

`evaluate_ip_access_multi` is the single deny/allow/default algorithm. It consumes
ordered borrowed matcher iterators, keeps non-empty authored allowRefs as whitelist
intent even when no source is available, and returns the existing matched group Arc.
The first matching source wins; within that source longest prefix wins. Deny precedes
allow and lazy traversal stops as soon as the decision is known.

`CompiledIpAcl` binds actual HTTP/connection/TLS/GCIR field loans to the shared
IpListReadKey/read-policy core. `EdgionConfigDataSnapshot::ip_list_matchers` visits
those keys lazily and checks current identity/type/enable/visibility/permission.
There is no raw String-key live projector, consumer-owned inline Option matcher,
temporary Vec/SmallVec matcher union, or legacy Resources wrapper. The unused
single-matcher `evaluate_ip_access` facade is removed after its final callers.

The evaluator does not choose client address, fetch resources, decide availability
policy, or map protocol responses. HTTP uses its existing configured response;
Stream uses `access_decision_to_stream_result`. ACL absence is optional omission:
allow intent still denies misses, and deny-only absence uses its configured default.
Required Condition/Geo dependencies and trust fallback retain their distinct roles.

Historical `plugins-stream-F012` (2026-05-27) consolidated three identical local
matcher algorithms. `ARCH-028` first removed temporary borrowed-reference unions,
then the reference-only redesign removed the three local wrapper owners entirely.
The earlier F003 tradeoff is historical; it does not justify duplicate IP ownership.

Tests must prove first-source versus narrower later source, inner longest prefix,
Arc identity, early iterator termination, deny priority, IPv4/IPv6/mapped addresses,
whitelist misses and defaults. This removes intermediate matching collections;
it makes no whole-path allocation-free or numeric performance claim.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).
