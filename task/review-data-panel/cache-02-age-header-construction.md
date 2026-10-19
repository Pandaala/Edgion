# Response-cache Age header construction

Status: deferred for user decision. No runtime change.
Source baseline: 9f0b71529.

## Evidence and scope

`response_cache::lifecycle::deliver_hit` passes `age.as_secs().to_string()` to
Pingora's `ResponseHeader::insert_header`. The pinned fork's conversion accepts
`TryInto<HeaderValue>`. Locked http 1.5.0 converts an owned String through
`HeaderValue::from_shared`, transferring its buffer rather than copying the
decimal bytes into a second allocation. A direct numeric argument is therefore
not automatically an allocation removal.

The same http version's `From<u64>` uses an itoa stack formatter but reserves a
20-byte BytesMut before freezing it. A local allocator probe linked against the
existing locked artifact (with bytes 1.12.1) measured these construction paths:

| Age | String path allocation calls / requested bytes | Numeric path calls / requested bytes |
| --- | --- | --- |
| 0 | 1 / 1 | 2 / 44 |
| 9 | 1 / 1 | 2 / 44 |
| 100 | 1 / 3 | 2 / 44 |
| u64::MAX | 1 / 20 | 1 / 20 |

The probe brackets only HeaderValue construction, disables counting before
printing, and asserts identical value bytes and unset sensitivity flags. A
separate pointer assertion confirms String buffer transfer. Counts describe
allocator requests on this build, not physical heap usage, retained capacity,
whole-request allocation totals or throughput. Neither path is allocation-free.

This candidate concerns only the replay Age value. It does not authorize changing
freshness arithmetic, origin timestamps, whole-second flooring, header charging,
output deadlines or other numeric response headers.

## Decision requested

Keep the current owned String path, or authorize a measured construction change
whose CPU benefit justifies its allocation behavior. Compare typical short ages
and maximum-width values using the locked dependencies before choosing. Do not
describe direct numeric insertion as removing a redundant byte copy.

## Acceptance before implementation

Use IMPL with an exact allow-list and independent reviews. Preserve decimal bytes,
header replacement and sensitivity, origin freshness/age semantics, hit charges,
precommit error behavior and bounded downstream delivery. Measure allocation calls
and requested bytes separately from lifetime, capacity and CPU. Do not broaden
this into a workspace-wide header-formatting rewrite.

Retain the response-cache lifecycle and H1/H2 replay tests. The current broad
cache selection has an existing policy-fixture failure recorded in
[verification-01](verification-01-cache-policy-fixture.md); it is not evidence of
a constructor regression and must not be suppressed to claim a green suite.
