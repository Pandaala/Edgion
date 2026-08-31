# D0018 — Make Disabled Outbound TLS Inert Before Publication

## Meta
- Status: needs-investigation
- Severity: medium
- Discovered in: B0052, Redis TLS readiness path
- Category: correctness

## Evidence
Redis mapping now ignores custom TLS material when `tls.enabled` is false, but Controller reference resolution and Gateway readiness checks may still inspect CA or client-certificate references from a disabled block and prevent publication before mapping runs.

## Why deferred
The behavior crosses shared resource validation, secret reference resolution, readiness gating, and multiple outbound providers.

## Candidate next step
Trace every outbound TLS consumer and make disabled TLS configuration consistently inert before reference resolution and publication, while preserving scheme-implied TLS where the owning protocol defines it.
