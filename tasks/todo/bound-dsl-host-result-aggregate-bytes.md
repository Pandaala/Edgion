# Bound DSL host-result aggregate bytes

## Meta
- Status: pending
- Type: resource-safety
- Priority: P2

## Problem
Per-entry count and string-length limits do not bound the total bytes retained by some Gateway DSL builtin results before they enter the VM. `redis.hgetall` first receives an owned result map from the Redis client, then checks its entry count and builds a DSL map. HTTP-call response headers are bounded by count and individual string length, but lack a cumulative header-byte cap.

The existing VM value-shape limits (Map depth 64, container items 32,768, Map key bytes 262,144) and per-execution work budget apply at VM admission or VM operations. They do not cap the Redis client's earlier `HashMap<String, String>` allocation or the HTTP response-header map while it is being built. This producer-side gap remains open.

## Starting points
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`: `BuiltinId::RedisHGetAll` checks `max_map_size` only after `client.hgetall(&key).await`, then truncates individual strings and builds an `IndexMap`; `complete_http_call_response` checks header count and individual lengths while collecting response headers.
- `edgion-gateway/src/link_sys/providers/redis/ops.rs`: `hgetall` returns an owned `HashMap<String, String>`; inspect the underlying Redis API for a way to enforce a byte budget before full materialization.
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` and `edgion-dsl/src/value.rs`: existing VM admission and work limits are a separate boundary. Keep their contract intact.

## Scope
Trace Redis HGETALL retrieval through the LinkSys provider and DSL result construction, and trace HTTP-call header capture. Design a cumulative byte cap for names/keys plus values, with overflow-safe accounting and enforcement as early as each API permits. Specify whether an over-limit Redis result uses the existing `ResultTooLarge` failure map and whether an over-limit HTTP response uses the existing `http_call_failure_map`; preserve the established failure policy and fixed per-entry limits. Distinguish unavoidable dependency-side aggregation from bytes controlled during DSL Map construction; do not claim VM admission retroactively bounds producer allocation. Document any dependency limitation that prevents a hard cap before materialization.

## Verification
Add focused Gateway and provider tests for many large entries, exact-cap/over-cap boundaries, overflow handling, and failure behavior. Verify the cap at the earliest practical allocation point for each producer, and keep error details out of script-visible failure bodies and logs. Run the relevant workspace checks and review sensitive-data/logging rules.
