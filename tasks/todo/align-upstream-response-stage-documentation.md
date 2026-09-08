# Align Upstream Response Stage Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | documentation |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Align shared plugin guides with the actual Pingora final-response-header lifecycle without changing runtime behavior.

## Requirements

- Correct `skills/01-architecture/02-gateway/05-plugin-system.md` so `UpstreamResponse` is not described as running after the complete response.
- Correct `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` so the stage is not called response-end or described as following the full response body.
- State that interim 1xx responses are skipped, the hook runs before downstream header commit and body streaming, and complete main-response aggregation is unavailable.
- Keep synchronous header filtering, retry/normalization, async header decisions, bounded streaming processors and trailer handling as distinct lifecycle phases.
- Validate all links and phase examples against the current Gateway driver.

## Acceptance criteria

- Both guides describe the same final-response-header phase as the runtime and trait contract.
- No text implies that the complete upstream response body is available.
- Agent-document validation passes.
