---
name: gep2257-duration-field-scope
description: Use when reviewing integer-vs-string duration config fields; covers which fields are GEP-2257 strings and which integer durations are deliberate.
status: fixed
finding_id: acme-outlier-gep2257-conversion
closed: 2026-07-19
knowledge_type: intentional-design
---

# GEP-2257 Duration Field Scope — CONVERTED SET IS CLOSED

## Conclusion

The historical conversion covered seven CR-surface duration fields; the two DNS-01 fields were later removed with DNS-01 on 2026-09-09. Seven fields were converted from integers to
GEP-2257 strings (2026-07-19): `EdgionAcme` `renewal.renewBefore` (was
`renewBeforeDays`), `renewal.checkInterval`, `renewal.failBackoff`, dns-01
`propagationTimeout`, `propagationCheckInterval` (the latter removed
2026-07-20 as a dead knob — see
[acme-propagation-check-interval-removed.md](../../acme/superseded/dns01-retired.md));
`EdgionBackendTrafficPolicy`
`outlierDetection.ejectionTime` (was `ejectionSeconds`) and `maxEjectionTime`
(was `maxEjectionSeconds`). Every other integer duration field in the
workspace keeps its integer form by recorded decision.

Fix suggestions of the form "convert remaining integer duration field X to a
GEP-2257 string for consistency" are **not accepted** without new evidence.

## Core Rationale

**1. The converted set is the operator-ergonomic CR surface**

These seven were the selected CR-surface conversions: a human writes a
duration into a CR and the unit was previously implicit (or baked into a
`*_seconds`/`*_days` name). Other existing operator-facing integer lifetimes
were retained by recorded decision (see the not-converted set below).
Conversion used the existing shared infrastructure:
`edgion-resources/src/duration.rs` (`GEP2257_PATTERN` + `parse_duration`),
schemars `pattern(...)` on the field, CRD `pattern:` + string `default:`, and
`parse_duration` at the consumers (controller `services/acme/service.rs`
`spec_duration_or`; gateway `OutlierDetectionConfig::normalize`).

**2. The not-converted set has anchored reasons**

Remaining integer durations keep integer form for: third-party/spec parity
(Kafka `linger.ms`, etcd clientv3, Pingora `ServerConf`, Gateway API / RFC
integer-seconds fields), bounded ms-granularity knobs with `MAX_*` caps,
family consistency around those anchors, process-local config / internal
resolved structs that are not the CR ergonomic surface, or existing plugin
token/session lifetimes that already used integer seconds at conversion time
(CSRF `expires`, OIDC `sessionLifetime`, JWT `lifetimeGracePeriod`). CSRF
B0070 does not reopen this set — see
[csrf-expires-integer-seconds-retained.md](../../csrf/intentional-design/csrf-expires-integer-seconds-retained.md).
The gateway-internal `CircuitBreakerConfig` twin deliberately stays `u64`
seconds — it is resolved once at `normalize()` and consumed on the data plane.

**3. Parse-failure handling follows the lenient-admission pattern**

Invalid duration strings are rejected at the schema layer
(`OutlierDetectionConfig::get_validation_error`, `EdgionAcmeHandler::validate`
→ `Accepted=False`); the runtime consumers fall back to the documented
defaults with a WARN instead of panicking, because lenient admission can leak
an unvalidated CR to them. Sub-second `ejectionTime` (e.g. `"500ms"`) is
rejected by validation, not truncated to 0.

## Fix Suggestions Not Accepted

- Convert Kafka/etcd/Pingora-adjacent integer duration fields — breaks
  third-party/spec parity the decision anchored on.
- Dual-accept (int-or-string) or deprecated aliases for the seven fields —
  pre-release, no compatibility shims by project policy.
- Truncate sub-second `ejectionTime` to seconds — silently produces a 0s
  ejection; validation rejects instead.

## Re-evaluation Triggers

Re-open this decision only if:

- A new CR-surface duration field is added (decide its form at design time —
  new operator-facing durations should default to GEP-2257 strings).
- One of the third-party anchors (Kafka/etcd/Pingora config shapes) itself
  moves to duration strings.
- Operator feedback shows a specific retained integer field is a recurring
  misconfiguration source.

## Reference Cases

- `tasks/todo/acme-outlier-gep2257-conversion.md` (implementation task,
  closed 2026-07-19) and the integer-duration decision task it followed.
- Sibling conversion: commit `7e18faee` (health-check / backend-traffic
  string-side fields).
- CSRF `expires` adjudication (D0041, 2026-09-05, accepted tradeoff):
  [csrf-expires-integer-seconds-retained.md](../../csrf/intentional-design/csrf-expires-integer-seconds-retained.md).

## Resolution references

- `acme-outlier-gep2257-conversion` — 2026-07-19.
