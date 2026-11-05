---
name: resource-grpc-route
description: GRPCRoute resource — gRPC Service/Method matching, gRPC-Web, and the relationship to HTTPRoute.
---

# GRPCRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

GRPCRoute defines gRPC-layer routing rules. Its Controller-side processing is highly aligned with HTTPRoute (sharing route_utils helper functions); the main difference is the matching method on the Gateway side: it matches by gRPC service name and method name rather than by HTTP path.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/grpc/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/grpc/routes_mgr.rs`
- Match unit: `edgion-gateway/src/routes/grpc/match_unit.rs`
- Type definitions: `edgion-resources/src/resources/grpc_route.rs`

## Controller-side processing

### validate

1. Calls `validate_grpc_route_if_enabled()` for header matcher enum/regex validation; cross-namespace validation is handled via `ref_denied` in the parse stage.
   Method validation uses the shared `GRPCRouteMatch::compile_method()` via `GRPCRouteRule::method_match_error()` in per-rule classification and status, for delegation children too. Invalid method predicates drop their whole rule; valid siblings survive, and all-invalid effective rules reject attachment with `Accepted=False / UnsupportedValue`. These errors never enter route-wide advisory validation. Missing outer method is unconstrained; omitted type is Exact; method-only Exact and regex are supported, also in delegation children (the delegating rule itself is restricted, see below). Explicit empty matchers, invalid Exact names, and invalid regexes fail closed with bounded static errors.
   Per-rule header modifier validation uses the shared `GRPCRouteRule::header_modifier_error()`, including backend filters and delegating rules. Invalid rules are excluded while valid siblings survive; all-invalid header rules reject attachment. See [header modifier validation](../../04-review/plugin-runtime/regression-guards/header-modifier-validation.md).
2. Validates the actual Route source; backend target/status checks run through the effective target-only view in `update_status`.

### Native backend reference ownership

`refs::route_fields::GrpcRouteBackendRefs` borrows the actual typed Route. Only an authored,
validated terminal loan can construct a Grant query; effective/status/AI loans,
named/wildcard delegation selectors, and owned dependency receipts cannot. The
canonical `refs::targets` classifier requires omitted/empty group and omitted/exact
`Service` kind plus port `1..=65535`; literal `core` and pseudo Service kinds are
unsupported. Exact same-family delegation uses the Gateway API group and no port;
`*` is a namespace selector, not an object identity. HTTP alone also supports exact
`edgion.io/EdgionBackend` without a backend port.

Source namespace omission defaults to `default`; an explicitly invalid namespace
or missing/invalid source name closes the existing Accepted gate, including direct
status calls, and empties derived executable rules/listeners. No native target is
looked up or registered for that invalid source.

Under delegation every route authorizes and registers only its own authored
backendRefs under its own owner; no rule is cloned across routes. Valid terminal
receipts survive later rule filtering and missing/denied targets. Service manager owners retain their explicit canonical
namespace while root Grant wakeups retain raw metadata namespace representation.
Apply replaces all prior dependencies and requeues dropped policy keys.

### parse

Mirrors HTTPRoute's step order:
0. **Replace ReferenceGrant dependencies and mark denials**: clear the prior CrossNsRefManager set once at parse entry; register each target namespace under the Route owner before reading the ReferenceGrant store; mark `ref_denied` on `spec.rules`. Clear any authored `resolved_delegation_keys`, compute `grpc_route_delegation::grpc_delegation_facts`, and sync the separate `grpc_route_delegation_graph` (same `DelegationGraph` type as HTTP, disjoint keys, `kind: GRPCRoute`). A non-inert child gets its confirmed `resolved_delegation_keys`; the returned requeue set goes through the workqueue. Shared mechanics: [HTTPRoute delegation](04-http-route.md#delegation-cascading-match)
1. Complete the authored target set in CrossNsRefManager without another clear
2. Register Service references in ServiceRefManager
3. Clear and recompute `resolved_listeners` (resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()`)
4. Resolve `resolved_hostnames` (intersection of route hostnames with Gateway Listener hostnames)
5. (ReferenceGrant `ref_denied` marking happens in step 0 — see above)

