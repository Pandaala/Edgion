# new-02: The Redis Cluster client fails commands on `ASK`/`TRYAGAIN`, so every resharding or scale-out causes rate-limit and quota errors

- **Category**: design-error
- **Severity**: P2
- **Scope**: local (Gateway Redis LinkSys, Cluster mode)
- **Location**:
  - `edgion-gateway/src/link_sys/providers/redis/transport.rs:131-136` (`ClusterClientBuilder::retries(0)`)
  - `edgion-gateway/src/link_sys/providers/redis/backend.rs:361-373` (only MOVED is followed), `:695-709` (`moved_target`: ASK yields no target)
  - redis-rs 1.8.1 `cluster_handling/async_connection/request.rs:252-261` (`AskRedirect` with no retries left: error surfaced, `PollFlushAction::None`)
  - `skills/02-features/03-resources/18-link-sys.md` ("ASK, TRYAGAIN and CLUSTERDOWN fail the operation")

## Summary

The new redis-rs based Cluster transport sets redis-rs retries to 0, so that a command whose outcome is
unknown after an I/O failure is never replayed. To make up for that, Edgion follows `MOVED` by hand. It
does not follow `ASK`, and it treats `TRYAGAIN` as a failure. Neither reply carries the replay risk that
motivated `retries(0)`. A node answers `ASK` or `TRYAGAIN` **without executing the command**, exactly
like `MOVED`. Slot migration is routine (`redis-cli --cluster reshard/rebalance`, adding or removing
shards, managed-service scaling). Throughout a migration, every command for a key that has already moved
out of a migrating slot fails. Every multi-key script whose keys are split across the migration fails
with `TRYAGAIN`.

## Current behavior (evidence)

- `Transport::cluster` builds the client with `.retries(0)` (`transport.rs:132`). In redis-rs 1.8.1,
  `AskRedirect` with no retries left returns the error and schedules no slot refresh
  (`request.rs:252-261`). `TRYAGAIN` (`WaitAndRetry`) is also returned at once.
- `RedisBackend::exec` follows a redirect only when `moved_target` returns a target. `moved_target`
  accepts only `RetryMethod::MovedRedirect` (`backend.rs:701`). A unit test asserts that
  ASK yields `None` (`tests/unit/link_sys/providers/redis/backend_test.rs:82-83`). The ASK error
  goes to the caller unchanged and does not schedule recovery (`command_failure_requests_recovery`
  matches only I/O and READONLY).
- The business commands are exactly the ones exposed to this: rate-limit/quota `EVALSHA`/`EVAL`
  (`plugins/common/redis_rate_limiter/mod.rs:559-567`) and single-key GET/SET/INCR. They fail during
  migration and then follow the plugin's failure policy (default fail-close for non-auth plugins).
- The documented rationale ("Unknown outcomes are never replayed") covers I/O loss only. No review
  page explains why a definite "not executed here" redirect such as ASK is treated differently from
  MOVED.

## Industry comparison

- Every Redis Cluster client follows ASK (`ASKING` followed by the command on the target node, without
  updating the slot map): redis-rs itself (when retries > 0), Lettuce, Jedis, go-redis, redis-py
  `RedisCluster`, and Envoy's `redis_proxy` (`enable_redirection` handles both MOVED and ASK). The
  Redis Cluster specification makes ASK handling mandatory for a correct client.
- These clients retry `TRYAGAIN` after a short delay, bounded by the request deadline.

## Impact

During resharding, each migrating slot produces command failures for its already-migrated keys. A
large reshard runs for minutes to hours, so a rolling share of rate-limit keys fails, and with the
default fail-close setting a share of client requests is rejected. Cluster scale-out, which operators
expect to be transparent, becomes a visible incident.

## Suggested direction

- Follow ASK the same way as MOVED, under the same `maxRedirects` and operation deadline. On the
  redirect connection, send `ASKING` and the command as one pipeline, and do not change the slot map.
  Because the redirect is a definite "not executed" reply, the no-replay guarantee still holds.
- Retry `TRYAGAIN` (and optionally `CLUSTERDOWN`) a bounded number of times with a short jittered sleep
  inside the remaining operation deadline. Both replies also mean that nothing was executed.
- Update the LinkSys contract docs and the `redis-configuration-contract` regression guard. Extend
  `edgion-tests/tests/redis_migration.rs` with a live `CLUSTER SETSLOT … MIGRATING/IMPORTING` case.

## Verification (independent)

- **Verdict**: CONFIRMED (P2), verified by the parent on feature-10-28.
- `edgion-gateway/src/link_sys/providers/redis/transport.rs:131-132`: cluster client built with `.retries(0)`.
- `backend.rs:695-709`: only `RetryMethod::MovedRedirect` is followed; ASK / TRYAGAIN surface as errors.
