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

## Mapped CIDR insertion and query agreement

ARCH-181 corrected accepted mapped-only CIDRs that were stored in the IPv6 tree
but never reached by mapped queries. The unified algorithm builder now compiles
`::ffff:0:0/96` subnets into IPv4 prefixes; runtime query ownership is unchanged.
Strict grouped compilation rejects native/mapped aliases assigned to different
groups. Partial compilation keeps the first terminal, preserving its existing
advisory diagnostics: `collect_entry_errors` compares textual CIDRs and does not
report these semantic aliases, just as it does not normalize host-bit aliases.

Algorithm tests cover mapped /96, /120 and /128, host bits, longest-prefix lookup,
terminal aliases and excluded IPv6 families. Grouped-helper tests cover strict and
partial compilation, while HTTP tests publish actual IpLists and use `create_bound`
and the canonical evaluator to verify deny/default-Allow and whitelist decisions
for native and mapped queries. These are local unit tests, not deployed traffic or
performance measurements. No new runtime policy, logging or allocation is added.
