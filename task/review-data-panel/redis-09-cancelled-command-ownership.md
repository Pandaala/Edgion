# Redis caller cancellation and command ownership

Status: deferred for user decision. Priority: P3. Type: dependency investigation.
Source baseline: `6f743246b`; Fred 10.1.0. No runtime or dependency change.

## Evidence and distinct scope

HTTP RateLimitRedis wraps check_prepared in its optional request timeout;
Stream ConnectionRateLimit wraps the shared Redis check in redis_timeout.
These callers are in `plugins/http/rate_limit_redis/plugin.rs` and
`plugins/stream/connection_rate_limit/plugin.rs` under the Gateway. Stopping
those futures releases caller-local ownership, but does not establish release
or cancellation of commands already transferred to Fred.

Fred `utils.rs::basic_request_response` builds a Command, attaches a response
oneshot, retains a clone of its timed_out flag, prepares options and submits the
command before awaiting the response. Its own timeout/error continuations set
the flag. The tracing request_response branch similarly sets it after a polled
error result; the untraced branch delegates to basic_request_response. The
shared timeout helper uses tokio::time::timeout. Dropping these futures externally
does not run their error continuations, and the inspected functions contain no
cancellation drop guard that sets the flag.

`router/utils.rs::write_command` checks timed_out before frame preparation,
then transfers ordinary response-bearing commands to the connection's in-flight
buffer before awaiting the transport write. `router/mod.rs::retry_buffer` also
checks the flag before resubmission. These checks are not proof of cancellation
after they have run. Dropping a response receiver is not proof of removing its
command from either owner, nor evidence that the server did or did not execute it.

Gateway config_mapping sets max_command_attempts to one and keeps connection
recovery enabled. That execution-attempt policy does not itself signal caller
cancellation, undo a write or establish exactly-once execution. Cluster redirect
and failure handling retain their own existing rules.

This is separate from [frame encoding ownership](redis-04-command-frame-encoding.md),
which must preserve existing cancellation behavior, and from [pool selection](redis-03-pool-member-selection.md).
No leak, unbounded retention, exact lifetime, server outcome or performance
regression has been measured by this source inspection.

## Decision and acceptance

Choose whether to investigate explicit cancellation/deadline propagation for
queued commands, or retain the current dependency ownership. Potentially avoiding
work and retaining fewer payload owners after a caller stops waiting must be
measured; changing whether a queued write executes is a behavior decision.
A proposed signal must distinguish cancellation before submission, while queued,
before writing, during encoding/partial transmission, after full transmission
and while awaiting a response. A flag alone cannot roll back a transmitted script.

Before implementation, establish a deterministic dependency-level probe covering
those states, response arrival races, reconnect queues and caller cancellation
versus Fred's own timeout. Verify bounded cleanup and retained memory separately
from command execution. Preserve in-flight response ordering; removing an owner
must not misassociate another command's reply. Never infer permission to replay
an ambiguous write or to undo quota/reservation accounting from local timeout.

Preserve captured LinkSys generations, canonical pool/egress access, existing
failure policies and the exact NOSCRIPT-only application fallback. Do not close
shared sockets, purge queues, shorten configured deadlines, add application
retries, edit the local Cargo registry or introduce another Redis client as a
shortcut. Any dependency patch needs a separately reviewed allowlist and tests.
Existing integration fixtures remain frozen; live experiments require exclusively
owned Redis instances. Standalone results cannot prove Cluster/Sentinel behavior.
No new hot-path tracing, metrics or credential-bearing diagnostics are justified.

## Verification limits

Fresh unchanged-source verification passed 24 HTTP Redis-rate-limit, 31 Stream
connection-rate-limit and 62 Redis-provider tests, with one existing Stream test
ignored. Formatting and workspace all-target check/Clippy passed with existing
warnings. These verify current local behavior and configuration; they are not
transport fault injection, a cancellation lifetime probe, a full suite or a
benchmark. No cancellation implementation is accepted by this task.

Original contracts: [Redis configuration](../../skills/04-review/link-sys/regression-guards/redis-configuration-contract.md),
[script-cache lifecycle](../../skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md).
Dependency anchors: Fred utils.rs, router/{utils,mod,commands}.rs,
protocol/{command,connection}.rs and runtime/_tokio.rs.
