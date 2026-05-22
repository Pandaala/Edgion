---
name: tls-02-deprecated-tls-version-admission
description: Accepted RFC 8996 §2 divergence — Edgion keeps TLS 1.0/1.1 as an operator opt-in via min_tls_version and does not hard-reject; a config-time warn surfaces the deprecation. Reject re-reports proposing admission rejection or a per-handshake warning.
---

# tls-02 — TLS 1.0/1.1 remain admissible via `min_tls_version`

**Status:** ⚠️ accepted divergence (config-time warn added) · 2026-05-20
**RFC:** RFC 8996 §2-5 (MUST NOT negotiate)

## Conclusion

`min_tls_version: TLS1_0 / TLS1_1` stays a valid EdgionTls configuration — an
operator opt-in escape hatch for legacy interop. Edgion does NOT hard-reject it.
`validate_cert` emits a single config-time `tracing::warn!` (RFC 8996) when one of
these values is set; the default (unset) remains BoringSSL TLS 1.2+.

Suggestions of the form "reject TLS 1.0/1.1 at admission" or "warn inside
`map_tls_version`" are **not accepted** — the latter is on the per-handshake
data-plane hot path (see [../../03-coding/01-log-safety.md](../../03-coding/01-log-safety.md)).

## RFC clause

> "This document formally deprecates Transport Layer Security (TLS) versions
> 1.0 (RFC 2246) and 1.1 (RFC 4346). Accordingly, those documents have been
> moved to Historic status." — RFC 8996 §2

## Code shape today

- `types/resources/edgion_tls.rs:30-42` — `TlsVersion` enum keeps `Tls10`/`Tls11`; doc comment marks them RFC 8996-deprecated, legacy-only.
- `tls/validation/cert.rs` (`validate_cert`) — config-time `tracing::warn!` when `min_tls_version` is `Tls10`/`Tls11`; adds no error (resource stays valid). Regression test `test_deprecated_min_tls_version_is_not_rejected`.
- `tls/runtime/gateway/tls_pingora.rs:356-365` (`map_tls_version`) — unchanged; no tracing on the handshake path.

## Why we diverge

1. **Operator escape hatch.** Legacy IoT / older payment terminals still need TLS 1.0/1.1; removing the option breaks documented migration paths. Same posture as Envoy (keeps `TLSv1_0/1_1` config; default raised to TLS 1.2 in 1.16) and BoringSSL (library still supports them).
2. **Secure by default.** Unset `min_tls_version` → BoringSSL TLS 1.2+; the unsafe path is explicit opt-in only.
3. **Hot-path prohibition.** The finding's suggested per-handshake warn lives in `map_tls_version` (data plane); the deprecation is surfaced at config-load time instead.

## Re-evaluation triggers

- Gateway API introduces a standard minimum-TLS-version field that mandates rejection.
- BoringSSL drops TLS 1.0/1.1 support upstream (the option becomes inert).
- An operator-facing decision to drop legacy interop entirely.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a4.md → V03` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md → tls-02](../CLOSED-FINDINGS.md)
