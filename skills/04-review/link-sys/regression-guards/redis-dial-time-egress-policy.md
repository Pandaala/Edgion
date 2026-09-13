---
name: redis-dial-time-egress-policy
description: Use when reviewing Redis DNS, reconnect, Sentinel or Cluster dialing through Fred.
knowledge_type: regression-guards
finding_id: D0015
---

# Redis DNS and egress policy is enforced at every Fred dial

## Conclusion and scope

`RedisLinkClient::init` must install `redis_egress_resolver` on its Fred pool
before `Pool::init`. The adapter uses the canonical Gateway `EgressResolver`
with `ResolverSpec::System`, the Gateway-wide private-address policy, and the
shared DNS cache TTL. This applies to standalone seeds, reconnects,
Sentinel-discovered primaries and peers, and Cluster-discovered or redirected
nodes. Resolution happens for connection attempts, not Redis commands.

## Rationale and evidence

Configured Redis and Sentinel addresses enter Fred as host/port values. Fred
owns connection establishment and topology changes, so validating only the
configured seeds would miss later destinations. Fred's connection constructors
call its installed `Resolve` implementation before opening TCP or TLS streams;
the same constructors are used by reconnection, Sentinel and Cluster routing.
Adapting that hook closes the post-configuration and DNS-rebinding gap while
retaining one Edgion resolver and one address-classification policy.

The Gateway adapter also owns the existing low-cardinality egress-block metric.
Production rejects loopback, link-local and unspecified addresses. Private and
unique-local addresses follow `outbound_egress_block_private`; Redis intentionally
has no connector-level override. Integration test mode retains the canonical
loopback exception used by the shared resolver.

## Alternatives and limits

Controller-side resolution is not equivalent: the Controller resolves Kubernetes
ExternalName backends, while LinkSys Redis topology and reconnects are owned by
the Gateway process. Seed-only validation does not cover discovered nodes. A
Redis-specific DNS cache or direct Hickory resolver would violate the shared
`ResolverSpec` and egress mechanisms.

Fred's resolver hook currently requires its `dns` Cargo feature, which brings an
unused older Hickory version. That bounded dependency issue is recorded separately
in [the Fred compatibility constraint](../compatibility-constraints/fred-dns-feature-hickory.md).

## Verification and re-evaluation

Targeted tests exercise link-local rejection and the strict/permissive private
address boundary, including preservation of the Redis port. A separate
`RedisLinkClient::init` test builds a real Fred pool around a link-local literal
and proves that canonical policy rejection occurs before the connection timeout;
this locks the installation point without requiring a Redis server. Re-evaluate
when upgrading Fred, changing its connection constructor or topology routing,
adding a Redis-specific resolver option, or changing Gateway egress defaults. A
compile-only check is insufficient if Fred no longer invokes the injected resolver
for every connection path.

Fresh verification on 2026-09-13: all 34 Redis provider unit tests passed,
including the new adapter and real-pool wiring cases. Workspace formatting,
`cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed; Clippy retained unrelated
pre-existing warnings. SSA force, metrics inventory, Pingora source policy, and
the full `cargo deny check` passed. `cargo audit --ignore RUSTSEC-2026-0119`
passed with the repository's existing unmaintained-crate warnings. Independent
read-only review returned LGTM after requesting and re-reviewing the wiring test.

`validate_agent_docs.py` still reported seven links to unrelated task files that
are absent from the repository HEAD. None originates in the Redis pages added by
this fix; this baseline failure is not evidence that those new links were checked
successfully by the validator.

## Resolution references

- `D0015`, fixed 2026-09-13: Redis destinations previously used Fred's default
  resolver and bypassed canonical dial-time classification.
