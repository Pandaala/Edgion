---
name: tls-07-revocation-config-not-implemented
description: Accepted RFC 5280 §6.3 / RFC 6960 divergence — EdgionTls revocationCheck/ocspStapling are reserved-but-unimplemented schema; kept (not hard-rejected) with a config-time warn + explicit doc comments. Reject re-reports proposing admission rejection or schema removal.
---

# tls-07 — `revocationCheck` / `ocspStapling` are reserved schema, not implemented

**Status:** ⚠️ accepted divergence (config-time warn + doc comments added) · 2026-05-22
**RFC:** RFC 5280 §6.3 (CRL), RFC 6960 (OCSP) — informative (no MUST binds a TLS gateway to perform revocation)

## Conclusion

`EdgionTlsExtend.revocationCheck` and `EdgionTlsExtend.ocspStapling` stay valid
EdgionTls configuration but perform NO runtime work — the gateway does no OCSP/CRL
lookups and no OCSP stapling regardless of the values. The fields are kept (reserved
for a future release) and NOT hard-rejected. `validate_cert` emits a single
config-time `tracing::warn!` when `revocationCheck.mode != off` or
`ocspStapling.enabled == true`; the doc comments state "NOT IMPLEMENTED" explicitly.

Suggestions of the form "reject `mode != off` at admission", "strip / remove the
fields from the CRD schema", or "add a per-handshake revocation check warning" are
**not accepted**. Admission rejection would fail a whole EdgionTls resource (taking
down a working TLS listener) over a no-op field; a per-handshake warn lands on the
data-plane hot path.

## RFC clause

> "If a cRLDistributionPoints extension is present, the procedure described in
> Section 6.3 is used to determine the status of the certificate." — RFC 5280 §6.3.3

RFC 6960 defines online (OCSP) revocation status checking. Neither RFC contains a
MUST requiring a TLS gateway to perform revocation checking; the defect is a
false-advertised feature, not an RFC text violation.

## Code shape today

- `types/resources/edgion_tls.rs` — `RevocationCheckConfig` / `RevocationMode` /
  `OcspStaplingConfig` retained; doc comments now say "NOT IMPLEMENTED … reserved".
- `tls/validation/cert.rs` (`validate_cert`) — config-time `tracing::warn!` when
  `revocationCheck.mode != Off` or `ocspStapling.enabled`; adds no error (resource
  stays valid). Regression test `test_unimplemented_revocation_config_is_not_rejected`.
- `src/core/gateway/tls/**` — no consumer of `revocation_check` / `ocsp_stapling`
  (the whole `EdgionTlsExtend` block is reserved schema).

## Why we diverge

1. **Reserved schema for a future feature.** The entire `EdgionTlsExtend` block
   (revocation, stapling, session ticket/cache, early data) is documented as reserved;
   removing the fields would break the CRD schema for operators already carrying them
   and discard the reserved API surface.
2. **Do not break working listeners.** The field was always a no-op; rejecting it at
   admission converts "no-op field" into "resource rejected", which can take down a
   functioning TLS listener — a real regression. Surfacing the gap at config-load is
   the honest, non-destructive middle ground. Same posture as
   [tls-02-deprecated-tls-version-admission.md](tls-02-deprecated-tls-version-admission.md).
3. **Hot-path prohibition.** A per-handshake revocation warn lives on the data plane;
   the no-op is surfaced at config-load time instead (see
   [../../03-coding/01-log-safety.md](../../03-coding/01-log-safety.md)).

## Re-evaluation triggers

- OCSP/CRL revocation checking is actually implemented in the gateway TLS runtime
  (then the warn/doc divergence is removed and the fields become live).
- A compliance certification (PCI-DSS / FIPS) requires the proxy layer to reject
  configs that imply unmet revocation guarantees.
- Gateway API introduces a standard revocation field with mandated semantics.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a4.md → V11` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md → tls-07](../CLOSED-FINDINGS.md)
- Sibling pattern: [tls-02-deprecated-tls-version-admission.md](tls-02-deprecated-tls-version-admission.md)
