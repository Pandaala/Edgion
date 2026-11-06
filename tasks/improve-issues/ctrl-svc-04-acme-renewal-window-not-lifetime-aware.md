# ctrl-svc-04: ACME renewal window is an absolute 30 days and ignores certificate lifetime and ARI

- **Category**: best-practice
- **Severity**: P3
- **Scope**: local
- **Location**:
  - `edgion-controller/src/services/acme/service.rs:856-873` (`needs_renewal`: `now >= notAfter - renewBefore`)
  - `edgion-resources/src/resources/edgion_acme.rs:71-105` (`renewBefore` default 720h, `checkInterval`, `failBackoff`)
  - `edgion-controller/src/services/acme/acme_client.rs:183-235` (order creation: no profile, no `replaces`)

## Summary
Renewal is triggered at `notAfter - renewBefore` with a fixed default of 30 days and nothing ties
`renewBefore` to the issued certificate's lifetime. CA-issued lifetimes are shrinking (Let's Encrypt
is moving its default from 90 to 64 and then 45 days, and offers a 6-day "shortlived" profile). For
any lifetime ≤ `renewBefore`, `needs_renewal` is true immediately after issuance, so every
`checkInterval` (24 h) reissues the certificate, burning duplicate-certificate rate limits. The
controller also does not use ACME Renewal Information (ARI, RFC 9773), which lets the CA steer
renewal (including mass-revocation events) and exempts ARI-guided renewals from LE rate limits.

## Current behavior (evidence)
`needs_renewal` parses `status.certificateNotAfter` and compares it to a duration parsed from
`spec.renewal.renewBefore` (default `720h`). There is no validation that `renewBefore` is smaller
than the certificate validity and no lifetime-relative default. Orders are created with plain
`NewOrder { identifiers }`; no `replaces` field and no `renewalInfo` lookup.

## Industry comparison
- **cert-manager**: default renewal at 2/3 of the actual lifetime; `renewBefore` larger than the
  duration is rejected/clamped; `renewBeforePercentage` exists.
- **Caddy/certmagic, lego, Traefik ≥ 3.2**: implement ARI and use the CA-suggested window,
  falling back to "1/3 of lifetime remaining".

## Impact
Today mostly latent (LE default is still 90 days), but it becomes a reissue loop for short-lived
profiles or CAs with ≤ 30-day certificates and misses CA-driven early renewal on revocation events.

## Suggested direction
Compute the renewal point as `min(notAfter - renewBefore, notBefore + 2/3 * lifetime)` (or make the
default lifetime-relative) and reject/clamp `renewBefore >= lifetime` at issuance time. Add ARI:
fetch `renewalInfo` on each renewal check, schedule within the suggested window, and send
`replaces` on the renewal order (instant-acme supports both).

## Notes
`skills/04-review/resources/intentional-design/gep2257-duration-field-scope.md` covers duration
format only. Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `edgion-controller/src/services/acme/service.rs:856-873`: `now >= expiry - renew_before`, default `720 * 3600` s.
- No validation that `renewBefore` < certificate lifetime; for ≤30-day certs (e.g. LE short-lived profile) every check reissues.
- Latent with current 90-day LE default — P3 correct.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/services/acme/service.rs:856`: `needs_renewal` still `now >= notAfter - renewBefore`; `services/` unchanged since `feature-08-30`.
- `edgion-resources/src/resources/edgion_acme.rs` unchanged (default `renewBefore` 720h); no ARI / `replaces` in `acme_client.rs`.