### on_change

Same as HTTPRoute: register in GatewayRouteIndex, update AttachedRouteTracker, requeue the parent Gateway when needed, and requeue the parents naming this route (`grpc_route_delegation_graph.parents_naming`). The same post-cache-ready one-shot revalidation as HTTPRoute (`ref_grant/revalidation_listener.rs::trigger_grpc_delegation_parent_revalidation`) requeues every gRPC delegating parent, closing the cold-start window where a parent is processed before its children are loaded.

### on_delete

Same as HTTPRoute: clear all reference and index registrations; `grpc_route_delegation_graph.on_route_deleted` removes a participant and requeues its declared children, declared parents and the parents naming it.

### update_status

Same as HTTPRoute: produce a RouteParentStatus for each parentRef and set the Accepted and ResolvedRefs Conditions, plus delegation-specific status surfaces:
- `ResolvedRefs`: canonical target validation scans the route's own rules. Valid delegation selectors are excluded from Service existence reads; malformed selectors are invalid references. Named delegation targets report `BackendNotFound` (missing child) or `RefNotPermitted` (child does not declare this parent). Valid denied terminals retain `RefNotPermitted` without a Service existence read.
- `PartiallyInvalid`: own invalid rules only, including malformed delegating rules and backend-selecting plugins on delegating rules.
- Child-side delegation status: each `kind: GRPCRoute` parentRef gets its own entry from the shared `child_entry_verdict` (`DelegationAccepted` / `NoMatchingParent` / `NotAttached` / `DepthExceeded`, plus `InvalidParentRefs` / `UnsupportedValue` on every entry, and `PartiallyInvalid` for out-of-scope rules (a method match naming a service other than the delegated one) and per-rule mandatory-filter conflicts; when those cover every child rule the entry is `Accepted=False / UnsupportedValue`). The Gateway-acceptance loop gates on `is_gateway_parent_ref` so route-kind parentRefs never run `validate_parent_ref_accepted`.

### ExtensionRef ownership and recovery

