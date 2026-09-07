---
name: ep-slice-no-port-fail-closed
description: get_backends_for_service now fails closed (returns no backends → 500) when no EndpointSlice declares a usable port, instead of the old silent unwrap_or(8080). Reject re-reports of the 8080 magic-default misdirection; also reject the issue's suggested tracing::warn! inside get_backends_for_service (data-plane hot path, Iron rule 3).
knowledge_type: regression-guards
---

# backends-F013 — no-port EndpointSlices fail closed instead of defaulting to 8080

**Status:** ✅ fixed · 2026-05-24
**Severity:** Low (BadLogic) — silent fallback to port 8080 misdirected traffic and hid the misconfiguration

## Conclusion

`get_backends_for_service` (`edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs`)
now **fails closed** when no EndpointSlice for the service declares a usable port: the
`.find_map(...).unwrap_or(8080)` was replaced with a `let Some(port) = .find_map(...) else { return Vec::new(); }`.
With no backends built, every caller's existing empty-list handling takes over and the data-plane
selector returns `BackendEndpointSliceNotFoundByRoundRobinDefault` → **500**, which the access log
records. The operator now sees a clear failure instead of traffic silently landing on a guessed
port 8080 (which often still accepts the connection, making the misconfiguration invisible).

Re-reports of the form "`get_backends_for_service` falls back to port 8080 without warning",
"baked-in 8080 magic constant misdirects traffic", or "no log when no port is found" are
**not accepted** — verify the current `get_backends_for_service` returns `Vec::new()` on the
no-port branch before re-reporting.

## What was wrong (before the fix)

```rust
let port = slices
    .iter()
    .find_map(|s| s.ports.as_ref()?.first()?.port.map(|p| p as u16))
    .unwrap_or(8080);
```

When every EndpointSlice for a service had `ports: None`, an empty ports list, or `port: None`
(a portless / misconfigured Service that still receives routed traffic), the LB built backends
with the magic constant 8080. The connection attempt succeeds at the socket layer but lands on
the wrong port — connection refused or a wrong-application response — with **no signal** that a
fallback occurred. `port_resolver::resolve_target_port_key` is not a safety net: it returns early
when `br_port` is `None`, only *adjusts* an already-built address, and depends on the Service
object being present in the store (not guaranteed under partial sync / restart / headless shapes).

## Why the fix is correct

Returning an empty backend list is the fail-closed direction: there is no correct port to use, so
the gateway refuses to serve (500) rather than fabricate one. All four callers already handle the
empty case correctly — data-plane `selector.rs::hydrate_cell_if_empty` (→ 500), control-plane
`conf_handler_common.rs::update_*_lbs` (`replace_backends(empty)` clears stale entries),
`preload.rs` (skips), and `health/check/manager.rs` (no backends to probe). No signature change.

## Fix Suggestions Not Accepted

- **The issue's literal suggestion — "emit a `tracing::warn!` inside `get_backends_for_service`
  naming the service key" — is not accepted.** That function is reached on the data-plane request
  path via `selector.rs::hydrate_cell_if_empty` → `compiled_selector.rs::hydrate_if_empty`, which
  **re-invokes the provider on every request while the cell is empty**. Since the fix makes the
  cell stay empty for a no-port service, a warn there would re-fire per request (≈ once per QPS) —
  exactly the "log explosion" that Iron rule 3 (`skills/03-coding/01-log-safety.md`) forbids on the
  hot path. Observability is delivered through the access log's 500 status instead (project rule:
  data plane logs only via access log / metrics, never `tracing::`).
- "Keep 8080 as a configurable default" — there is no correct universal default; a portless
  Service is a misconfiguration that should surface, not be papered over.

## Re-evaluation Triggers

Re-open only if:

- `get_backends_for_service` reverts to `unwrap_or(<any port>)` or otherwise fabricates a port.
- A future requirement genuinely needs a default target port; that must come from an explicit,
  operator-set field, not a baked-in constant, and must not add `tracing::` to this function.

## Reference Cases

- backends-F013 (review finding, 2026-05-24)
- Iron rule 3 (`skills/03-coding/01-log-safety.md`) — why the suggested `tracing::warn!` was placed
  off this path (access log carries the 500 instead).
