# Fail closed on denied L4 backend references

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | enforcement done; e2e follow-up remains |
| Type | security bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

TCPRoute, UDPRoute, and both TLSRoute proxy modes must honor the Controller-resolved `ref_denied` marker after selection, and they must keep denied plus non-Service backendRefs in the weighted selector so those slots fail their declared share. Preload filtering is only a startup optimization and is not an authorization boundary. Enforcement is in place; data-plane e2e / grant hot-reload coverage remains a follow-up.

## Requirements

- Establish the canonical Gateway API behavior for a weighted rule containing both valid and invalid backend references, then preserve that selection/error proportion consistently across HTTP, gRPC, TCP, TLS termination, TLS passthrough, and UDP.
- Add a typed fail-close check immediately after L4 backend selection and before Service-key construction, EndpointSlice lookup, port resolution, policy lookup, logging of a target, or dialing.
- Do not treat skipping a denied reference during preload as runtime enforcement.
- Ensure full and partial route publications, ReferenceGrant revocation, and delete-to-create coalescing replace stale authorization state atomically with the route generation.
- Use one shared L4 backend-ref authorization helper where protocol semantics are identical; do not copy three ad hoc checks.
- Keep client-visible and connection-log failure reasons fixed and low sensitivity. Do not expose ReferenceGrant content, target topology, or internal dependency detail.

## Scope

### In scope

- TCPRoute, UDPRoute, TLSRoute termination, and TLSRoute passthrough backend selection and final pre-dial validation.
- Route-unit selector construction only where required to preserve the canonical weighted-invalid-backend behavior.
- Static and hot-reload tests for ReferenceGrant allow, denial, revocation, and recreation.

### Out of scope

- Controller ReferenceGrant resolution itself.
- HTTP or gRPC routing changes except shared parity tests.
- Backend port-resolution redesign, which is tracked in `tasks/todo/backend-port-resolution-precompiled-fail-closed.md`.

## Acceptance criteria

- A denied TCPRoute, UDPRoute, or TLSRoute backend reference can never reach Service lookup or a dial attempt.
- A forged/stale selector entry carrying `ref_denied` is rejected by the final pre-dial guard.
- Mixed valid/denied weighted backend rules follow the documented canonical invalid-backend traffic proportion; tests prevent silently dropping denied weight or retrying it onto an allowed backend unless the specification explicitly requires that behavior.
- Adding a valid ReferenceGrant enables the backend after publication; revoking it disables the backend without requiring a route restart.
- Full and partial updates plus delete-to-create coalescing cannot retain a previously authorized backend generation.
- HTTP, gRPC, TCP, TLS termination, TLS passthrough, and UDP parity tests cover denied cross-namespace references.
- Logs and diagnostics use a fixed reason and contain no Secret, ReferenceGrant, or endpoint-list detail.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/00-common/05-unification-points.md`
- `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
- `tasks/todo/backend-port-resolution-precompiled-fail-closed.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the preload audit. | The defect spans three route-unit selectors, four connection paths, weighted invalid-backend semantics, and hot-reload authorization generations. A preload-only filter reduces wasted work but cannot enforce the final dial boundary. |
| 2026-09-06 | Implement the post-select fail-close gate only. | Shared `l4_backend_ref_denied` plus the four dial paths. Selectors still retain denied weight. TLS retries return instead of continuing. Data-plane e2e / grant hot-reload / cross-protocol parity tests remain follow-up. An already-pinned UDP session is not torn down on grant revocation; that is existing session-pin behavior, not this gate. |
| 2026-09-06 | Keep unsupported L4 kinds in the selector and fail after select. | Aligns L4 with Gateway API weighted-invalid-backend share. Shared `classify_l4_selected_backend` / `l4_post_select` covers Denied and UnsupportedKind. TLS `return`s on both. TCP/UDP/TLS logs use `UnsupportedBackend`. Mixed-weight unit tests lock the three-slot count. e2e remains follow-up. |
