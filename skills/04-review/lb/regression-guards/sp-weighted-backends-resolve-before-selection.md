---
name: sp-weighted-backends-resolve-before-selection
description: Use when reviewing session-persistence findings about weighted/multi-BackendRef routes — token resolution now runs BEFORE weighted backendRef selection across ALL of the rule's backendRefs (GEP-1619), the token is a 64-bit truncated SHA-256, and Internal jumps override the sticky session without rewriting it. Covers what re-reports and alternative designs to reject.
status: fixed
finding_id: session-persistence-weighted-backends
closed: 2026-07-18
knowledge_type: regression-guards
---

# Session persistence across weighted BackendRefs — resolve-before-selection — FIXED

## Conclusion

The old HTTP/gRPC upstream-selection order was: extract token → weighted-RR pick a
backendRef → resolve the token only inside that Service's affinity map. With multiple
weighted backendRefs a 50/50 split re-picked a Service per request, the token missed
~50% of the time, and the session bounced between Services with a token rewrite on
every miss — violating GEP-1619's MUST ("persistence to a single backend MUST be
maintained across services, even if the weight is set to 0").

Fixed (2026-07-18) by moving resolution BEFORE weighted selection and widening its
domain to the whole rule: `sp_resolve_target_rule_scoped`
(`edgion-gateway/src/routes/http/session_persistence.rs`) walks the rule's FULL
backendRef list (`unit.backends`, declaration order, weight-0 refs included) and looks
the token up in each Service's existing per-service affinity map; a hit selects THAT
backendRef outright (weighted RR is skipped), a full miss falls back to weighted RR +
token refresh. Both call sites reordered: `select_http_backend`
(`pg_upstream_peer.rs`) and `handle_grpc_upstream` (`grpc/integration.rs`, which
gained a `backends` full-list field on `GrpcRouteRuleUnit`). The token widened from
CRC32/8-hex to truncated SHA-256/16-hex (single derivation: `generate_session_token`,
also used by `build_sp_affinity_map`) because a 32-bit identity was too
collision-prone once it selects across Services.

This is deliberately NOT the Envoy-exact design (cookie = decodable
`base64(ip:port)` + membership check): the one-way hash keeps pod addresses out of
client cookies (product decision, 2026-07-18). Suggestions to "switch to Envoy's
decodable token / delete the affinity maps" are **not accepted** without a new
product decision on topology disclosure.

## Core Rationale

**1. GEP-1619 makes weights new-session-only; conformance asserts it.**

Weights govern only requests without a token. An established session keeps routing
to its Service/endpoint even at weight 0. Envoy Gateway implements this by merging
all backendRefs into one Envoy cluster (per-backendRef weighted localities) so the
stateful-session host override spans the union; Edgion gets the same semantics by
widening the resolution domain to the rule instead of merging LB structures.

**2. Rule scoping is structural, not encoded in the token.**

The walk only visits the matched rule's own backendRefs, so two rules sharing the
default session name with disjoint backends cannot adopt each other's sessions (the
foreign token misses → refresh). Rules referencing the SAME Service resolve to the
same pod — that matches Envoy Gateway and is intended, not "accidental sharing". Do
not accept fixes that mix a rule identity into the token/affinity keys (per-rule map
duplication, invalidation complexity) absent a real cross-rule-leak report on
disjoint backends.

**3. Precedences and fallbacks are pinned.**

- `DynamicInternalUpstream` (Internal jump) wins over the sticky session for that
  request and leaves the session binding untouched (no resolution, state stays
  `Inactive`, no cookie rewrite).
- A sticky hit whose backendRef is ReferenceGrant-denied is treated as a miss
  (sticky target cleared, `InjectNew`, weighted fallback).
- Sticky-endpoint-unhealthy fallback / `strict` fail-close semantics are unchanged
  (`get_peer_with_sp_fallback`).

## Fix Suggestions Not Accepted

- "Adopt Envoy's decodable `base64(ip:port)` token and delete the affinity maps" —
  leaks pod `IP:port` to clients; explicitly rejected in this task's design round.
- "Use a hash ring / consistent hashing instead of exact affinity" — the session
  must represent the exact prior selection (task design constraint).
- "Keep 32-bit CRC32 (collision negligible)" — rejected; the token now selects
  across Services, and the task pinned a ≥64-bit identity.
- "Two rules with the same Service resolving to the same pod is a leak" — intended,
  matches Envoy Gateway; see Rationale 2.
- "Add HMAC to the widened token" — still rejected per
  [crc32-session-affinity-not-a-security-boundary.md](../intentional-design/crc32-session-affinity-not-a-security-boundary.md).

## Re-evaluation Triggers

Re-open only if:

- Session persistence is promoted to an access-control boundary (then the HMAC
  decision reopens first).
- A real cross-rule session-leak report on rules with DISJOINT backends.
- A product decision accepts exposing pod addresses in tokens (then the
  Envoy-exact decode-and-validate design becomes the simplification).

## Reference Cases

- Task: `tasks/todo/session-persistence-weighted-backends.md` (P0, closed 2026-07-18).
- GEP-1619 "Session Persistence and Traffic Splitting" (gateway-api.sigs.k8s.io/geps/gep-1619).
- Integration: `SpWeighted` dynamic scenario (HTTP: split / sticky-at-weight-0 /
  endpoint-delete fallback / rule-scope isolation) and
  `GRPCRoute/Backend/SessionPersistenceWeighted` static suite.
- Unit: `test_sp_lookup_rule_scoped_across_services` (`session_persistence.rs`).
- Docs: `docs/{en,zh-CN}/user-guide/http-route/resilience/session-persistence.md`
  ("Traffic Splitting" section).

## Resolution references

- `session-persistence-weighted-backends` — 2026-07-18.
