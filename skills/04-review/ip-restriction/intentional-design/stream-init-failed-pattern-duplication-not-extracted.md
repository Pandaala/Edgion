---
name: stream-init-failed-pattern-duplication-not-extracted
description: Current reference-only IpList ownership, actual-owner binding, canonical borrowed matching and preserved historical review evidence.
knowledge_type: intentional-design
---

# Stream ACL initialization ownership

The former `cfg.clone(); validate_and_init(); init_failed` wrappers no longer exist.
Connection and TLS constructors consume actual site loans and construct one
`CompiledIpAcl` through the canonical key/read core. They retain scalar policy,
not plugin-owned CIDR trees or copied collections. No generic initializer trait or
compatibility helper replaces the removed wrappers.

Both runtimes verify actual owner, policy and side proofs before cached entry reuse.
Invalid sources/configurations produce their existing front fail-closed gates,
while disabled and policy-bypassed known entries remain inert. Connection uses the
physical peer; TLS preserves RemoteIp/DirectPeerIp selection. Each callback captures
one ConfigData snapshot and lazily borrows authority-filtered list matchers.

Historical `plugins-stream-F003` declined an initializer abstraction because it
needed a trait to bridge distinct config types for a small cold-only saving. That
tradeoff applied to the previous local matcher architecture, not to permission to
reintroduce duplicated matcher ownership. `ARCH-028` changes the ownership boundary:
canonical IpList compilation and `evaluate_ip_access_multi` own matching, and
`access_decision_to_stream_result` owns the unchanged protocol result mapping.

Re-evaluate actual site/proof pairing, before-cache rejection, nonexecuting masks,
list-only recovery and the original stream/TLS policy and source-selector cases.

Canonical contract: [grouped lists](../../../01-architecture/00-common/04-grouped-list-pattern.md).
