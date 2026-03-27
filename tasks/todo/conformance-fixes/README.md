# Conformance Test Fixes

> Extracted from [conformance-failures analysis](../conformance-failures/README.md)
> Current score: 38/60 PASSED | Target after fixes: ~57/60

## Fix Order

| # | Fix | Priority | Effort | Tests Fixed | File |
|---|-----|----------|--------|-------------|------|
| 1 | [RewritePath double-slash](01-rewrite-path-double-slash.md) | High | Trivial | 1 | `url_rewrite.rs` |
| 2 | [Listener isolation hostname check](02-listener-isolation-hostname-check.md) | High | Small | 1 (12 subtests) | `route.rs` |
| 3 | [AttachedRoutes filtering](03-attached-routes-filtering.md) | High | Small | 4 | `handlers/mod.rs` |
| 4 | [GRPCRoute error response](04-grpcroute-error-response.md) | High | Small | 4 (23 subtests) | `error_response.rs` |
| 5 | [Route status persistence](05-route-status-persistence.md) | High | Medium | 6 | `resource_controller.rs` |
| 6 | [HTTPS catch-all listener TLS](06-https-catchall-listener-tls.md) | Medium | Small | 2 (5 subtests) | Gateway TLS layer |
| 7 | [Timeout zero handling](07-timeout-zero-handling.md) | Medium | Medium | 2 | `ParsedRouteTimeouts` |
| 8 | [Percentage mirror field](08-percentage-mirror-field.md) | Medium | Small | 1 | `http_route.rs` + `plugin.rs` |

**Deferred:** HTTPRouteListenerPortMatching (4 subtests) — requires dynamic listener creation (P4 architectural change).
