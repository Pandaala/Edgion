# Outbound URL serialization ownership

Status: deferred for user decision. No runtime change.
Source baseline: c41e16633.

## Evidence and scope

`replace_url_host` in
`edgion-gateway/src/plugins/http/common/outbound/call.rs` parses a base URL,
replaces its host, then copies the resulting serialization with `to_string()`.
The successful branch could consume the URL with `String::from(url)` instead.
Locked `url` 2.5.8 implements that conversion by moving `Url::serialization`;
its Display implementation writes the same serialization. The error fallback
would still copy the original base. This removes one successful serialization
copy, not URL parsing, host parsing or all intermediate allocations.

The dependency's `set_host_internal` copies the old suffix, truncates the
serialization to the host start, writes the new host, and restores the suffix.
It reuses or grows the existing String. Moving that String therefore preserves
its spare capacity; copying only the final text can produce smaller storage.
A local probe linked against the existing locked dependency artifact compared
both conversions after the same parse/set_host operations:

| Base | Replacement host | Output length | Copied capacity | Moved capacity |
| --- | --- | --- | --- | --- |
| `https://[2001:db8:abcd:1234:5678:9abc:def0:1234]:8443/` | `a.test` | 20 | 20 | 54 |
| `http://10.0.0.1:8080/` | `tenant-long.service.example.test` | 45 | 45 | 84 |

Both rows asserted identical output bytes. These are observed String capacities
on the local build, not stable allocator guarantees, physical heap measurements,
or throughput results. Both shrinking and growing a host can leave spare capacity.

The request caller selects a fresh Service endpoint and effective authority policy,
then stores the helper result in a local Cow. `CompiledRequest::build_request_in`
formats that base with the selected path and parses a separate final request URL.
The local base has no explicit early drop before the retry loop. This source
ownership is not proof of physical retention across every await after optimization.
`client.rs::resolve_probe_base` also uses the helper for the Service policy's
active-health host. Its result feeds separate health-request URL composition.

Only `ResolvedTarget::Service` exposes an authority service key. Service clients
already install AnchorPinnedResolver independently of current authority-policy
presence; the authority changes Host/SNI while destinations stay pinned to ready
endpoints. Static URL targets retain their existing egress behavior. This candidate
requires no resolver, policy, validation, failure, metric or cache changes.

## Decision requested

Retain the current copy, authorize moving the URL serialization with its capacity
tradeoff, or investigate a conversion/lifetime design that avoids the copy without
increasing retained storage. Do not infer a net memory improvement from one fewer
copy. No dependency upgrade or alternate URL assembly algorithm is proposed.

## Acceptance before implementation

Use IMPL with a frozen allow-list for the helper, its actual callers and focused
tests. Preserve exact URL serialization, scheme/port/path/query/fragment behavior,
parse/set_host error fallback, policy lookup timing, authority rejection, pinned
resolution and egress enforcement. Do not replace concatenation with URL joining.
Cover IPv4/IPv6 bases, shorter and longer DNS authority names, default and explicit
ports, invalid-input fallback, request assembly and active-health composition.

Measure allocation calls, requested bytes and retained capacity separately; assess
owner lifetime across request construction, retries and cancellation before making
heap or throughput claims. Existing shared outbound tests provide regression
coverage but are not a live DNS/TLS handshake or performance benchmark. Run the
applicable repository checks and obtain independent final review.
