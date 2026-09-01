# Use Clone-cheap Immutable Local Response Bodies

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | architecture-performance |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0089 |

## AI guide summary

Design one canonical local-response body representation that lets generation-owned static bodies cross `PluginRunningResult::ErrResponse` without a deep clone on every request while preserving dynamic responses and exact response-writer semantics.

## Requirements

- Inventory every `ErrResponse` producer and every request- and response-stage consumer before selecting a representation.
- Prefer one immutable clone-cheap type, or an enum that clearly separates shared immutable and dynamically owned bodies; do not add plugin-local wrappers or a second response cache.
- Preserve exact bytes, empty-versus-absent body semantics, HTTP framing, final-status validation, response replacement, gRPC error mapping, and writer-error behavior.
- Keep dynamically constructed fail-close bodies generic and ensure no dependency detail, address, trace, token, credential, or configured sensitive value reaches a public response.
- Do not introduce response aggregation, response-side `BodyStorage`, unbounded buffering, or an allocator-sensitive behavioral contract.
- Update all producers and consumers atomically; Edgion is pre-release, so do not retain a compatibility shim.

## Acceptance criteria

- A generation-owned static local-response body can be reused across requests with bounded clone cost and without copying its complete byte content before the response writer.
- Dynamic response bodies remain supported without an unnecessary second full copy.
- Unit tests cover absent, empty, static, dynamic, HTTP, gRPC, writer-failure, and response-replacement paths with exact bytes and framing.
- Existing integration tests remain unchanged unless the approved design introduces an observable wire-contract change that genuinely needs new coverage.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer the representation change from B0089. | The owned body is defined at a cross-crate enum boundary with many producers and consumers; changing Mock alone cannot remove the request-time deep clone. |
