# Bounded Redis hash result conversion

Status: deferred for user decision. No runtime change.
Source baseline: fb821ad80; locked Fred 10.1.0.

## Evidence and scope

`bounded_hash_result` in `edgion-gateway/src/link_sys/providers/redis/ops.rs`
checks raw Array/Map entry counts and original field/value byte totals before
Fred conversion. Admitted replies pass through `RedisValue::try_from`,
`into_map`, and conversion to `HashMap<String, String>`. The sole production
consumer of the bounded operation is DSL `redis_hash_result` in
`plugins/http/dsl/lang/builtins.rs`: it truncates fields and values at UTF-8
boundaries, moves each field String and builds an Arc value in an IndexMap.

Fred's `protocol/utils.rs::frame_to_results` constructs a Value array for a raw
array or a Fred Map for a raw map. `types/args.rs::into_map` converts an array by
popping value/key pairs from its end into a hash map. Its Map arm simply moves
the existing map. The wrapper `RedisValue::Map(map).convert()` therefore does
not itself imply another Fred Map allocation. The typed HashMap conversion in
`modules/response.rs` consumes the inner entries through FromKey/FromValue and
collects the String map. Source collection stages are not an exact allocator
count or evidence of a measured bottleneck.

There are observable ordering boundaries. Reverse pair insertion gives the
earliest original pair precedence for duplicate raw array keys. The subsequent
String HashMap has no stable iteration order. DSL truncation can collapse distinct
fields to one name; IndexMap insertion then replaces the value of an earlier
visited field. Removing a map or changing traversal can change iteration and
collision outcomes. Current randomized traversal is not a requirement to invent
a stable order, and does not authorize silently choosing a new winner policy.

## Decision requested

Retain the current conversion, or investigate removing a bounded intermediate
representation while keeping the canonical Fred conversion semantics. Compare
hashing, text copies, container allocation and peak retained memory before
selecting a design. Reuse dependency conversions where appropriate; do not add
a second general RESP decoder or replace HGETALL with HSCAN/Lua.

Resolve ordering and truncation-collision policy explicitly before a design
that changes them. Sorting, forward raw traversal, or first/last-wins rules are
behavior decisions, not automatic allocation optimizations. This is distinct
from Redis 05's numeric script results and Redis 06's bounded GET text carrier.
The existing public unbounded `hgetall` API is outside this proposal.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Keep raw count and byte admission
before conversion, deduplication or truncation. Preserve odd-array rejection,
type/byte competing-error order, fixed result-too-large and dependency failure
classes, bounded sensitive logging, per-call live client resolution, existing
command routing/deadlines and the canonical VM result budgets.

Compare RESP2 flat arrays and native RESP3 maps, empty/exact/over-limit inputs,
the currently accepted string-like frame kinds, invalid UTF-8 keys/values,
server errors and unsupported kinds. Include duplicate raw keys, distinct fields
that collide after truncation, multibyte boundaries, and defensive zero limits.
Test both final values and iteration where the approved contract requires it.
Preserve the existing success envelope and no partial-result behavior on failure.

Measure retained raw frames, intermediate containers and final DSL storage
together; Fred's complete reply aggregation remains outside the conversion cap.
No total heap bound or throughput gain follows from eliminating one container.

At this baseline, 15 Redis operation tests and 501 Gateway DSL tests passed.
They cover raw count/byte preflight, RESP2/RESP3 admission, conversion failures,
UTF-8 truncation and result envelopes. They do not prove a new collision policy,
allocation savings or live Redis transport behavior. Original contracts are
`skills/04-review/dsl/regression-guards/host-result-byte-budgets.md`,
`skills/04-review/dsl/regression-guards/redis-error-results-bounded-and-redacted.md`
and `skills/03-coding/dsl/redis-builtin.md`.
