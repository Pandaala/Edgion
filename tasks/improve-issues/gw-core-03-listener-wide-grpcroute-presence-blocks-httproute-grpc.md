# gw-core-03: Any GRPCRoute on a Listener makes native gRPC to every other hostname on that Listener return UNIMPLEMENTED

- **Category**: design-error
- **Severity**: P1 (correctness, cross-tenant)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:200-222`
  - `edgion-gateway/src/routes/grpc/routes_mgr.rs:96-98` (`DomainGrpcRouteRules::has_routes` = `match_engine.is_some()`)
  - `edgion-gateway/src/routes/grpc/integration.rs:32-55`
  - Docs in conflict: `skills/08-kubernetes/01-gateway-api/01-not-implemented.md:51-59` vs `skills/01-architecture/02-gateway/03-routes/02-grpc-route.md:107-115`

## Summary
For a native gRPC request (`content-type: application/grpc`), the Gateway first matches the Listener's
GRPCRoute table. If nothing matches and the table has any routes at all, for any hostname, the request is
terminated with `grpc-status: 12 UNIMPLEMENTED` and the HTTPRoute table is never consulted. Proxying gRPC
through an HTTPRoute is a common and spec-valid pattern, for example with an `h2c` appProtocol backend. That
pattern breaks for a host as soon as another tenant attaches a GRPCRoute for an unrelated hostname to the
same shared Listener. The fallback decision is made per Listener, while the Gateway API resolves
HTTPRoute/GRPCRoute coexistence per hostname intersection.

## Current behavior (evidence)
- `pg_request_filter.rs:208-219`:
  ```rust
  let grpc_routes = publication.grpc_routes(&edgion_http.grpc_route_manager);
  let _ = try_match_grpc_route(&grpc_routes, session, ctx, gateway_index).await;
  if !ctx.is_grpc_route_matched && grpc_routes.has_routes() && ctx.request_info.discover_protocol == Some("grpc") {
      ctx.record_error(EdgionErrorCode::RouteNotFound);
      end_response_grpc_unimplemented(...).await?;
      return Ok(true);
  }
  ```
- `routes_mgr.rs:96-98`: `has_routes()` is `self.grpc_routes.match_engine.is_some()`, which is true when any
  GRPCRoute is attached to the Listener regardless of hostname.
- Scenario: Listener `https/443` is shared, with `allowedRoutes.namespaces.from: All`. Team A has
  `HTTPRoute host=a.example.com → svc-a (appProtocol h2c)` serving gRPC. Team B adds
  `GRPCRoute host=b.example.com`. From then on, every gRPC call to `a.example.com` is answered locally with
  UNIMPLEMENTED. Team A's route status still says Accepted, and nothing in team B's resources points at the
  cause.
- The skills are inconsistent. `08-kubernetes/01-gateway-api/01-not-implemented.md:57` claims "gRPC
  requests match GRPCRoute first and fall back to HTTPRoute on no match", while the architecture page and
  the code implement listener-wide termination.

## Industry comparison
- **Gateway API spec** (vendored `gateway-api-standard-v1.6.2.yaml`, GRPCRoute `spec.hostnames`): when an
  HTTPRoute and a GRPCRoute attached to the same Listener have a **non-empty hostname intersection**,
  exactly one is accepted (oldest, then alphabetical) and the other gets `Accepted=False`. Coexistence is
  therefore resolved per hostname at admission time. Routes with disjoint hostnames must both work.
- **Envoy Gateway / Istio / Contour**: GRPCRoute is translated into ordinary route entries
  (`:path` prefix/exact `/pkg.Service/Method` + header matches) inside the same per-virtual-host route
  table as HTTPRoute. A gRPC request to `a.example.com` only ever sees `a.example.com`'s routes. There is
  no Listener-wide protocol gate.

## Impact
- Cross-tenant availability breakage on shared Listeners: adding a GRPCRoute anywhere silently breaks gRPC
  served by HTTPRoutes on every other hostname.
- Hard to diagnose: the response is a well-formed gRPC UNIMPLEMENTED, so clients see "method not
  implemented" rather than a gateway error.
- The parallel gRPC route stack (`routes/grpc/`, about 2.6k lines of manager, match engine, delegation build
  and conf handler that largely mirror `routes/http/`) is what forces this coarse fallback rule. The two
  tables cannot be compared per hostname without a request-time cross-table walk.

## Suggested direction
- **Option A (minimal, local)**: make the fallback decision per hostname. Terminate with UNIMPLEMENTED only
  when the GRPCRoute table has a group for the request's matched hostname, that is, when a hostname group
  is selected but no candidate matches. Otherwise fall through to HTTPRoute matching. `DomainGrpcRouteRules`
  already has the `RadixHostnameMatcher` and catch-all group needed to answer "does any GRPCRoute claim this
  hostname".
- **Option B (recommended, large)**: implement the spec's admission rule in the Controller. Reject the
  younger route of an HTTPRoute/GRPCRoute pair with intersecting hostnames on the same Listener, setting
  `Accepted=False`. Then in the Gateway, either keep two tables with per-hostname exclusivity guaranteed, or
  lower GRPCRoute into the HTTP route table (path `/service/method` + header matchers + gRPC-specific rule
  runtime), as Envoy Gateway does. Lowering removes most of the duplicated `routes/grpc` matching and
  delegation code and makes precedence obviously per-virtual-host.
- In both options, fix `08-kubernetes/01-gateway-api/01-not-implemented.md` so it describes the real
  behavior.

## Notes
- Checked `skills/04-review/routing/regression-guards/grpc-method-matching-and-precedence.md`. It records the
  listener-wide `has_routes()` termination as current behavior but gives no rationale for choosing Listener
  granularity over hostname granularity, and does not discuss shared-Listener tenancy.
- Also checked `routing/intentional-design/route-architecture-consistency.md` (descriptive matrix only) and
  `08-kubernetes/01-gateway-api/01-not-implemented.md` (which documents a *different*, fallback behavior). No
  accepted-tradeoff page covers this, and no `tasks/` item tracks it.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged; cross-tenant silent outage on a shared Listener)
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:208-222`: on no GRPCRoute match, `grpc_routes.has_routes()` alone (plus `discover_protocol == "grpc"`) triggers local UNIMPLEMENTED; the code comment even says fallback happens "only when no GRPCRoutes are configured".
- `edgion-gateway/src/routes/grpc/routes_mgr.rs:95-98`: `has_routes()` is `match_engine.is_some()`, Listener-wide and hostname-blind.
- `skills/08-kubernetes/01-gateway-api/01-not-implemented.md:57` claims "fall back to HTTPRoute on no match", contradicting the code; `skills/04-review/routing/regression-guards/grpc-method-matching-and-precedence.md:34` records `has_routes()` as current behavior without a rationale for Listener granularity.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:215`: `!ctx.is_grpc_route_matched && grpc_routes.has_routes() && discover_protocol == Some("grpc")` still terminates locally.
- `edgion-gateway/src/routes/grpc/routes_mgr.rs:96-98`: `has_routes()` unchanged; only `conf_handler_impl.rs` changed in `routes/grpc/` (timestamp helper).
- `skills/08-kubernetes/01-gateway-api/01-not-implemented.md` has no diff and still contradicts the code.
