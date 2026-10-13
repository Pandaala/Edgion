---
name: named-ip-group-documentation
description: Current reference-only IpList ownership, actual-owner binding, canonical borrowed matching and preserved historical review evidence.
knowledge_type: regression-guards
---

# Named IpGroups and reference-only ACL documentation

Business IP/CIDR collections belong only to IpList items. Each item is an IpGroup
with name, optional description and ordered cidrs. HTTP IpRestriction, connection
IpRestriction, TLS-route IpRestriction and GCIR ProfileRules contain ordered
allowRefs/denyRefs objects, never inline allow/deny arrays or CIDR strings disguised
as references. Each reference supplies name and optional namespace.

Examples must show named list resources alongside referring policies, preserve
source/group/CIDR order and describe optional ACL omission accurately. IpList owns
partial-good collection admission/status. Plugin policy validation owns source,
reference and scalar shape; published removed-field invalidity seals the generation.
Admission/decode rejection happens before publication and retains the admitted one.

Historical D0049 corrected scalar legacy allowlist/denylist examples to then-current
named inline groups without changing behavior. Its inline schema conclusion is now
superseded by the reference-only contract. Preserve its documentation correctness
lesson: a rename is insufficient if item types, resource ownership or namespaces
are wrong. Verify English/Chinese tables and YAML examples semantically.

ACL matched groups intentionally remain in denial logs under the documented
security exception; credential authentication failure groups remain redacted.
Re-evaluate typed fields, schema shape, central ownership and all examples whenever
the shared contract changes. Unrelated policy allow/deny names are not IP collections.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).
