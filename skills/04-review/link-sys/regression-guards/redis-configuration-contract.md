---
name: redis-configuration-contract
description: Use when reviewing Redis LinkSys schema, validation, redis-rs mapping, timeouts, retries, pooling, or cluster redirects.
knowledge_type: regression-guards
finding_id: redis-configuration-contract
---

# Redis configuration fields have one validated runtime meaning

## Conclusion and scope

Every operator-facing Redis field has one documented runtime meaning. The supported contract is:

- `db`: integer from 0 through 255; the selected database must also exist on
  the target Redis server.
- `pool.size`: integer from 1 through 64, default 8; standalone/Sentinel only. Cluster ignores it and normalizes it in the runtime fingerprint.
- `timeout.connect`: positive GEP-2257 duration no greater than 10 seconds,
  default 5 seconds.
- `timeout.command`: positive GEP-2257 duration no greater than 30 seconds,
  default 5 seconds.
- `topology.cluster.maxRedirects`: integer from 0 through 64; when absent,
  the Edgion default of 5 applies. The `cluster` subobject is optional.
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
recover after an outage. An unknown command outcome is never replayed. For
`new-02-redis-cluster-ask-tryagain-fail-commands-during-resharding`, the
previous MOVED-only handling failed commands rejected during slot migration.
Each Cluster command now shares one extra-attempt limit across MOVED, ASK and
TRYAGAIN. ASKING must succeed on the same private connection before the
redirected command is sent. TRYAGAIN waits briefly and routes again using the
key. CLUSTERDOWN, other server errors and ambiguous I/O do not enter this loop.

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
Controller resource validation and repeated by the Gateway before building redis-rs
configuration. The same type resolves timeout defaults and bounds, preventing
the Controller and Gateway from interpreting a value differently. Runtime
mapping must reject invalid values rather than clamp them.

Redis production construction now takes the actual Arc<LinkSys>. Common's
private-field PreparedRedisTls retains its reference topology and one global TLS
snapshot; source and whole active ordered material pairing precede the runner's
digest/RV/zero-live decisions. Disabled local references remain inert; rediss
still selects TLS and inherits global defaults through an empty local policy.
Consumed inherited axes join the material digest, with local override masks.
RedisCandidate supplies the same preparation and fingerprint to every runner
retry. Construction resolves one canonical PEM material selection. Transport builds
redis-rs verifying connections; default roots are the explicit process System
bundle ([System CA single trust store](../../tls/regression-guards/system-ca-single-trust-store.md)), custom roots replace
them, and client identity uses the same ownership proof. Disabled references
remain inert. The test-util actual-owner fixture uses the production path.

Ordinary preparation/build failure retains the prior published client for new
lookups/calls under the existing custom LinkSys lifecycle. Forced refresh retires
that publication first and remains pending until a replacement publishes. A
proof failure cannot retire an ordinary zero-live publication through plan_apply.
Etcd, Elasticsearch, Kafka and OTLP candidates have since moved to the same
prepared actual-owner TLS (`PreparedLinkSysTls` / `PreparedOtlpTls`) under the
shared [resource reference contract](../../../01-architecture/00-common/06-resource-reference-contract.md).

The Gateway sets connection/response deadlines and zero Cluster retries. The
operation deadline includes migration retries and NOSCRIPT fallback; no uncertain command
is replayed. Multi-key Cluster operations use one node so CROSSSLOT cannot
partially execute through client fanout. Tests cover mapping, all-or-nothing
endpoint validation, topology, TLS, credentials and fingerprint behavior.

Historical redis-configuration-contract and D0013 established these invariants.
Revalidate source APIs, endpoint routing and live scenarios when upgrading clients.

For new-02, the Redis topology suite passed all ten scenarios. In
`cluster_migration_follows_ask_retries_tryagain_and_preserves_uncertain_writes`,
a split-slot DEL exhausted two extra attempts after three Redis TRYAGAIN
replies, respected a shorter operation deadline, and succeeded once both keys
had moved. Dropping the reply after an ASK-redirected INCR left the counter at
one. Redirect preparation errors and budget expiry bypass published-client
recovery; only the business-command result enters its failure classifier.
