# Redact ACME DNS Provider Errors

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | security |
| Priority | P1 |
| Issue | N/A |
| Source | D0036 credential ownership audit |

## AI guide summary

Replace arbitrary DNS-provider response and dependency error text in ACME diagnostics with bounded classifications while retaining safe Controller-owned context needed to operate DNS-01 flows.

## Requirements

- Inventory Cloudflare, AliDNS, Pebble/challtestsrv, propagation, create, and cleanup error paths from provider response parsing through status and Controller log sinks.
- Map provider-controlled bodies, JSON error arrays/messages, transport/parser text, and other unbounded detail to a fixed, bounded error classification.
- Retain only reviewed safe context such as provider kind, operation, bounded HTTP status, resource identity, and timeout class; apply existing redaction or masking rules to domain and other potentially sensitive values.
- Keep observability owned by the Controller binary. Do not add Gateway access-log fields, data-plane tracing, new high-cardinality metric labels, or duplicate per-operation logs.
- Preserve DNS request/body caps, deadlines, retry/backoff, propagation behavior, best-effort cleanup, credential resolution, and provider semantics.
- Reuse a shared classification/mapping seam where provider implementations need the same behavior rather than introducing divergent provider-local policies.
- Do not modify existing integration tests; track any genuinely necessary new end-to-end scenario separately.

## Scope

### In scope

- ACME DNS provider errors and every diagnostic/status sink that can receive them.
- Focused unit tests proving provider-controlled text cannot reach those sinks.

### Out of scope

- DNS credential ownership or zeroization redesign.
- ACME protocol, issuance, propagation, timeout, or retry-policy changes.

## Acceptance criteria

- Provider-controlled response bodies and error messages cannot appear in Controller logs or resource diagnostics.
- Failures retain bounded, actionable classifications and reviewed safe context without credential, challenge value, endpoint, or unmasked domain leakage.
- Create and cleanup paths have deterministic tests for representative provider, transport, parse, status, oversize, and timeout failures.
- Controller format/check/Clippy and documentation, logging, and metrics guards pass; existing integration tests remain unchanged.
