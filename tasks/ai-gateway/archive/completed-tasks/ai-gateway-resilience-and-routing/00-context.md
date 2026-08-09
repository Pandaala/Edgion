# Context

## Related Skills

- `skills/SKILL.md`
- `skills/00-design-rules.md`
- `skills/07-tasks/SKILL.md`
- `skills/07-tasks/new_feature_work_flow.skill`
- `skills/08-kubernetes/01-gateway-api/SKILL.md`
- `skills/01-architecture/05-resources/04-http-route.md`
- `skills/01-architecture/05-resources/22-edgion-ai-service.md`
- `skills/01-architecture/02-gateway/06-load-balancing.md`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
- `skills/02-features/03-resources/04-httproute.md`
- `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`
- `skills/02-features/03-resources/22-edgion-ai-service.md`
- `skills/03-coding/observability/00-access-log.md`
- `skills/05-testing/01-integration-testing.md`

## Primary code entry points

- `edgion-resources/src/resources/http_route.rs`
- `edgion-resources/src/resources/http_route_preparse.rs`
- `edgion-resources/src/resources/edgion_ai_service.rs`
- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_ai_service.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs`
- `edgion-gateway/src/routes/http/match_unit.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`
- `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`
- `edgion-gateway/src/ai/service_store.rs`
- `edgion-gateway/src/ai/usage.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/backends/outlier_detection/store.rs`

## Constraints carried into the design

- A request body may be cached only in the existing request-scoped `BodyStorage`.
- The main upstream response remains streaming and must not be aggregated.
- Request callbacks and response-chunk callbacks use no `tracing` calls.
- Cross-provider retries are allowed only before a final response is committed.
- Credentials, request bodies, provider errors, and endpoint URLs never enter logs or metrics.
- Runtime configuration snapshots are immutable and read without blocking locks.
- Route, resource, Secret, ReferenceGrant, and policy changes re-enter the owning serialized
  workqueue through requeue; handlers do not call one another directly.
