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

ARCH-056 retires the connection-only HTTP-key presence mask, Controller migration
reason and Gateway duplicate predicate. Connection rules derive deserialization
through the shared IpAclReferenceSet wire: ipSource/message/status and the obsolete
legacyHttpFields key are ignored, not transported or executable. Canonical raw-ingress
schema walking supplies advisory paths; those diagnostics need not survive typed
Admin persistence or conf_sync. A valid current reference policy remains accepted.
Removed inline allow/deny presence remains monotone typed invalidity, including
when invalidInput is explicitly false or current references are also present.
No raw rejected payload map, compatibility initializer or request-time parser remains.
The prior mask already bounded retained payloads; retirement claims reduced policy
coupling, not measured performance or memory gains.

Controller standalone reference observation is deliberately separate from
execution: original request-stage HTTP, connection, TLS and GCIR fields retain
passive dependency/status/requeue behavior even when disabled. Status uses existing
cache-only permission diagnostics. Every generation clears all incoming proofs and
old denial markers; only valid executable roots mint read proofs. Gateway disabled
and policy-bypassed known entries remain inert. Wrong-stage HTTP roots are clear-only.

Historical plugins-stream-F009 (2026-05-27) removed the L4 preparse clone initializer.
Its previous exclusion of TLS/GCIR and local data-plane initializer discussion is
superseded: all affected policies now share stateless validation and canonical
reference matching. Preserve current structural Accepted=False/Invalid and front-gate
contracts; test disabled passive diagnostics separately from executable authority.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).

## ARCH-056 regression scope

Resource tests cover JSON/YAML flattening, exact advisory paths, transport omission
and monotone inline invalidity. Controller tests separate advisory acceptance from
structural Invalid and preserve reference-status tests. Gateway tests retain invalid
front-gate token non-consumption and exercise real referenced deny/allow decisions
with ignored HTTP keys. Known disabled/bypassed entries remain inert; unsupported
types remain fail closed. Standard Gateway API routing, status, transforms and
ReferenceGrant semantics are outside this change. Static guards and unit tests
are bounded evidence, not a fresh live conformance certification.
