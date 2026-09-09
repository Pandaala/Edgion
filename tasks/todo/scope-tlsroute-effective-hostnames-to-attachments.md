# Scope TLSRoute Effective Hostnames to Attachments

## Meta

- Status: pending
- Type: bug
- Priority: P1
- Severity: high
- Source: Gateway campaign B0204 / D0114

## Evidence

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs:151`
  projects all attached listener hostnames into a route-global effective union.
- `handlers/route_utils.rs:709` in the same directory unions intersections; the
  hostname-less route branch ignores catch-all listener hostname `None`.
- `edgion-resources/src/resources/tls_route.rs:45` carries resolved listeners and
  effective hostnames as independent arrays, without their association.
- `edgion-gateway/src/routes/tls/gateway_tls_routes.rs:96` inserts that entire
  union into each listener table. Passthrough's SNI gate accepts any Gateway
  listener on the physical socket, not the selected route's attachment.

For example, R attaches to gw-a/a:8443 for a.example.com and gw-b/b:9443 for
b.example.com. Both tables receive both names. A third Gateway accepting
b.example.com on :8443 can let R receive that SNI on :8443 although R does not
attach to that logical listener. This violates routing isolation; a further
business-authentication bypass depends on backend controls and is not proven.

A hostname-less route attached to both catch-all and named listeners instead
loses catch-all reachability because the global union contains only named hosts.

## Acceptance Criteria

- Review a coordinated Controller/schema/Gateway plan preserving effective
  hostname and catch-all semantics per attachment, not as a route-global union.
- Prevent cross-listener hostname leakage and borrowing another Gateway's
  attachment on a shared physical listener.
- Ensure updates, removals and listener migration clear old associations while
  preserving unaffected listener snapshots and canonical incremental processing.
- Add future shared-topology integration scenarios for the three-Gateway/two-port
  case, catch-all plus named attachments, and update/delete hot reload; verify
  actual backend identity, not only a table match.
- Keep existing integration files frozen during the current campaign. This is
  not fixed by B0204's semantics-preserving storage optimization.
