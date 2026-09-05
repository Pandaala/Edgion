# D0052 — Canonicalize Auth Claim-to-Header Projection

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0085, JwtAuth audit
- Category: deterministic-header-ownership
- Canonical task: [`tasks/todo/canonicalize-auth-claim-header-projection.md`](../../../todo/canonicalize-auth-claim-header-projection.md)

## Evidence
JwtAuth and OpenID Connect represent claim-to-header mappings as HashMaps. The shared request helper iterates that nondeterministic order, replaces destination headers, and stops after the aggregate 64 KiB cap. Duplicate case-insensitive destinations can therefore produce a nondeterministic final value, and oversized configurations can produce a nondeterministic accepted subset.

## Why deferred
The correction needs one construction-time ordered projection plan, normalized unique destination ownership, and coordinated resource validation for both JwtAuth and OpenID Connect. Sorting only in the request path would add hot-path work without resolving duplicate ownership. This does not reopen D0037/D0038: those closed as overwrite-written-names only. Skip-write leftovers on a configured claim destination remain in this task's scope if they are pursued.

## Integration-test impact
No integration-test issue is required. Resource validation, shared-helper mocks, and deterministic cross-consumer unit tests can prove order, uniqueness, and byte-cap behavior while existing integration tests remain unchanged.
