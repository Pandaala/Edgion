# ctrl-svc-05: Controller DNS resolver re-LISTs every Service cluster-wide every 15 s and resolves serially

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/services/dns_resolver/service.rs:140-170` (`run`: fixed ticker)
  - `edgion-controller/src/services/dns_resolver/service.rs:187-253` (`reconcile_with_runtime`)
  - `edgion-controller/src/services/dns_resolver/service.rs:255-294` (`list_owned_slices`, `list_services`: direct apiserver LIST)
  - `edgion-controller/src/services/dns_resolver/service.rs:296-374` (`resolve_and_apply`: sequential per Service)
  - `edgion-controller/src/services/dns_resolver/service.rs:570-625` (`effective_spec` → `fetch_httpdns_owner`: apiserver GET per Service per tick)
  - `edgion-controller/src/config/mod.rs:205-215` (enabled by default, `refresh_interval: "15s"`)

## Summary
The leader-only DNS resolver is enabled by default and, on every tick (15 s default), performs an
uncached `LIST` of **all Services** and all resolver-owned EndpointSlices in every watched namespace
(cluster-wide by default) directly against the apiserver, then resolves each controller-mode
ExternalName Service one after another inside the same task. For httpdns resolvers it also issues
an apiserver `GET` of the LinkSys per Service per tick. The controller already watches Services
through its reflector/handler (`handlers/service.rs`) and has the LinkSys in its processed cache, so
the polling LIST is redundant load; the serial resolution means one slow resolver delays every other
Service's refresh, and the effective refresh period grows linearly with the number of ExternalName
Services. DNS TTLs are ignored.

## Current behavior (evidence)
- `reconcile_with_runtime` calls `list_owned_slices` then `list_services` with
  `ListParams::default()` (`:198`, `:209`, `:273-294`) — no resourceVersion=0 cache read, no
  informer, no label/field selector on Services. This runs even when the cluster has zero
  ExternalName Services, because the service is started unconditionally on leadership
  (`conf_mgr/conf_center/kubernetes/center.rs:1316`) and `decide_action` filters only after the LIST.
- The loop `for svc in services { ... resolve_and_apply(...).await; }` (`:211-250`) awaits each DNS
  query in turn; `MissedTickBehavior::Delay` then pushes the next tick back.
- `effective_spec` → `fetch_httpdns_owner` does `Api::<LinkSys>::get` per Service per tick
  (`:615-623`), bypassing the processed (ReferenceGrant-checked, secret-resolved) LinkSys cache —
  which is also why an mTLS httpdns endpoint "degrades to fallback" (`:565-569`).
- Refresh is a fixed interval regardless of record TTL.

## Industry comparison
- **controller-runtime / client-go** controllers read from shared informer caches and react to
  watch events; periodic full LISTs against the apiserver are an explicit anti-pattern (they are the
  main source of apiserver load in large clusters, cf. API Priority & Fairness guidance).
- **Envoy STRICT_DNS / LOGICAL_DNS** resolves per cluster asynchronously with `dns_refresh_rate`
  and optional `respect_dns_ttl`, with each cluster refreshed independently.
- **Istio** resolves `ServiceEntry` DNS in the proxy (or via the DNS proxy), not by polling the
  apiserver.

## Impact
- Constant apiserver load proportional to total Service count (not ExternalName count), every 15 s,
  on every Edgion installation — a noticeable cost in clusters with tens of thousands of Services,
  and it competes with the controller's own watch traffic under APF.
- Head-of-line blocking: one unreachable resolver (timeouts) stalls the whole refresh cycle, so
  IP changes on healthy ExternalNames propagate late.
- An mTLS httpdns resolver silently cannot work in controller mode because the raw-CR read path
  skips secret resolution.

## Suggested direction
- Drive the work set from the existing Service cache: maintain the set of controller-mode
  ExternalName Services from the Service handler (or a dedicated cache view) and only iterate that
  set; read owned EndpointSlices from the EndpointSlice cache the controller already maintains.
- Resolve with bounded concurrency (`FuturesUnordered` with a small limit) and a per-Service
  deadline; schedule each Service by `max(min_refresh, min(TTL, refresh_interval))`.
- Take the httpdns LinkSys from the processed runtime store (already ReferenceGrant-checked and
  secret-resolved) instead of a raw `GET`.
- Consider defaulting `enabled` to false until a deployment actually uses controller-mode DNS, or
  skip the tick entirely when the cached ExternalName set is empty.

## Notes
`skills/04-review/controller/regression-guards/dns-owned-family-slice-convergence.md` covers slice
correctness (family convergence, fingerprinting) and explicitly keeps the IO in the leader job; it
does not address the polling/LIST model. `backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md`
is about gateway-side miss coalescing. Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-controller/src/services/dns_resolver/service.rs:204-250`: `list_services` then sequential `resolve_and_apply(..).await` per Service.
- `service.rs:279-295`: `ListParams::default()`, `Api::<Service>::all` when `watch_namespaces` empty — no cache, no selector.
- `edgion-controller/src/config/mod.rs:205-213`: `enabled: true`, `refresh_interval: "15s"` by default.
- `service.rs:612-623`: `fetch_httpdns_owner` does a raw `Api::<LinkSys>::get` per call.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-controller/src/services/dns_resolver/service.rs` unchanged since `feature-08-30`: uncached `ListParams::default()` Service LIST and serial `resolve_and_apply` per tick.
- `edgion-controller/src/config/mod.rs:209-210`: still `enabled: true`, `refresh_interval: "15s"` (only YAML-crate rename in this file).
