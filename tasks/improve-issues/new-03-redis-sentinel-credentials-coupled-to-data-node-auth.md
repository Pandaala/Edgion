# new-03: Sentinel connections always reuse the data-node credentials, so Sentinel mode cannot reach unauthenticated or separately authenticated Sentinels

- **Category**: design-error
- **Severity**: P2
- **Scope**: local (Gateway Redis LinkSys, Sentinel mode)
- **Location**:
  - `edgion-gateway/src/link_sys/providers/redis/transport.rs:36-44` (one `RedisConnectionInfo` that carries the data-node username/password), `:81-101` (`info(.., sentinel=true)` resets only `db`)
  - `edgion-gateway/src/link_sys/providers/redis/backend.rs:311-319` (`sentinel_vote` dials each Sentinel with `sentinel = true`)
  - `edgion-resources/src/resources/link_sys/redis.rs:379-387` (`RedisSentinel` has no credential field)
  - `docs/en/ops-guide/infrastructure/connectors.md:333,337` ("Sentinel nodes share data-node credentials"; `topology.sentinel.password` removed)
  - redis-rs 1.8.1 `connection.rs:1495-1522` (`check_resp2_auth`: any AUTH error other than the arity case → `AuthenticationFailed`)

## Summary

The redis-rs migration builds Sentinel connections from the same settings as data-node connections.
It only forces `db = 0`. When `auth` is configured for the Redis primary, Edgion sends
`AUTH [user] password` to every Sentinel. Sentinel authentication is configured independently of the
monitored primary (`requirepass`/ACL in `sentinel.conf`, while `sentinel auth-pass` is the Sentinel's
own credential *to* the primary). A very common setup has no Sentinel password, or a different one.
In that setup every Sentinel answers `AUTH` with
`ERR AUTH <password> called without any password configured…` or `WRONGPASS`. redis-rs turns this
into `AuthenticationFailed`, no Sentinel casts a vote, and the LinkSys never initializes. The former
`topology.sentinel.password` field was removed as a no-op instead of being wired up, so the operator
cannot work around the problem.

## Current behavior (evidence)

- `Transport::new` sets the password and username once on `settings` (`transport.rs:36-44`).
  `Transport::info` clones the same settings for Sentinel dials and only changes the database
  (`:92-96`).
- `sentinel_vote` opens each Sentinel connection with these settings before `SENTINEL MYID`
  (`backend.rs:318-319`). redis-rs runs AUTH during connection setup, and `check_resp2_auth` maps every
  server error except the "wrong number of arguments" fallback to `AuthenticationFailed`, so the dial
  fails. `discover` then returns an error when `votes.is_empty()` (`backend.rs:304-307`).
  `init` fails with the generic "Redis initialization failed" message.
- The CRD `RedisSentinel { master_name, sentinels }` has no credential field, and the docs record the
  coupling as the contract (`connectors.md:333`). The note at `:337` says the earlier
  sentinel-specific password was removed because it "was a runtime no-op".

## Industry comparison

- go-redis `FailoverOptions.SentinelUsername/SentinelPassword`, Jedis `sentinelUser/sentinelPassword`,
  Lettuce per-sentinel `RedisURI` credentials, redis-py `Sentinel(sentinel_kwargs=…)`, Spring
  `spring.data.redis.sentinel.username/password`, and redis-rs's own `SentinelNodeConnectionInfo` all
  keep Sentinel credentials separate from data-node credentials.
- Common Kubernetes operators (for example spotahome redis-operator) deploy Sentinels without a
  password while the Redis pods require one. This is exactly the combination that Edgion cannot use.

## Impact

With such a topology, Sentinel mode cannot be used at all: initialization fails, and every
Redis-backed plugin on that LinkSys follows its failure policy. The opposite workaround (configure
no `auth` and leave the data nodes open) weakens security. The fixed error text does not show that
the cause is the Sentinel AUTH step.

## Suggested direction

- Add an optional `topology.sentinel.auth` (SecretSlot, username + password) that is resolved and
  fingerprinted like `auth`. When it is absent, connect to Sentinels **without** credentials, or use
  an explicit `inheritDataAuth: true` if keeping the current behavior matters. The project has no
  backward-compatibility obligation.
- Give Sentinel AUTH failures their own fixed initialization class ("Redis Sentinel authentication
  failed") so operators can tell them apart from data-node failures.
- Update the CRD, `crd_coverage`, the connector docs, and the topology integration fixture with an
  unauthenticated-Sentinel plus authenticated-primary case.

## Verification (independent)

- **Verdict**: CONFIRMED (P2), verified by the parent on feature-10-28.
- `transport.rs:35-44`: data-node username/password are stored once in shared `settings`.
- `transport.rs:92-96`: Sentinel connections reuse those settings and only reset `db` to 0, so Sentinel AUTH uses data-node credentials.