The Controller consumes `GrpcRouteExtensionRefs` loans from the route's own
filter positions, with the same rules as
[HTTPRoute](04-http-route.md#extensionref-ownership-and-recovery): exact
`edgion.io/EdgionPlugins`, a valid name and a valid owning Route identity precede
dependency registration or content lookup; every ExtensionRef resolves in the
declaring route's namespace; each route registers its own plugin dependencies;
no ExtensionRef loan can mint a ReferenceGrant query. On a delegated call each
ancestor level resolves its own ExtensionRef array against the root-matched
Gateway's plugin scope.

## Gateway-side processing

GRPCRoute's ConfHandler compiles routes into `GlobalGrpcRouteManagers`:

0. **Rules and delegation children**: compiles `spec.rules` minus `invalid_rule_indices`. A delegation child (all parentRefs are GRPCRoute refs) is not bucketed by listener; `grpc/delegation_build.rs` accepts its keys exactly as HTTP does (Controller keys, local parentRef/shape re-check, `Accepted=True` gating), compiles its units once and shares them into the `GrpcMatchEngine` subtree of each key. Delegating rule units hold their `Arc<DelegationSlot>` from compile time
1. **preparse**: in the route cache, call `route.preparse()` to build runtime structures
2. **Route units**: compile each method predicate once into `GrpcMatchInfo.compiled_method_match`; any invalid predicate drops its whole rule, even without Controller status. Valid matches share the rule/plugin/backend runtime through `Arc`.
3. **Domain matching**: index the Route's declared spec.hostnames (borrowed from its existing schema Arc), with omitted hostnames in catch-all. Keep resolved_hostnames as the deep-match eligibility constraint: Listener intersection must not promote Route hostname priority. The shared Algo `RadixHostnameMatcher` visits exact and descending wildcard suffixes before catch-all. Within each group, merge up to five indexed streams by build-time service/method/header/route rank. Failed full matches continue to broader domains. See [the engine](../02-gateway/03-routes/02-grpc-route.md).
4. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`; on change, only rebuild the route table for the affected listeners
5. **gRPC-Web support**: automatically supports the gRPC-Web protocol via content-type detection

## Delegation (GRPCRoute → GRPCRoute)

GRPCRoute delegation uses the same cascading match as HTTPRoute (shared
`edgion-resources/src/resources/route_delegation.rs` vocabulary, shared
`DelegationGraph` and status helpers in `handlers/route_delegation.rs`, shared
Gateway `routes/delegation/` slot table, descent state and inheritance fold).
Only rule-shape facts are gRPC-specific (`handlers/grpc_route_delegation.rs`,
`delegation_target_of_grpc_rule`). User semantics:
[`docs/en/user-guide/grpc-route/delegation.md`](../../../docs/en/user-guide/grpc-route/delegation.md);
feature schema: [`02-features/03-resources/05-grpcroute.md`](../../02-features/03-resources/05-grpcroute.md).

gRPC-specific rules:

1. A delegating rule with `matches` absent/empty is **invalid** (HTTP treats a
   matchless delegating rule as delegating `/`; delegate-all is out of scope for
   gRPC). Every delegating match needs `method.type` Exact-or-omitted, a non-empty
   `service` and no `method`.
2. Child matches are absolute and intersect with the delegated service at match
   time: a different `service` never matches, a child without `service` matches
   the delegated one, and a matchless child rule is a catch-all inside the subtree
   (safe because the root level already fixed the service). The former matchless
   rewrite and child regex rejection are gone; ordinary method validation applies.
3. Each slot holds its own `GrpcMatchEngine` with per-level build-time ranks.

### Compiled native backend ownership

`GrpcRouteRuleUnit::compile_grpc_rule` takes the actual GRPCRoute and effective rule
index. The source-validated effective loan compiles the canonical Service key,
policy target and checked numeric section once. Private compiled schema and target
prevent detached construction. Invalid owners call no rule/backend runtime factory;
invalid or denied slots retain their original weights and indexes but call no
backend factory, sticky lookup, policy lookup or backend plugin dispatch.

Sticky candidates and the selected generation borrow permitted Service targets.
Final peer resolution checks the diagnostic context against that target instead
of cloning or trusting its key. Invalid/denied selections retain the existing
HTTP 200 with gRPC status 14 wrapper. First-attempt traffic-policy capture also
requires the selected target, and retries preserve the captured Arc or absence.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| GRPCRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's Listener |
| GRPCRoute → GRPCRoute | GRPCRoute | delegation backendRef (`kind: GRPCRoute`) + child `parentRefs {kind: GRPCRoute}` + `grpc_route_delegation_graph` | Bidirectional confirmation; child carries `resolved_delegation_keys`; Gateway matches through per-key `GrpcMatchEngine` subtrees |
| GRPCRoute → Service | Service | backendRef | Route's backend target service |
| GRPCRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| GRPCRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |

### Session persistence lifetime ownership

Gateway rule compilation validates each rule's own `sessionPersistence` and
resolves its optional lifetime before publishing any match units. The shared
resource policy uses the canonical duration parser with strict surrounding
whitespace rejection and whole-second ceiling/minimum-one semantics (including
zero). Both direct compilation and normal full/incremental publication reject an
invalid policy, including Permanent cookies without `absoluteTimeout`.

The scalar belongs to the immutable Gateway rule unit alongside its effective
rule, not to the serialized resource. A request retains that same generation
through response issuance after reload. Response code borrows config and lifetime
together and uses one scalar for wire expiry and Permanent-cookie Max-Age; it
never parses configured duration text. See the
[session persistence regression guard](../../04-review/routing/regression-guards/session-persistence-token-extraction.md)
for ARCH-060 verification boundaries.
