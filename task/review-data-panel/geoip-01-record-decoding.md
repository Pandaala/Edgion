# GeoIP record decoding and projection

Status: deferred design decision. Source baseline: `f31f074dc`.

## Evidence

`edgion-gateway/src/geoip/provider.rs::GeoIpProvider::lookup` decodes a
maxminddb `geoip2::City` before `geoip/info.rs::project` builds owned `GeoInfo`
facts. The lockfile selects maxminddb 0.30.0. Its inspected local `geoip2.rs`
uses borrowed strings and a typed localized Names structure, not a heap map of
owned translations. Its subdivisions field is a Vec of typed subdivisions.
A record with subdivisions therefore has transient collection storage although
Edgion projects only the first subdivision's ISO code. Several other decoded
fields, including postal and registered/represented country, are not projected.
This is source evidence of avoidable-looking work, not an allocation benchmark.

Projection deliberately uses country.iso_code, continent.code,
subdivisions[0].iso_code, city.names.english and location coordinates. Empty
strings become absent; country/continent/subdivision use ASCII uppercase.
Registered/represented country are not fallbacks. The resulting strings and Arc
outlive the database read guard and remain valid after database replacement.

HTTP resolution is lazy. Equal normalized direct/remote addresses can reuse an
already initialized lookup; set_remote_ip invalidates the remote slot when its
canonical value changes. Logs inspect initialized slots without causing queries.
HTTP/stream CIDR-only rules avoid MMDB queries. Preserve these existing savings.

## Decision alternatives

- Keep the dependency's canonical City decode and measure representative records
  before taking on a custom decoding contract.
- Retain only the first subdivision while decoding and validating every trailing
  subdivision with the same typed semantics. This can target collection storage,
  but requires a reviewed custom sequence owner and type-maintenance strategy.
- Decode only projected fields. This has a wider semantic risk: fields formerly
  decoded to a specific type may instead pass through IgnoredAny. The inspected
  decoder still structurally checks skipped data, but skipping is not equivalent
  to validating the former typed field. Do not label this behavior-preserving
  without differential evidence and an explicit error-policy decision.

A decode failure is GeoLookup::Failed and drives failOpen in rule consumers;
turning an error into empty/successful facts can change an access decision.
Likewise, silently skipping invalid later subdivisions is not justified merely
because only the first one contributes displayed facts. No runtime or dependency
edit is authorized by this task record; use a separately reviewed IMPL scope.

## Acceptance for a future implementation

Compare outputs and failure classification against the current City path for
City/Country records, absent and empty fields, multiple subdivisions, malformed
unused fields and malformed trailing subdivisions. Exercise typed mismatches,
decoder structural failures and panic isolation; synthetic fault injection alone
does not establish decode equivalence. Preserve Unavailable/NotFound/Failed,
HTTP/stream failOpen, payload-free errors, and panic-only metric accounting.

Keep mapped-IP normalization, lazy slots and RealIp invalidation unchanged.
Retain atomic mmap replacement and owned facts after reload/context transfer;
do not borrow database strings into independently retained facts or add a global
lookup cache. Measure transient allocations, decoding work and retained memory
separately from final String/Arc ownership; do not infer throughput from counts.

Relevant original knowledge:
- `skills/04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md`
- `skills/04-review/observability/regression-guards/geoip-lookup-failures-follow-fail-open.md`

This record is based on current source inspection. No fresh runtime test,
malformed-record experiment or benchmark was performed for this documentation batch.
