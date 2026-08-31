# D0025 — Make GeoIP Lookup Failures Follow Failure Policy

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0059, GeoIP provider audit
- Category: security-correctness
- Canonical task: [`tasks/todo/make-geoip-lookup-failures-follow-failure-policy/`](../../../todo/make-geoip-lookup-failures-follow-failure-policy/make-geoip-lookup-failures-follow-failure-policy.md)

## Evidence
`GeoIpProvider::lookup` currently maps both MMDB record decode errors and caught reader panics to `GeoLookup::NotFound`. HTTP and stream consumers interpret `NotFound` as ordinary missing data rather than database failure, so a malformed or incompatible lower-trust record can bypass the consumer's existing `failOpen` decision. The existing panic counter documents this deliberate degradation but does not restore policy correctness.

## Why deferred
The correction spans the provider outcome model, HTTP and stream policy consumers, access-log representation, bounded metrics, and deterministic malformed-record coverage. A provider-only change would either break exhaustive consumers or leave failure semantics inconsistent.

## Integration-test impact
Existing integration tests remain frozen. Provider and consumer unit tests with a deterministic malformed-record or decoder seam should cover the complete typed outcome and failure-policy path; no separate integration-test issue is needed unless the reviewed implementation proves otherwise.
