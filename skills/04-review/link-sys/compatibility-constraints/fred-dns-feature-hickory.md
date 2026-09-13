---
name: fred-dns-feature-hickory
description: Use when reviewing the Fred dns feature or duplicate Hickory versions in the lockfile.
knowledge_type: compatibility-constraints
finding_id: D0015
---

# Fred `dns` feature carries an unused legacy Hickory resolver

## Conclusion and scope

Fred 10.1 exposes `Pool::set_resolver` only with its `dns` feature. Enabling that
feature also activates Fred's optional `hickory-resolver` 0.24 dependency, while
Edgion's separate `ResolverSpec::StandardDns` branch uses Hickory 0.26 through
`edgion-common::dns`. Redis uses the canonical `ResolverSpec::System` branch and
therefore resolves through Tokio and the operating system. The older Hickory
version is accepted only as feature-level dependency baggage needed to reach the
resolver injection API.

## Rationale and evidence

Redis runtime code constructs `RedisEgressResolver`, which delegates to the
Gateway `EgressResolver` and `ResolverSpec::System`, then passes it to
`Pool::set_resolver` before initialization. Fred constructs its system
`DefaultResolver` with each client, but Edgion replaces it before Fred starts any
connection. Fred 10.1 itself has no call site into the optional Hickory resolver;
the dependency is feature baggage. This preserves the one-resolver rule despite
the unavoidable compiled dependency.

`hickory-proto` 0.24.4 is affected by `RUSTSEC-2026-0119`, an O(n²) DNS message
encoding CPU-exhaustion issue. The vulnerable path is unreachable here: neither
Fred nor Edgion invokes the bundled Hickory resolver, and Edgion's active resolver
uses `ResolverSpec::System`, which delegates to Tokio and the operating system.
Edgion's separately pinned Hickory 0.26 implementation remains limited to the
`StandardDns` branch. `deny.toml` carries the required explicit triage entry rather
than silently bypassing the advisory.

## Alternatives and limits

Do not use Fred's bundled Hickory resolver, expose it as Redis configuration, or
add direct calls to Hickory 0.24. Forking Fred solely to unbundle the trait from
the feature is not justified for the current pre-release dependency boundary.
Seed-only classification is not an acceptable way to remove the feature because
it fails to cover reconnect and discovered topology nodes.

## Verification and re-evaluation

Re-evaluate on every Fred upgrade. Remove the `dns` feature and the legacy
Hickory packages as soon as Fred exposes custom resolver injection without its
built-in Hickory dependency, or when Edgion replaces Fred with a client whose
dialer can consume the canonical resolver directly. Also revisit if Cargo feature
changes add a Fred call site into Hickory 0.24 or make Fred's default resolver
perform a connection before `Pool::set_resolver`.

## Resolution references

- `D0015`, Scope B accepted 2026-09-13: complete dial-time enforcement was chosen
  with this explicit, removable dependency constraint.
