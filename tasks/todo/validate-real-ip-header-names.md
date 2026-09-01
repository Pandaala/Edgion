# Validate RealIp Header Names Canonically

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness-security |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0098; reuse the canonical field-name seam from D0063 if available |

## AI guide summary

Validate configured RealIp header names once at the resources boundary so global and plugin extraction reject invalid HTTP field names with Controller/Gateway preparse parity.

## Requirements

- Inventory every global and plugin `RealIpConfig` consumer before changing admission.
- Reuse the canonical HTTP field-name token and configured-header size rules; do not add a RealIp-local parser.
- Preserve X-Forwarded-For, X-Real-IP, Forwarded, custom-header, recursive, trusted-proxy, and max-trusted-hop semantics.
- Keep `DirectPeerIp`, PROXY-effective downstream peer, and current-layer `RemoteIp` distinct.
- Produce stable, non-sensitive validation errors and update English and Chinese RealIp documentation atomically.
- Keep existing integration tests unchanged unless implementation proves a gap that deterministic resource, preparse, extractor, and plugin tests cannot cover; create a separate integration issue if that occurs.

## Acceptance criteria

- Invalid, oversized, or non-token configured names fail resource validation before publication.
- Controller and Gateway preparse outcomes match for global and plugin configurations.
- Colocated tests cover defaults, valid custom names, invalid tokens, and the configured size bound.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer from B0098. | Header-name admission crosses resources, global and plugin consumers, documentation, and preparse parity; a hot-path filter would preserve invalid accepted configuration. |
