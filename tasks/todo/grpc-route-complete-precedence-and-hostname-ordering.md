# Complete GRPCRoute Precedence and Hostname Ordering

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and implement one complete GRPCRoute winner-selection order across every simultaneously matching candidate. The current matcher uses fixed service/method buckets before request-dependent hostname and header checks, so correcting precedence requires an end-to-end design rather than another isolated bucket adjustment.

## Requirements

- Reconfirm the authoritative Gateway API GRPCRoute precedence contract and express it as one reusable total-order model covering applicable hostname specificity, service length, method length, header-match count, creation timestamp, namespace/name, rule index, and match index.
- Audit `GrpcMatchEngine`, `GrpcRouteRuleUnit::deep_match`, listener/Gateway attachment, `GrpcRouteRules`, route compilation and publication, and the HTTP fallback boundary before changing the selection algorithm.
- Prove whether the fixed Exact/RegularExpression tier traversal can select a lower-precedence route before a higher-precedence candidate whose hostname or headers also match the request.
- Preserve Exact and RegularExpression match semantics, invalid-regex handling, listener attachment, ReferenceGrant decisions, immutable snapshot publication, and the existing first-success behavior for genuinely ordered candidates.
- Keep all reusable ranking work at configuration-build time. If any precedence input is request-dependent, minimize the request-path work and do not add string allocation, regex compilation, blocking synchronization, or repeated comparison of request-independent fields.
- Reuse one canonical precedence representation. Do not create separate partially overlapping comparators in the matcher, deep-match layer, or route manager.
- Preserve gRPC-first routing with HTTPRoute fallback when no GRPCRoute matches; explicitly test overlapping HTTPRoute and GRPCRoute hostnames without introducing a new cross-type rejection policy.
- Do not add request-path tracing, parallel logs, or metrics. Gateway traffic diagnostics remain owned by the canonical access log and must not expose headers, route configuration, or other sensitive request data.
- Add deterministic focused unit coverage for every ordering field and for overlapping candidates that cross current Exact/RegularExpression buckets.
- Add live overlap coverage only under separate authorization. Existing `edgion-tests/**` files remain frozen unless their exact paths are explicitly authorized.

## Scope

### In scope

- Complete GRPCRoute candidate precedence from immutable route construction through request winner selection.
- Hostname, service, method, header, route-age, route-identity, rule, and match ordering interactions.
- Focused in-crate tests and a separately authorized standalone integration scenario if unit coverage cannot observe the full publication boundary.

### Out of scope

- Changing Gateway API schemas, listener attachment policy, ReferenceGrant semantics, backend selection, retries, timeouts, or HTTPRoute precedence.
- Introducing hostname-conflict rejection between HTTPRoute and GRPCRoute.
- Modifying existing integration-test files without explicit later authorization.

## Acceptance criteria

- Every pair of simultaneously matching GRPCRoute candidates is resolved by one documented Gateway API precedence chain, independent of insertion or hash iteration order.
- A fixed Exact/RegularExpression bucket boundary cannot override a more authoritative hostname, service, method, or header winner.
- Request-independent ranks are prepared once per immutable generation, and the request path performs only the work required by genuinely request-dependent inputs.
- Deterministic tests distinguish every precedence field, cross-tier overlap, hostname overlap, and final route/rule/match tie-break.
- gRPC-to-HTTP fallback, deep-match failure continuation, publication ownership, access logging, and existing integrations remain unchanged.
- Workspace format/check/Clippy, documentation validation, and repository static guards pass.
