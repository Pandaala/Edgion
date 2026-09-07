---
name: empty-hosts-san-vacuous-pass
description: validate_cert now reports NoHostsDeclared when spec.hosts is empty, instead of check_san_matches vacuously returning Ok. The guard is on spec.hosts (caller), NOT inside check_san_matches — the suggested in-callee guard would break the InvalidSanPattern invariant. Reject re-reports of the empty-hosts vacuous pass.
knowledge_type: regression-guards
---

# tls-F009 — `check_san_matches` vacuously passes for empty declared hosts

**Status:** ✅ fixed · 2026-05-22
**Severity:** Minor (BadLogic) — latent correctness gap; not exploitable (CRD enforces
non-empty `hosts`, and the matcher skips no-host certs), defense-in-depth only.

## Conclusion

`validate_cert` (`edgion-gateway/src/tls/validation/cert.rs`) now pushes
`CertValidationError::NoHostsDeclared` when `tls.spec.hosts.is_empty()`. Previously, an empty
`spec.hosts` flowed into `check_san_matches` as an empty slice, the `for declared_host in
declared_hosts` loop never ran, `missing_hosts` stayed empty, and `Ok(())` was returned — SAN
validation trivially passed regardless of the certificate's actual SANs.

Re-reports of the form "`check_san_matches` silently passes / returns Ok for empty
`declared_hosts`", "empty `spec.hosts` skips SAN validation", or "SAN match is vacuously
satisfied with no declared hosts" are **not accepted** — verify the current `validate_cert`
guards `tls.spec.hosts.is_empty()` before re-reporting.

## Why the fix is at the caller, NOT inside `check_san_matches`

This is the key subtlety, and why the original issue's "Suggested direction" (add an
`is_empty()` → `SanMismatch` guard at the top of `check_san_matches`) is **wrong**:

The caller passes `valid_hosts` (the subset of `spec.hosts` that survived the
`is_valid_san_pattern` partition), **not** `spec.hosts`. So `valid_hosts` is empty in two
distinct cases:

- **A.** `spec.hosts` itself is empty — the genuine latent defect this fix targets.
- **B.** `spec.hosts` is non-empty but every host is an invalid wildcard (e.g.
  `*.*.example.com`) — already reported via `InvalidSanPattern`; here `valid_hosts` is
  legitimately empty and `check_san_matches` returning `Ok` is **correct** (no redundant
  error).

A guard inside `check_san_matches` cannot tell A from B, so it would make case B emit a
spurious `SanMismatch`, breaking the existing invariant test
`test_validate_cert_reports_invalid_san_pattern` (asserts an invalid pattern must NOT also
produce a `SanMismatch`). Keying the guard on `spec.hosts.is_empty()` at the caller hits case
A precisely and leaves case B untouched.

## Fix suggestions not accepted

- "Add the `is_empty()` guard inside `check_san_matches`" (issue Suggested direction) — breaks
  the `InvalidSanPattern` no-double-report invariant (case B above). The guard must live in
  `validate_cert` on `spec.hosts`.
- "Close as Won't-Fix, the empty-hosts case is unreachable" — the CRD/matcher guards make it
  unreachable today, but the project keeps fail-closed defense-in-depth for schema-bypass /
  future-optional-field scenarios; the fix is one cheap caller-side line.

## Re-evaluation triggers

- A future caller passes `spec.hosts` (not the partitioned `valid_hosts`) into
  `check_san_matches`, or the partition is removed — re-examine whether the in-callee guard
  becomes safe.
- `spec.hosts` becomes optional in the CRD schema (would make this guard the primary boundary,
  not just defense-in-depth).

## Reference cases

- `test_validate_cert_reports_no_hosts_declared` (unit test in `cert.rs`) — empty hosts →
  `NoHostsDeclared`, asserts no spurious `SanMismatch`.
- `test_validate_cert_reports_invalid_san_pattern` — the invariant the in-callee guard would
  have broken (case B).
