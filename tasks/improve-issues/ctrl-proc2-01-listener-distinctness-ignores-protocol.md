# ctrl-proc2-01: Listener conflict detection ignores protocol, so TCP+UDP on one port are both dropped and HTTP/HTTPS/TLS/TCP mixes on one port crash Gateway startup

- **Category**: design-error
- **Severity**: P1 (correctness / availability)
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs:56-77` (`make_conflict_key` = `ip:port` only; `hostnames_conflict` = equality)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs:358-406` (`detect_conflicts`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:697-717` (registration: `(name, make_conflict_key(addr), hostname)`, protocol dropped)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:2709-2736` (`detect_mixed_tls_mode_listeners`: only `protocol == TLS` listeners are compared)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs:133-220` (`replace_shared_listeners`: cross-Gateway groups keyed by `ListenerKey` = `addr/PROTOCOL`)
  - Data plane consequence: `edgion-gateway/src/runtime/server/base.rs:193-246,285-344` (`startup_listener_priority`, `select_startup_listener_contexts` → `anyhow::bail!` on overlapping sockets with different `ListenerKey`), called from `edgion-gateway/src/cli/pingora.rs:110` with `?`

## Summary
The controller decides listener distinctness with a key that drops the protocol (`ip:port`) and treats two
listeners as conflicting only when their hostnames are byte-identical. That is wrong in both directions.
TCP and UDP listeners on the same port have different OS sockets and are distinct per the Gateway API.
Edgion marks them both `Conflicted`, and the Gateway then binds neither. A TCP listener and an
HTTP/HTTPS/TLS listener on the same port are never distinct per the spec, and HTTP and HTTPS on the same
port cannot share one socket either. Edgion only flags these mixes when both hostnames are empty. The data
plane then sees two different `ListenerKey`s on one socket, and `configure_listeners` returns an error.
Because the Gateway binds listeners only at startup, the Gateway process fails to start and crash-loops.

## Current behavior (evidence)
- `gateway.rs:697-717` registers each listener as `(l.name, make_conflict_key(resolve_listener_addr(&g, l.port)), l.hostname)`.
  The protocol is not part of the key or the `ListenerRef`.
- `listener_port_manager.rs:76` `hostnames_conflict(a, b) = a == b`. `detect_conflicts` flags a pair only
  when both hostnames are `None` or identical strings.
- Status: `gateway.rs:1319-1330` turns a `detect_conflicts` hit into `Conflicted=True / ListenerConflict`.
  The only protocol-aware check, `detect_mixed_tls_mode_listeners`, considers `TLS` listeners only. It looks
  for different `tls.mode` values on one port and never compares HTTP, HTTPS, TCP, or UDP.
- Gateway: `startup_listener_priority` skips every listener whose status is `Conflicted`.
  `select_startup_listener_contexts` then `bail!`s when two selected listeners with different `ListenerKey`s
  (`addr/PROTOCOL`) overlap on one socket. `socket_bindings_overlap` (`base.rs:329`) does treat UDP and TCP
  as non-overlapping, so the data plane already knows these two are distinct.

Concrete outcomes in one Gateway (cross-Gateway mixes in a shared process behave the same, because
`replace_shared_listeners` groups by `addr/PROTOCOL` and never compares different protocols):

| Listeners | Gateway API verdict | Edgion controller | Gateway process |
|---|---|---|---|
| `TCP:53` + `UDP:53` (no hostnames) | distinct, both accepted | both `Conflicted` | neither bound (DNS-style TCP+UDP service impossible) |
| `HTTP:8080 host=a.com` + `TCP:8080` | not distinct, both MUST NOT be accepted | no conflict | `bail!` → startup failure |
| `HTTP:8443 host=a.com` + `HTTPS:8443 host=b.com` | different protocols on one socket, rejected by mature impls | no conflict | `bail!` → startup failure |
| `HTTPS:443 host=a.com` + `TLS:443 host=b.com` (Passthrough) | distinct by hostname (a common layout) | no conflict | `bail!` → startup failure |

The last row is a supported Gateway API pattern: HTTPS terminate and TLS passthrough on 443, split by SNI.
Edgion can neither serve it nor report it. The process dies instead of setting a listener condition.

## Industry comparison
- Gateway API (`gateway-api-standard-v1.6.2.yaml`, `spec.listeners` "Distinct Listeners"): HTTP/HTTPS/TLS
  listeners are distinct by Port+Hostname. TCP/UDP listeners are distinct by Port within the same protocol.
  When TCP shares a port with HTTP/HTTPS/TLS, "all the Listeners that share a port with the TCP Listener are
  not distinct and so MUST NOT be accepted". The expected condition is `Conflicted=True` with reason
  `ProtocolConflict`.
- Envoy Gateway computes conflicts per `(port, L4 transport)`. HTTP/HTTPS/TLS on one TCP port are grouped,
  incompatible protocol mixes get `ProtocolConflict`, and UDP is a separate socket namespace. HTTPS+TLS on
  one port is served by one Envoy listener with SNI-based filter chains.
- Istio's Gateway translator likewise keys servers by port+protocol and reports conflicts in status instead
  of failing the proxy.

## Impact
- A valid `TCP:53`/`UDP:53` Gateway silently loses both listeners.
- An invalid or unsupported mix is accepted by the controller and turns into a data-plane crash-loop of every
  Gateway process that serves the class. This is a blast radius far beyond the offending listener, and it is
  reachable by anyone who can edit a Gateway.
- Status disagrees with the data plane, so operators get no `ProtocolConflict` hint. The only symptom is the
  pod failing at startup.

## Suggested direction
1. Make the conflict key `(ip, port, transport)`, where transport is UDP or TCP. Within a TCP group, apply
   the spec rules:
   - any TCP listener sharing the port with another listener makes the whole group `Conflicted=True,
     reason=ProtocolConflict`;
   - an HTTP+HTTPS/TLS mix on one port is `ProtocolConflict`;
   - HTTPS+TLS on one port is allowed only if the data plane gains an SNI dispatcher for it, otherwise
     `ProtocolConflict` (or `UnsupportedValue`);
   - same-protocol HTTP/HTTPS/TLS listeners keep the existing hostname-equality rule.
2. Apply the same transport-and-protocol compatibility check to the cross-Gateway shared groups in
   `replace_shared_listeners`, so two Gateways served by one process cannot pass an incompatible pair to the
   data plane.
3. Keep the Gateway `bail!` as a last-resort invariant check, but the controller must guarantee that it is
   unreachable. Add a processor-level test for each row of the table above.

## Notes
- Checked `skills/04-review/tls/regression-guards/cross-gateway-sni-collision-deterministic.md`. It
  documents that `detect_conflicts` is intra-gateway only and covers same-protocol SNI collisions, not
  protocol or transport distinctness.
- `skills/04-review/resources/intentional-design/schema-validator-non-k8s-asymmetry.md` (ARCH-152) covers
  port-range validation only.
- The mixed TLS-mode check and its integration fixture
  (`edgion-tests/integration/conf/Status/TLSListenerMode/`) show that protocol conflicts are intended to be
  reported in status. The protocol-blind key is a gap, not a design choice.
- No `tasks/` item tracks this.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `listener_port_manager.rs:53-77`: `make_conflict_key(addr)` = `addr.to_string()` (comment: "Protocol is excluded"); `hostnames_conflict(a,b) = a == b`.
- `handlers/gateway.rs:697-716`: registration tuple `(name, make_conflict_key(addr), hostname)` drops protocol.
- `handlers/gateway.rs:1300-1335`: only mixed TLS-mode (`PROTOCOL_CONFLICT`), PROXY-policy and hostname-equality conflicts are reported; no HTTP/HTTPS/TCP/TLS protocol-mix check exists (`grep ProtocolConflict` shows only these sites).
- `edgion-gateway/src/runtime/server/base.rs:285-326`: `select_startup_listener_contexts` `bail!`s on overlapping sockets with different `ListenerKey` (`addr/PROTOCOL`, `edgion-resources/src/listener_key.rs:25`), propagated by `?` at `base.rs:246` and `cli/pingora.rs:110` → process startup failure; `socket_bindings_overlap` (`:329`) already treats UDP vs TCP as distinct.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `listener_port_manager.rs:59,76`: `make_conflict_key(addr)` (protocol excluded) and `hostnames_conflict` equality unchanged; file unchanged since `feature-08-30`.
- `handlers/gateway.rs` unchanged, so the registration tuple still drops protocol and only mixed TLS-mode conflicts are reported.
- `edgion-gateway/src/runtime/server/base.rs` and `cli/pingora.rs` unchanged: overlapping sockets with different `ListenerKey` still `bail!` at startup.
