---
name: named-port-resolve-against-originating-slice
description: Use when reviewing resolve_target_port_key named-multi-port findings; covers why a named port is now resolved against the slice the selected endpoint came from, not the first slice declaring the name.
status: fixed
finding_id: port-resolver-heterogeneous-slice-ports
closed: 2026-07-18
---

# resolve_target_port_key honors the selected endpoint's originating slice — FIXED

## Conclusion

`resolve_target_port_key` (`edgion-gateway/src/backends/port_resolver.rs`) no longer collapses a
named multi-port lookup onto the first EndpointSlice that declares the port name. It now locates the
slice the **selected endpoint** actually came from (by matching the endpoint IP) and resolves the
port name within that slice; only when the origin is unknown does it fall back to a **deterministic**
cross-slice match (`min`, not HashSet-iteration first-match).

Re-reports of the form "named port takes the first slice's number regardless of endpoint",
"heterogeneous sibling slices misdirect traffic to the wrong pod port", or "first-match depends on
HashSet order" are **not accepted** — verify the current `resolve_in_slice` + `slice_contains_ip`
origin lookup before re-reporting.

## Core Rationale

**1. Heterogeneous named ports are a legitimate K8s state, not just misconfiguration**

When a Service uses a *named* `targetPort` (a string), kube-controller resolves the name per-Pod
against each Pod's `containerPort`. During a rollout where the new pod template exposes that named
port on a different number, EndpointSlices legitimately carry different port numbers for the same
name side by side. kube-proxy routes each endpoint on its own slice's port; the gateway must match
that semantics. The old `slices.iter().find_map(...)` applied the first slice's port to every
endpoint, connecting a slice-B pod on slice-A's port (wrong pod port → connection refused / wrong
app) for the duration of the rollout window.

**2. The cell already carries correct per-slice ports; the rewrite only needed to stay slice-local**

Since the 2026-07-17 `get_backends_for_service` fix, each backend address is built with its own
slice's port. The rewrite exists to select the *named* port out of a multi-port slice's port list;
scoping that lookup to the originating slice keeps it consistent with the address the LB already
chose. Homogeneous, numeric-port, and single-port Services are unaffected — those paths return
before or resolve identically.

**3. Determinism for the unknown-origin fallback**

`get_slices_for_service` returns slices in `HashSet` iteration order, so a plain first-match is
non-deterministic across rebuilds. The fallback now uses `min()` over the matching ports so a stale
or unmatched endpoint resolves stably instead of flipping between rebuilds.

## Fix Suggestions Not Accepted

- "Add a `tracing::warn!` when the port is rewritten / origin is unknown" — data-plane hot path,
  Iron rule 3 (`skills/03-coding/01-log-safety.md`); no logging added here.
- "Also make `get_backends_for_service`'s portless `fallback_port` deterministic / carry per-backend
  port-name maps through the cell" — the portless-slice case is config-error territory
  (kube-controller always sets slice ports) and was intentionally left out of this narrow fix; a
  cell-schema change is out of scope. See the parked task for that direction.
- "Match the originating slice by string-compare on the address" — addresses are parsed to
  `IpAddr` before comparison so canonical vs. non-canonical IPv6 forms compare equal.

## Re-evaluation Triggers

Re-open this decision only if:

- `resolve_target_port_key` reverts to a cross-slice first-match that ignores the selected endpoint.
- A cell-schema change starts carrying explicit per-backend port identity (then the IP-match origin
  lookup can be replaced by the carried identity).

## Reference Cases

- `tasks/todo/port-resolver-heterogeneous-slice-ports.md` (finding, 2026-07-17), closed 2026-07-18.
- Sibling: [ep-slice-no-port-fail-closed.md](ep-slice-no-port-fail-closed.md) (backends-F013) — the
  2026-07-17 fail-closed fix that narrowed the bug to this rewrite site.
- Unit test: `port_resolver::tests::named_port_resolves_against_originating_slice`.
- Integration: `edgion-tests/src/client/suites/http_route/multi_port.rs` (homogeneous MultiPort path,
  stays green).
