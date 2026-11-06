# ctrl-proc-01: EndpointSlice churn cascades into Route, Gateway, EdgionTls and policy republish storms

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs:20-56,70-76`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs:390-411` (also `grpc_route.rs:280`, `tls_route.rs:176`, `tcp_route.rs:179`, `udp_route.rs:179`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs:79-98,232-262`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:724-782`
  - `edgion-controller/src/conf_mgr/runtime.rs:179-181`

## Summary
Every EndpointSlice event requeues every Route that references the Service, even though no Controller
handler reads EndpointSlice contents. Each Route reconcile then requeues its parent Gateways without
checking whether anything changed. Each Gateway reconcile allocates a new attachment-proof epoch. As a
result, every EdgionTls and EdgionAcme attached to that Gateway is requeued and republished. Because the
processor republishes on every reconcile (see ctrl-proc-02), ordinary pod scaling or rolling deploys
cause repeated republishes of Routes, Gateways, EdgionTls and policies to every data plane.

## Current behavior (evidence)
1. **EndpointSlice → Routes, with no consumer of the data.** `EndpointSliceHandler::on_change`/`on_delete`
   (`endpoint_slice.rs:70-76`) call `requeue_dependent_resources`. That function requeues every
   `service_event_refs` entry for the Service: all Routes, BackendTLSPolicy, EBTP and mirror users
   (`endpoint_slice.rs:32-55`). No handler reads EndpointSlice from the Controller cache:
   `grep '"EndpointSlice"'` under `conf_mgr/sync_runtime` matches nothing outside the handler itself.
   Route `ResolvedRefs` reads only the Service (`route_utils.rs:367`), and the Gateway resolves endpoints
   on its own. Every endpoint change (pod readiness flip, scale, rolling update) therefore reprocesses
   every Route behind that Service, and the reprocess changes nothing.
2. **Route → Gateway, always.** Each Route `on_change` calls `requeue_parent_gateways`
   (`handlers/mod.rs:79-98`) for every parentRef, whatever the outcome (`http_route.rs:405` and its
   siblings). `update_tracker_accepted_only` (`route_utils.rs:637`) already returns whether the attachment set
   changed, but nothing uses that value on the apply path. Only TCP/UDP/TLS `on_delete` use it. The
   architecture page `skills/01-architecture/01-controller/06-requeue-mechanism.md` §5 says
   `requeue_parent_gateways()` is "only triggered when the attachment list actually changes", which
   contradicts the current code.
3. **Gateway reconcile → new epoch → every EdgionTls/EdgionAcme.** `GatewayHandler::finalize_transport`
   (`gateway.rs:724-734`) calls `next_gateway_attachment_epoch()` (a global `fetch_add`, `runtime.rs:179`)
   on every reconcile. `resolved_attachment_proof` therefore differs every time, even when the attachment
   digest is unchanged. `after_publish` (`gateway.rs:775-781`) then requeues every EdgionTls/EdgionAcme
   in the Gateway's index "on every epoch". Each of them gets a new proof and is republished. On the data
   plane, `TlsStore::apply_partial_update` re-validates each updated entry
   (`edgion-gateway/src/tls/store/tls_store.rs:315-345`).
4. **Route → backend policies, always.** `requeue_backend_policies_for_owner` (`handlers/mod.rs:232-262`)
   requeues every BTP/EBTP attached to the Route's Services on every Route reconcile. Policy
   reconciliation scans the raw policy store (`policy_ancestor_admission.rs:136`,
   `edgion_backend_traffic_policy.rs:144-157`), so with P policies an endpoint storm costs O(P) work per
   policy.
5. Each Route reconcile also logs at `info` with the full `parent_refs` Debug output
   (`http_route.rs:374-384`), so every endpoint flip adds one INFO line per Route.

Concrete chain for one pod readiness change behind a Service used by R routes on a Gateway with T EdgionTls:
R Route reconciles and publishes, then ≥1 Gateway reconcile and publish (after the 100 ms coalescing delay),
then T EdgionTls reconciles and publishes, plus every BTP/EBTP on that Service. The per-kind conf_sync
ring buffers default to 200 events (`skills/04-review/conf-sync/intentional-design/ring-buffer-fixed-capacity-toooldversion-relist.md`).
They wrap quickly, which pushes data planes into TooOldVersion full relists.

## Industry comparison
- Istio pilot treats an EndpointSlice change as an EDS-only incremental push (`ConfigUpdate` with
  `Full=false`). It never rebuilds Route/Listener/Secret config for an endpoint change. Gateway API status
  in Istio is computed from config, not from endpoints.
- controller-runtime `Watches(..., handler.EnqueueRequestsFromMapFunc)` registers only edges whose source
  data the reconciler actually reads. Envoy Gateway's translator likewise maps EndpointSlice only into the
  xDS EDS resources, not into Route status.

## Impact
Control-plane CPU and conf_sync bandwidth scale with pod churn × routes × attached TLS objects. Gateway
data planes repeatedly re-validate certificates and receive republished Routes and Gateways. The ring
buffer wraps, which forces full relists across the fleet during rolling deploys, when stability matters
most.

## Suggested direction
1. Remove the EndpointSlice → dependents requeue, keeping only the conf_sync relay. If a future handler
   needs endpoint data, register a dedicated edge for that handler only.
2. Requeue parent Gateways from Route `on_change` only when `update_tracker_accepted_only` reported a change
   or the parent set changed (dropped/added parents). Carry the "attachment changed" bit from
   `update_status` to `on_change`, or move tracker maintenance into `on_change`.
3. Allocate a new Gateway attachment epoch only when the attachment projection changes. Compare the digest
   (plus instance/generation) with the last published one in `GatewayRouteIndex`, and reuse the previous
   epoch otherwise. The epoch stays monotonic, so ABA fencing for ACME is preserved. Then requeue
   EdgionTls/EdgionAcme only when the proof actually changed.
4. Requeue backend policies from a Route only when its forwarding Service/backend set changed (the dropped
   and added sets are already computed by `register_service_backend_refs`).
5. Downgrade the per-route listener-resolution log to `debug`.

## Notes
- Checked `skills/01-architecture/01-controller/06-requeue-mechanism.md`. It describes the Route → Gateway
  edge as "refreshed on reconciliation" and Gateway → EdgionTls as "every publication epoch", but it does
  not account for the epoch being bumped on every reconcile or for EndpointSlice feeding the chain. Its §5
  states the opposite of the code.
- Checked `skills/04-review/handler-requeue/*` and `acme/regression-guards/acme-http01-gatewayref-serving-ownership.md`
  ("Every Gateway publication epoch requeues EdgionAcme proof consumers"). Item 3 keeps that contract,
  because consumers still refresh on every new epoch. Only gratuitous epochs are removed.
- **Overlap:** the (dropped) ctrl-center unconditional-republish draft already covers the EndpointSlice → Route
  requeue and the missing publish-equality check. That finding suggests narrowing EndpointSlice requeues to
  port-set changes, but no Controller handler imports or reads `EndpointSlice` (only `endpoint_slice.rs`
  itself does), so removing the edge entirely is safe. **This finding's unique content is items 2-4**:
  the unconditional Route → Gateway requeue, the per-reconcile Gateway epoch that fans out to every
  EdgionTls/EdgionAcme, and the unconditional Route → policy requeue. Publish suppression alone would
  *not* fix these, because the epoch changes the Gateway/EdgionTls bytes on every reconcile, so they are
  never equal.
- No task under `tasks/` covers this.

## Related findings

- ctrl-center-03, ctrl-proc-02 (no output-equality gate), ctrl-proc-03. Also: K8s 410 relists and etcd compaction resyncs republish every object, and no-op events consume ring-buffer capacity, pushing slow gateways into TooOldVersion relists (from ctrl-center review).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs:20-76`: `on_change`/`on_delete` → `requeue_dependent_resources` over all `service_event_refs`; no other controller handler reads EndpointSlice (only `service_ref.rs` names it, for key lookup).
- `handlers/http_route.rs:405-406`: `requeue_parent_gateways(..)` and `requeue_backend_policies_for_owner(..)` unconditionally on every `on_change`.
- `handlers/gateway.rs:724-733`: `finalize_transport` calls `next_gateway_attachment_epoch()` every reconcile; `:775-781` requeues every EdgionTls/EdgionAcme "on every epoch".
- Note: the cross-reference to the (dropped) ctrl-center unconditional-republish draft is stale (that file does not exist; current ctrl-center-03 is the ingest-scope finding). The publish-equality half lives in ctrl-proc-02.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `handlers/endpoint_slice.rs:20-76`, `handlers/http_route.rs:390-411`, `handlers/mod.rs:79-98,232-262` and `runtime.rs` are unchanged since `feature-08-30`.
- `handlers/gateway.rs:728`: `next_gateway_attachment_epoch()` still called on every `finalize_transport`; `after_publish` still requeues every EdgionTls/EdgionAcme per epoch.
- No new handler-requeue or requeue-mechanism page accepts the cascade.
