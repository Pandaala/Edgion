---
name: redis-configuration-contract
description: Use when reviewing Redis LinkSys schema, validation, Fred mapping, timeouts, retries, pooling, or cluster redirects.
knowledge_type: regression-guards
finding_id: redis-configuration-contract
---

# Redis configuration fields have one validated runtime meaning

## Conclusion and scope

Every operator-facing Redis field must map to Fred behavior or be absent from
the typed schema. The supported contract is:

- `db`: integer from 0 through 255; the selected database must also exist on
  the target Redis server.
- `pool.size`: integer from 1 through 64, default 8.
- `timeout.connect`: positive GEP-2257 duration no greater than 10 seconds,
  default 5 seconds.
- `timeout.command`: positive GEP-2257 duration no greater than 30 seconds,
  default 5 seconds.
- `topology.cluster.maxRedirects`: integer from 0 through 64; when absent,
  Fred's default of 5 applies. The `cluster` subobject is optional.
- Redis endpoint syntax is authority-only: `host`, `host:port`, bracketed IPv6,
  and the same forms under `redis://` or `rediss://`. The default port is 6379;
  user information, non-root paths, queries, fragments, unknown schemes, empty
  hosts, and port 0 are rejected.
- Standalone has exactly one top-level endpoint. Cluster has one or more
  top-level endpoints. Sentinel has no top-level endpoints and instead requires
  a non-empty `topology.sentinel.sentinels` list plus a non-empty `masterName`.
  Inactive topology subobjects are rejected.
- Every active endpoint parses successfully or the complete configuration is
  rejected. Without explicit `tls.enabled=true`, the active list cannot mix
  TLS and plaintext schemes. Validation errors name only the field/index and
  never echo the submitted endpoint.

Redis has a fixed infinite connection-recovery policy so a published client can
recover after an outage. Ordinary commands have one execution attempt and are
not transparently replayed when the outcome is unknown. Cluster `MOVED` and
`ASK` routing remains independently bounded by `maxRedirects`.

## Removed configuration

Redis does not expose `retry`, `pool.minIdle`, separate `timeout.read` and
`timeout.write`, `topology.cluster.readFromReplicas`, or connector-local
`observability`. These fields previously had no effect or had ambiguous runtime
semantics. Binary-owned logging and the canonical metrics inventory remain the
only observability controls.

The LinkSys CRD intentionally preserves provider-specific configuration because
several providers share the same structural branch. On strict operator ingest,
the generated `LinkSysSpec` schema reports removed Redis keys through advisory
`unknown_fields`; it does not silently recognize them and does not turn schema
drift into hard admission rejection.

## Validation and runtime boundary

`RedisClientConfig::validation_errors` is the canonical value validator used by
Controller resource validation and repeated by the Gateway before building Fred
configuration. The same type resolves timeout defaults and bounds, preventing
the Controller and Gateway from interpreting a value differently. Runtime
mapping must reject invalid values rather than clamp them.

The Gateway sets Fred's connection timeout, command timeout, pool size,
`max_command_attempts`, and optional cluster redirect limit explicitly. Any new
Redis field must be wired through schema, validation, runtime behavior,
documentation, and tests in the same change.

## Verification and re-evaluation

Resource tests cover accepted limits, rejected values, defaults, Controller
validation propagation, advisory reporting of removed fields, and the committed
CRD preservation markers that let removed nested keys reach strict ingest.
Gateway tests cover the Fred mapping for timeouts, single-attempt commands,
redirect limits, pool defaults, rejected oversized pools, all-or-nothing
endpoint parsing, topology cardinality, and transport selection. Controller and
dynamic integration regressions cover `Accepted=False/Invalid` and retention of
the last-known-good client for an invalid hot-update generation. Re-evaluate
this contract when upgrading Fred, changing Redis client publication or
reconnect behavior, or adding a Redis operator-facing setting.

## Resolution references

- `redis-configuration-contract` — fixed by removing ineffective fields and
  aligning the remaining schema, validation, Fred mapping, documentation, and
  tests.
- `D0013` / `redis-endpoint-contract` — fixed by moving topology, syntax, and
  transport validation into the shared Redis resource contract and removing
  Gateway-side lossy parsing.
