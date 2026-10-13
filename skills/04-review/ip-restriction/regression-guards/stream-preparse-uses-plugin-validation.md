---
name: stream-preparse-uses-plugin-validation
description: Current reference-only IpList ownership, actual-owner binding, canonical borrowed matching and preserved historical review evidence.
knowledge_type: regression-guards
---

# Stateless Stream ACL policy validation

Controller preparse uses shared readonly PluginValidation for connection,
TLS-route and GCIR policy structure. No clone-and-initialize path or temporary
CIDR matcher build remains. References and scalar policy are validated here;
canonical IpList alone owns grouped CIDR partial-good admission/compilation/status.

The connection wire shadow discards removed HTTP fields while retaining a bounded
presence mask for the existing LegacySchemaDeprecated diagnostic. Removed inline
allow/deny presence is monotone typed invalidity. No deprecated accepted field,
raw rejected payload map, compatibility initializer or request-time parser remains.

Controller standalone reference observation is deliberately separate from
execution: original request-stage HTTP, connection, TLS and GCIR fields retain
passive dependency/status/requeue behavior even when disabled. Status uses existing
cache-only permission diagnostics. Every generation clears all incoming proofs and
old denial markers; only valid executable roots mint read proofs. Gateway disabled
and policy-bypassed known entries remain inert. Wrong-stage HTTP roots are clear-only.

Historical plugins-stream-F009 (2026-05-27) removed the L4 preparse clone initializer.
Its previous exclusion of TLS/GCIR and local data-plane initializer discussion is
superseded: all affected policies now share stateless validation and canonical
reference matching. Preserve existing Accepted/error-prefix and front-gate
contracts; test disabled passive diagnostics separately from executable authority.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).
