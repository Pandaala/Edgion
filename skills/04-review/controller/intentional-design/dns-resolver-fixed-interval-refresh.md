---
name: dns-resolver-fixed-interval-refresh
description: Use when reviewing how often the Controller DNS resolver re-resolves ExternalName Services, or a proposal to honor DNS record TTLs there.
knowledge_type: intentional-design
---

# Controller DNS resolver refreshes on a fixed interval, not record TTLs

## Conclusion and scope

The Controller DNS resolver (`edgion-controller/src/services/dns_resolver/service.rs`,
`ResolverState`) refreshes every `controller`-mode ExternalName Service on a fixed
`dns_resolver.refresh_interval` (default 15 s, floored at `MIN_REFRESH`). Record TTLs
are deliberately ignored on this path, matching Envoy's default
`respect_dns_ttl: false` with a fixed `dns_refresh_rate`.

This decision covers only Controller-published endpoints. The Gateway endpoint table
(`gateway` mode, the default since the 2026-11-10 DNS resolution redesign) does honor
TTLs: `clamp(ttl * ttlFactor, minRefreshSeconds, maxRefreshSeconds)`; see
[endpoint table lifecycle](../../backends/regression-guards/dns-endpoint-table-lifecycle.md).
`system` mode asks the OS resolver per attempt.

## Rationale and evidence

TTLs are available only for some resolvers: `edgion_common::dns::resolve_answer`
returns `DnsAnswer { addrs, ttl }` with a TTL for configured standard DNS servers and
for JSON HTTPDNS with `ttlPath`. The system resolver, TTL-less HTTPDNS formats, the
HTTPDNS system fallback, and IP literals report none. The Controller default (no
`dns_resolver.servers` / `link_sys_ref`) is the system resolver, so by default there
is no TTL to honor. The resolver still calls `resolve` on purpose.

Honoring TTLs on this path was considered and rejected as not worth its complexity:

- Controller resolution adds a hop (resolve -> in-memory slice -> EndpointSlice
  processor -> conf_sync to Gateway). Refreshing at TTL expiry still leaves the
  Gateway stale by the propagation delay, so a correct design needs refresh-ahead and
  per-target scheduling that the Gateway table already provides.
- The domains for which freshness matters most (public CDN / geo-DNS / rotating
  answers) are better served by Gateway resolution, which respects the Gateway's own
  vantage point, keeps per-Gateway answer diversity, and avoids the propagation hop.
  It honors record TTLs when its effective resolver reports them; otherwise it
  refreshes every `fallbackRefreshSeconds`. It is the default mode since 2026-11-10;
  `controller` is an explicit per-Service opt-in.
- A fixed interval is simple, predictable, and operator-tunable: `refresh_interval`
  directly bounds how long a Controller-published answer can lag DNS.

## Alternatives and limits

- Accepted cost: a record whose TTL is shorter than `refresh_interval` can be served
  past its TTL by up to roughly `refresh_interval` plus propagation delay. A record
  with a long TTL is re-queried more often than DNS requires; the query volume is one
  lookup per controller-mode ExternalName Service per interval (A and AAAA in parallel
  under `lookup_family: all`), and unchanged answers publish nothing.
- Mitigations: lower `refresh_interval`, or leave a Service whose record changes
  faster than the interval in the default `gateway` mode (every protocol).
- Rejected: `clamp(TTL, MIN, refresh_interval)`, `max(TTL, 5s)` without a cap, and
  TTL refresh-ahead (`0.8 * TTL` or `TTL - lead`). All add scheduling complexity for
  a path that should not be chosen for fast-changing records.

## Verification and re-evaluation

Coverage: `tests/unit/services/dns_resolver/service_test.rs`
(`next_due_follows_the_fixed_interval`,
`resolves_publishes_and_suppresses_unchanged_answers`).

Re-evaluate when `controller` mode must serve fast-rotating records it cannot hand to
the Gateway endpoint table, or when operators report stale Controller endpoints within
`refresh_interval`.

## Resolution references

- ctrl-svc-05 follow-up item 1 ("DNS TTL-driven refresh"), deferred 2026-11-07
  because TTLs were not surfaced; its premise was stale by 2026-11-10
  (`resolve_answer` exists) and the item was closed as an accepted fixed-interval
  design on 2026-11-10.
