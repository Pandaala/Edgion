# Redis command pool-member selection

Status: deferred routing and performance decision for the user.
Source baseline: `f600df823`. No runtime or dependency change.

## Current evidence

The shared RedisRateLimiter::eval_script calls RedisLinkClient::pool(), which
returns a borrowed Fred Pool. EVALSHA and the exact NOSCRIPT EVAL fallback each
use that pool. The captured LinkSys generation survives the asynchronous exchange.

Pinned Fred 10.1.0 implements Pool's ClientLike::inner by selecting a member on
each call. The default prefer_connected setting uses next_connected: advance an
atomic counter, then scan at most the fixed pool size for a connected member.
If none is connected it returns the starting member. Selection itself returns a
borrow, not a newly allocated client or payload copy.

The Lua implementations call inner for Cluster key-slot validation. With the
workspace's partial-tracing feature, request_response calls it for tracing
selection; the untraced branch then enters basic_request_response. Request setup,
prepare_command's inherited options and conditional timeout lookup, and default
send_command also select through inner. Thus one Lua command can select multiple
times. The exact count depends on control flow; this is not evidence of several
wire commands or a demonstrated performance bottleneck.

Source locations in Fred are clients/pool.rs, commands/impls/lua.rs, utils.rs,
runtime/_tokio.rs and protocol/command.rs. Edgion's entry is
edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs::eval_script.

## Decision and alternatives

Keep the existing Pool dispatch, or separately design borrowing a selected Client
for one command. A further alternative pins that member across both EVALSHA and
its permitted EVAL fallback. These are distinct routing decisions: changing the
number of selections changes counter progression and the actual submitting
member; keeping a member across await also changes fallback selection when
connection state changes. Cluster topology, blocking policy and option reads
currently occur through the selected members at their respective call sites.
Equal construction settings do not establish equivalent live member state.

A selected-member design could remove repeated counter updates and scans, but
no measured cost, contention, allocation reduction or throughput gain is claimed.
Do not bypass Fred's command preparation, routing or reconnect machinery to
obtain this optimization. Do not cache a member in a plugin or across requests.
The separate [fallback input ownership task](redis-02-fallback-command-ownership.md)
preserves existing pool routing and does not authorize this change.

## Acceptance for an approved implementation

Specify the exact selection lifetime and behavior when a selected member loses
connectivity. Preserve the captured LinkSys generation, aggregate health gate,
retirement ownership, key-slot validation, command options and deadline policy.
Do not introduce application replay on timeout, disconnect or ambiguous execution;
only the existing exact NOSCRIPT classifier allows one EVAL fallback. Keep the
same immutable script, keys and arguments and all existing result/failure mappings.

Compare selection/counter work and command preparation with network-free
instrumentation, including all-connected, partially connected and all-disconnected
pools, concurrent callers and cancellation. Separately validate warm/cold script
execution, member loss/recovery, fallback and generation replacement against
exclusively owned Redis fixtures. Standalone results do not prove Cluster slot
routing, topology changes or Sentinel failover. SCRIPT FLUSH must never target
a shared server, even through a separate logical database. Existing integration
fixtures remain frozen for this campaign.

Keep canonical LinkSys clients, dial-time egress policy, sensitive-data handling
and access-log ownership. Add no hot-path tracing or diagnostic key/endpoint
payloads, and no new metrics merely to justify the optimization. An approved
implementation needs its own reviewed scope and fresh verification.

## Original knowledge and verification limits

- [Aggregate pool health](../../skills/04-review/link-sys/regression-guards/redis-pool-aggregate-health.md)
- [Script cache lifecycle](../../skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md)
- [Dial-time egress policy](../../skills/04-review/link-sys/regression-guards/redis-dial-time-egress-policy.md)

This task records source evidence and an unresolved choice. Existing tests cover
the current implementation, not selected-member routing. No live Redis experiment,
selection instrumentation or throughput benchmark ran for this audit.
