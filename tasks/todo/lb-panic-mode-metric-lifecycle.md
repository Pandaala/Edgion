# Keep the load-balancer panic-mode gauge current across lifecycle exits

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make `edgion_lb_panic_mode{service}` return to zero whenever a Service leaves panic mode, including lifecycle paths that no longer run the survivor calculation. `panic_engaged` currently writes the gauge only after it evaluates a non-zero threshold against a non-empty selector with health or outlier state. If the series was previously set to one, changing the threshold to zero, removing health/outlier state, emptying the selector, or deleting the Service takes an early return and can leave the exported gauge at one indefinitely.

## Requirements

- Preserve the inventory contract: the gauge is one exactly while the named Service is currently in panic mode and zero otherwise.
- Reset an existing series when the effective threshold becomes zero, health/outlier capability is removed, the selector becomes empty, or the Service is deleted.
- Do not register a new zero-valued series for every ordinary Service that has never been panic-capable.
- Do not construct an owned service label or perform registry lookup/allocation on every request. Reuse the canonical Gateway metrics layer and a bounded lifecycle-owned handle or equivalent mechanism.
- Keep the `service` label domain bounded to canonical Service resource keys; do not add endpoint, route, address, or request labels.
- Coordinate threshold capture with `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md`; metric reset must observe the same effective Service generation as selection.
- Add one shared test-state guard for tests that mutate the process-global panic default or override store, with RAII restoration so assertion failures cannot leak state across tests.

## Scope

### In scope

- Panic-mode metric handle ownership and reset paths.
- Selector, policy, health/outlier, EndpointSlice, and Service lifecycle hooks needed to set zero.
- Cross-module isolation for tests that mutate panic-threshold globals.

### Out of scope

- Changing panic-threshold selection semantics or its configuration schema.
- Adding an access-log field for panic mode.
- Removing unrelated Prometheus series.

## Acceptance criteria

- Tests prove transitions from one to zero for threshold disable, health removal, outlier removal, empty selector, and Service deletion.
- A Service that never configures/evaluates panic mode does not create a labeled gauge series merely because it handles traffic.
- Repeated request selection uses a cached metric handle and performs no label allocation.
- Concurrent configuration and selection cannot republish one after a lifecycle reset for an older generation.
- All tests that mutate `GLOBAL_DEFAULT` or the process-global override store share one guard and restore state on unwind.
- Metrics inventory, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/02-features/02-config/02-edgion-gateway-config.md`
- `skills/02-features/04-observability/01-metrics.md`
- `skills/04-review/architecture/edgion-gateway-config-global-aggregation-deterministic.md`
- `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the file audit. | Writing zero from every request-path early return would create unnecessary series and owned labels, while correct reset coverage spans policy, health, outlier, selector, Service, and metrics lifecycles. |
