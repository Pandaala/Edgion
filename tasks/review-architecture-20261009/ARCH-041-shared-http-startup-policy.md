---
id: ARCH-041
status: candidate
kind: behavior-defect
priority: P2
partition: PART-377
analyst: /root/analyze_outbound
independent_verification: pending
---

# Validate shared HTTP listener startup declarations before deduplicating

## Current ownership and ordinary trigger

cli/pingora.rs obtains ConfigClient::list_gateways().data and invokes
GatewayBase::configure_listeners. ConfigClient filters ClientCache::list_owned by
process scope without sorting. The list comes from CacheData::values, backed by
HashMap::values. configure_listeners binds one service per ListenerKey. A later
Gateway with that key checks only actual bind address before continuing at
base.rs:200-216; its HTTP startup declaration is not compared. Typed inbound PROXY
declarations, by contrast, are aggregated and conflict-checked before construction.

Two non-dedicated Gateways of the same GatewayClass may declare one ordinary HTTP
bind IP/port/protocol with distinct hostnames and usable class configuration.
A enables http-to-https-redirect; B disables it and owns an HTTPRoute for
b.example.com. Controller ListenerPortManager::detect_conflicts compares listeners
inside one Gateway only and does not reject this pair.

When A is first, listener_builder.rs:195-224 constructs EdgionHttpRedirectProxy for
the entire port and returns before obtaining HTTP/gRPC route managers. Its filter
redirects every valid-Host request, including B's hostname. When B is first, the
normal routing service is built and A's redirect declaration is discarded.
Different effective redirect ports on two enabled owners likewise select a target
by list order. Source branches establish these outcomes; no live incident or
restart/wire test was run. The witness is same-class plaintext HTTP, independent
of ARCH-040's single-listener HTTPS ALPN defect.

## Bounded improvement and alternatives

Give the existing startup deduplication boundary one normalized HTTP-service
declaration, consumed by both construction and equality checks. Compare effective
redirect-versus-normal mode, effective u16 target only in redirect mode, and
resolved frontend H2 choice only where the chosen service uses it. Use existing
falsy tokens, case-insensitive true and redirect-port parse/default semantics;
compare behavior rather than unrelated raw annotation maps or spelling.

Reject disagreements through the existing startup Result/cleanup boundary with a
bounded error naming conflicting Gateway/listener metadata. Prevent Server::run
from starting a silently selected service. A small local prepared declaration or
preflight over precisely construction-eligible listeners suffices; no general
listener policy framework or per-request work is needed.

Sorting would choose a repeatable winner while discarding another declaration.
Rejecting all shared keys removes valid routing. Raw annotation equality rejects
unrelated fields and equivalent values. A normalized declaration keeps valid
sharing and establishes ownership of the physical service choice. No throughput
improvement is asserted.

## Required ownership and future verification

Equivalent declarations bind once and retain all logical owners. Keep Controller
Conflicted semantics, immutable route/plugin generations, connection-view
equivalence, TLS certificate precedence and canonical observation unchanged.
Do not add dynamic rebinding, post-start retargeting, cross-class shared ports,
Gateway recreation recovery or EGC ownership redesign. The accepted startup-only,
stable-handle and recreation/restart records do not approve this conflicting
same-class declaration; cross-class sharing remains unsupported.

Check both input orders for redirect-versus-normal and different active target
ports: both must reject identically. Equivalent spellings/defaults must bind once;
inactive target-port annotations must not cause conflicts. Preserve independent
local-IP listeners, normal route/Gateway ownership, PROXY preflight/guards,
startup error cleanup and H2 receive-window bounds. Existing inspected base tests
cover overlap/class targets, PROXY tests cover shared declarations, and route/store
tests cover logical ownership and old snapshots. No test executed here establishes
the proposed correction. Original startup, recreation, EGC stable-handle and
request-body-policy decisions were inspected. Independent verification is pending.
