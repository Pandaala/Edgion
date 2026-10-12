---
name: ep-slice-no-port-fail-closed
description: EndpointSlices that do not declare the requested Service port produce an empty eagerly compiled Service section instead of the historical silent unwrap_or(8080); use when reviewing no-port fail-close behavior and its current discovery-generation error mapping.
knowledge_type: regression-guards
---

# backends-F013 — no-port EndpointSlices fail closed instead of defaulting to 8080

**Status:** ✅ fixed · 2026-05-24
**Severity:** Low (BadLogic) — silent fallback to port 8080 misdirected traffic and hid the misconfiguration

## Conclusion

The Gateway still **fails closed** when no EndpointSlice declares the requested Service port, but
the active implementation is no longer `get_backends_for_service` or lazy selector hydration.
`discovery/coordinator.rs::normalize_section` skips each slice that has no matching port. If none match,
normalization yields an empty resolved geometry. Final materialization alone
publishes empty `all_slots` and `ready_slots` plus `CompiledSelectors::build(empty)`
as part of the joint immutable discovery generation. Preflight constructs no selectors. No address
or fallback port is fabricated.

On a request, `selector.rs::select_service_endpoint` captures the available Service projection and
runtime snapshot from one published generation, resolves the requested section, selects from its
already-compiled policy selector, and fails when that selector is empty. The verified mappings are:

- an existing empty section using default round-robin returns
  `BackendEndpointSliceNotFoundByRoundRobin`;
- an absent Service projection returns `BackendEndpointSliceNotFoundByRoundRobinDefault`;
- an invalid or missing requested section returns `BackendPortResolutionFailed`.

The resulting diagnostic is owned by the normal request failure/access-log path. This preserves
fail-close behavior without sending traffic to a guessed port.

Re-reports of the form "the Gateway still falls back to port 8080" or "a no-port slice is dialed
using a magic default" are **not accepted** unless current `normalize_section` again fabricates a port.

## What was wrong before the fix

The retired EndpointSlice store used this fallback:

```rust
let port = slices
    .iter()
    .find_map(|s| s.ports.as_ref()?.first()?.port.map(|p| p as u16))
    .unwrap_or(8080);
```

When every EndpointSlice for a service had `ports: None`, an empty ports list, or `port: None`, the
LB built backends with the magic constant 8080. A connection could fail or reach the wrong
application, while the configuration error itself remained hidden. This snippet and its
`get_backends_for_service` call chain are historical evidence of the defect, not current behavior.

## Why the current design is correct

Publishing an empty resolved section is the fail-closed direction: there is no correct serving
socket to place in `all_slots`, so all eager selectors are built empty. The section remains in the
same immutable generation as its Service identity and runtime membership, preventing either a
guessed address or a mixed-generation selector. Request processing performs no provider callback,
hydration, or selector rebuild.

## Fix suggestions not accepted

- **Emit a `tracing::warn!` naming the service key on request failure.** The suggested historical
  function is retired, and adding a new request-path `tracing::` event would violate binary-owned
  logging, canonical access-log ownership of request failures, and observability restraint. The
  selector returns the established `EdgionErrorCode`; normal access-log diagnostics carry the
  request outcome without a parallel log channel.
- **Keep 8080 as a configurable implicit default.** There is no correct universal default. Any
  future default target port must come from an explicit operator-set field rather than a baked-in
  fallback.

## Re-evaluation triggers

Re-open only if:

- `normalize_section` starts fabricating a port or admits a slice that does not match the requested
  Service section.
- A future requirement introduces an explicit default target-port policy; it must preserve schema,
  publication, and observability boundaries rather than reviving an implicit constant.

## Reference cases

- backends-F013 (review finding, 2026-05-24).
- `edgion-gateway/src/backends/discovery/coordinator.rs` — `normalize_section` and joint section
  publication.
- `edgion-gateway/src/backends/selector.rs` — `load_active_projection`,
  `select_service_endpoint`, and current error mapping.
- `skills/03-coding/observability/00-access-log.md` and `skills/03-coding/01-log-safety.md` —
  canonical request diagnostics and observability restraint.
