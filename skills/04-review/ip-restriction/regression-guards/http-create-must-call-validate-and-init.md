---
name: http-create-must-call-validate-and-init
description: Current reference-only IpList ownership, actual-owner binding, canonical borrowed matching and preserved historical review evidence.
knowledge_type: regression-guards
---

# Reference-only HTTP ACL construction

`IpRestriction::create_bound` validates the actual owning entry and obtains its
`CompiledIpAcl` from the owner-bound cold compiler. It stores scalar response/address
policy, a prebuilt body and the shared plan. It never clones CIDR collections,
builds a private tree, or retains a partly initialized `init_failed` runtime.

Malformed policy, source or proof metadata is checked before protected cache reuse.
The owning runtime records a non-retainable sealed configuration outcome; a clean
published generation can recover. Captured removed allow/deny fields—including
null and empty values—are typed invalidity, not watcher decode-only errors.

`run_request` ensures the decision view and uses the narrow synchronous
`PluginSession::ip_acl_decision` capability. Its owned scalar/group-Arc result ends
immutable session borrows before response writes. Ordinary denials retain configured
status/body and mandatory ACL group logs. Missing optional ACL references preserve
whitelist intent or deny-only default behavior, rather than becoming a universal
required dependency error.

Historical `fr-types-01` (2026-05-29) fixed an uncompiled allow matcher silently
falling through to Allow. The 2026-06-20 partial-good update then moved bad-CIDR
handling out of init failures. Those local matcher/initializer APIs are now removed;
the security invariant remains: authored whitelist intent cannot disappear when its
available union is empty. IpList alone owns partial-good collection compilation and
its status. The obsolete inline-deny metric is removed.

Re-evaluate source binding, cache rejection, same-view dispatch, list-only updates,
valid→legacy-invalid→clean publication, exact denial body/logs and every original
constructor/whitelist test. Historical passing tests do not establish fresh acceptance.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).
