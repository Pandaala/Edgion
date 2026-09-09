# HTTP PluginStore Request-Generation Snapshot

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Design a coherent per-request PluginStore snapshot so global, Gateway, route, gRPC, and backend extension references cannot observe different same-kind publication generations during one request. The challenge is choosing the capture boundary and lifetime without pinning unnecessary maps across awaits or weakening hot reload.

## Requirements

- Define one explicit PluginStore generation/view contract for all plugin-reference tiers used by a request.
- Decide whether capture occurs before preflight or later, accounting for the await boundary and CORS inspection of route references.
- Include backend extension references resolved after routing, not only the vectors built by global plugin resolution helpers.
- Preserve declaration order, runtime Arc identity, dangling-reference behavior, route positional fail-close semantics, and atomic writer-side publication.
- Add deterministic concurrency tests for publication changes between resolution stages.

## Scope

### In scope

- PluginStore snapshot/view API and request-context ownership.
- Global, Gateway, HTTP/gRPC route, and backend extension-reference consumers.
- Capture timing, hot-reload visibility, bounded retained state, and concurrency verification.

### Out of scope

- Unrelated singleton ownership and local-test-store cleanup.
- Changes to existing `edgion-tests/**` files without explicit authorization for those existing test paths.
- Plugin execution-order or failure-policy redesign.

## Affected modules

| Module | Impact |
|--------|--------|
| Plugin runtime store | Expose an immutable coherent generation view. |
| HTTP request context and filter lifecycle | Capture and retain the chosen generation at the defined boundary. |
| Plugin reference resolvers | Resolve every tier through the same view. |
| Tests | Prove deterministic behavior across concurrent publication. |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | Capturing only inside one helper would mask intra-vector mixing while leaving tier, preflight, and backend generation mixing unresolved. |
