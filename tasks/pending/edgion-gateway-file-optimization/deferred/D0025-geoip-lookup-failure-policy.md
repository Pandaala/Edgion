# D0025 — Make GeoIP Lookup Failures Follow Failure Policy

## Meta
- Status: fixed
- Severity: high
- Discovered in: B0059, GeoIP provider audit
- Category: security-correctness
- Canonical task: [`tasks/todo/make-geoip-lookup-failures-follow-failure-policy/`](../../../todo/make-geoip-lookup-failures-follow-failure-policy/make-geoip-lookup-failures-follow-failure-policy.md)
- Closed: 2026-09-04 — `GeoLookup::Failed` follows existing `failOpen`; see [`skills/04-review/observability/geoip-lookup-failures-follow-fail-open.md`](../../../../skills/04-review/observability/geoip-lookup-failures-follow-fail-open.md)

## Evidence
`GeoIpProvider::lookup` previously mapped both MMDB record decode errors and caught reader panics to `GeoLookup::NotFound`. HTTP and stream consumers interpreted `NotFound` as ordinary missing data rather than database failure, so a malformed or incompatible lower-trust record could bypass the consumer's existing `failOpen` decision.

## Resolution
Decode errors and caught panics are now payload-free `GeoLookup::Failed`. HTTP rule mode and the Stage-1 stream consumer apply the existing `failOpen` contract. Access logs use bounded `lookup_failed`. L4 fail-close uses `reason=lookup_failed`. Successful misses remain `NotFound`; an unloaded database remains `Unavailable`.
