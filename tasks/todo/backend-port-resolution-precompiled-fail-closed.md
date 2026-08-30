# Precompile backend port resolution and fail closed on missing mappings

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Move Service-port to selected-endpoint-port resolution out of the Gateway request path and make unresolved requested ports an explicit failure. `resolve_target_port_key` currently reads the live Service snapshot, takes two blocking EndpointSlice `RwLock` read guards, clones every slice for the Service, and scans endpoint addresses for each selected request. When the Service, requested port, originating slice, port name, or target port cannot be resolved, it silently leaves the load balancer's existing port unchanged. For a multi-port Service that port may belong to a different application. The fallback for an unknown originating endpoint also chooses the minimum matching port across heterogeneous slices, which is deterministic but not proof that the selected endpoint serves that port.

## Requirements

- Resolve and validate the requested Service port against the exact selected EndpointSlice/backend generation during configuration publication or selector-cell construction.
- Carry enough immutable per-backend port identity to select the requested port without reading `ServiceStore` or `EpSliceStore`, cloning Kubernetes resources, scanning endpoints, or taking a blocking lock on the request path.
- Return a typed outcome that distinguishes no requested port, a resolved port, and an unresolved/invalid requested port.
- Fail the request or stream connection closed when `backendRef.port` is present but its Service port or selected endpoint port cannot be resolved. Never retain a first-port or stale-port value as an implicit fallback.
- Preserve per-slice named-targetPort semantics during heterogeneous rollouts. An unknown origin may use a fallback only when all applicable slices prove the same port; otherwise fail closed.
- Validate all K8s `i32` port values with checked conversion and the valid non-zero `u16` range; do not use truncating `as u16` casts.
- Coordinate Service port-name capture for BackendTLSPolicy with `tasks/todo/backend-tls-policy-defensive-validation-fail-closed.md` so an unresolved section mapping cannot become an absent TLS policy.
- Preserve IPv4/IPv6 numeric address equality and the existing post-resolution unsafe-address check.
- Apply identical resolution and failure semantics to HTTP, TCP, TLS passthrough/termination, UDP, and request-mirror consumers.

## Scope

### In scope

- Service and EndpointSlice publication data needed by selector cells.
- `resolve_target_port_key` and `resolve_port_name` replacement APIs.
- All Gateway consumers of selected Service backend ports.
- Focused hot-reload, heterogeneous-slice, invalid-port, and protocol-parity tests.

### Out of scope

- Route backend selection, weights, or retry counts.
- External DNS/EdgionBackend endpoint resolution.
- BackendTLSPolicy validation beyond port/section lookup integration.

## Acceptance criteria

- Request-path instrumentation proves no Service/EndpointSlice lock, Kubernetes resource clone, endpoint scan, formatting allocation, or string-to-IP parse occurs during port selection.
- A requested port with a missing Service snapshot, unknown Service port, absent endpoint port, invalid numeric range, or ambiguous heterogeneous origin produces a deterministic fail-close result before dialing.
- Each endpoint in heterogeneous sibling slices resolves a shared named port to its own slice's numeric port.
- Full and partial Service/EndpointSlice publications replace stale mappings atomically and captured selector generations remain internally coherent.
- HTTP, TCP, TLS passthrough/termination, UDP, and request-mirror tests prove that no unresolved path dials the load balancer's first port.
- BackendTLSPolicy exact-section lookup receives either the canonical port name or an explicit failure, never an ambiguous `None`.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/05-resources/16-service-endpoints.md`
- `skills/04-review/backends/named-port-resolve-against-originating-slice.md`
- `tasks/todo/backend-tls-policy-defensive-validation-fail-closed.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the file audit. | A local `Result` return would improve signaling but retain blocking locks, full resource clones, endpoint scans, and missing per-backend identity; every protocol caller and selector publication must migrate together. |
| 2026-08-31 | Preserve the heterogeneous-slice closed finding. | This task retains exact originating-slice resolution and numeric IPv6/IPv4 comparison; it replaces only the unproven unknown-origin fallback and hot-path lookup mechanism. |
