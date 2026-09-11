---
name: geoip-lookup-failures-follow-fail-open
description: Use when reviewing findings that GeoIP decode errors or caught MMDB panics are mapped to NotFound, or that failOpen only covers an unloaded database; covers the typed GeoLookup::Failed outcome and existing failOpen contract.
status: fixed
finding_id: D0025
closed: 2026-09-04
knowledge_type: regression-guards
---

# GeoIP lookup failures follow failOpen — FIXED

## Conclusion

A loaded-database MMDB decode error or caught reader panic is `GeoLookup::Failed`,
not `NotFound`. HTTP rule mode and the Stage-1 stream consumer apply the existing
`failOpen` knob to that outcome. Access logs emit bounded `status: lookup_failed`.
L4 fail-close denials use `reason=lookup_failed`. Ordinary successful misses stay
`NotFound`; an unloaded database stays `Unavailable`; an unparseable client
address stays `NotFound`.

Fix suggestions of the form "map decode errors to `NotFound` so the worker
survives" or "widen `Unavailable` to mean any lookup problem" are **not accepted**.

## Core Rationale

**1. Missingness and failure are different security outcomes.**

`NotFound` feeds `decide()` as empty facts, so a `defaultAction: allow` plus
country deny rule admits the client. `failOpen: false` exists so the control
does not silently bypass when it cannot evaluate. Decode/panic must take that
path.

**2. `Unavailable` means the control is not running.**

`edgion_geoip_db_loaded == 0` is the operator alert for "no database". Folding
a loaded-DB decode failure into `Unavailable` would make that gauge lie and
collapse two operator conclusions.

**3. Observability stays bounded.**

`Failed` is payload-free. The panic counter still counts only panics. HTTP
plugin logs use `lookup:failed fail-open|fail-closed`. No request-path tracing
and no decoder/IP/path labels.

## Fix Suggestions Not Accepted

- Keep mapping decode/panic to `NotFound` and rely on the panic counter —
  the counter does not restore policy.
- Widen `Unavailable` to cover decode/panic — breaks database-loaded meaning.
- Add a fifth fail-open/fail-close config shape — frozen by design rule F.
- Per-request tracing or decoder-detail metrics — hot-path / leak / cardinality.
- Corrupt the shared integration MMDB fixture to exercise this path.

## Re-evaluation Triggers

- A new GeoIP consumer that matches `GeoLookup` exhaustively without treating
  `Failed` as a `failOpen` input.
- A proposal to log decoder `Display` text or the looked-up IP on the failure
  path.

## Reference Cases

- Provider mapping: `edgion-gateway/src/geoip/provider.rs`
- HTTP: `edgion-gateway/src/plugins/http/geoip_location/plugin.rs`
- Stream: `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs`
- Access log: `edgion-gateway/src/observe/access_log/entry.rs`

## Resolution references

- `D0025` — 2026-09-04.
