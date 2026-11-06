# ctrl-center-03: Kubernetes ingest watches core kinds cluster-wide as full objects and syncs unreferenced Services/EndpointSlices/ConfigMaps to every gateway

- **Category**: best-practice
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:718-724` (`watcher_config`: one global `label_selector`, no field selector)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:757-770` (`provider_kinds`: Secret, ConfigMap, Service, EndpointSlice)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:87-140` (`spawn`: metadata filter applied only to the processor copy)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs:452-480` (reflector `Store` holds full objects)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs` (`parse` → `Continue(eps)` for every slice)
  - `edgion-controller/src/conf_mgr/processor_registry.rs:169-177` (only `no_sync_kinds`, default ReferenceGrant/Secret, are withheld)

## Summary

With the default empty `watch_namespaces`, the controller LIST/WATCHes every Secret, ConfigMap,
Service and EndpointSlice in the cluster, keeps each one twice (kube reflector `Store` with
`managedFields` and all data, plus the processed `ServerCache` copy), and publishes every Service,
EndpointSlice and ConfigMap to every gateway whether or not any route references it. The only scoping
knob, `label_selector`, is applied identically to all kinds, so it cannot be used without labelling
every backend Service and every Secret. This is the scale/cost side of the broadcast model; the
security side is filed as `arch-01`.

## Current behavior (evidence)

- `watcher_config()` = `watcher::Config::default()` + optional `labels(selector)`; the same config is
  passed to every namespaced kind (`SpawnContext.watcher_config`); only GatewayClass drops the selector.
  There is no `fields("type!=helm.sh/release.v1")` or similar on Secrets.
- `MetadataFilterConfig` (`remove_managed_fields`, blocked annotations) is installed with
  `processor.set_metadata_filter`, i.e. after the reflector; the reflector writer stores the raw
  object. Helm release Secrets (gzip'd release manifests, up to ~1 MiB each, one per revision) are
  therefore resident in controller memory cluster-wide.
- `EndpointSliceHandler::parse` returns every slice unchanged; Service and ConfigMap likewise.
  `all_watch_objs(no_sync_kinds)` exposes them to conf_sync; every gateway receives every slice event
  in the cluster (kube-system, CI namespaces, unrelated apps).
- Phase-1 readiness (ctrl-center-02) has to LIST all of this within 15 s.

## Industry comparison

- Istio: `discoverySelectors` (namespace label selectors) bound what istiod watches/pushes; secrets are
  read with a `type!=helm.sh/release.v1` field selector and informers strip `managedFields` via a
  transform before caching; Sidecar/gateway scoping limits pushed endpoints.
- Envoy Gateway: translates only resources reachable from its Gateways; EDS contains only referenced
  backends.
- controller-runtime: `cache.Options.ByObject` per-kind selectors and `DefaultTransform` to strip
  managedFields before storing.

## Impact

Controller memory and startup LIST time scale with the whole cluster rather than with the gateway's
configuration; every gateway pays memory, CPU and bandwidth for endpoint churn of Services it will never
route to (and ring-buffer capacity is consumed by irrelevant EndpointSlice events, accelerating
`TooOldVersion` relists for that kind).

## Suggested direction

1. Short-term (local): add a Secret field selector excluding `helm.sh/release.v1` (and optionally
   service-account tokens); apply a reflector-level transform (`kube::runtime::WatchStreamExt::modify`
   before `reflector`) that strips `managedFields` and blocked annotations; make `label_selector`
   per-kind or exempt core kinds the way GatewayClass is exempted.
2. Medium-term: publish Service/EndpointSlice/ConfigMap to conf_sync only when referenced (the
   `service_ref_mgr` / config-map ref indexes already know the referenced keys) — a reachability filter
   on the publish side, keeping full watches for status/ResolvedRefs evaluation.
3. Optional: namespace-selector based discovery (Istio `discoverySelectors`) instead of a static
   `watch_namespaces` list.

## Notes

Related: `tasks/improve-issues/arch-01-conf-sync-broadcasts-all-secrets-to-every-gateway.md` (security
framing, per-gateway scoping). This file adds the ingest-side memory/LIST cost and the unreferenced
core-kind publication, which arch-01 does not analyze. Checked `controller/regression-guards/source-scoped-watch-snapshots.md`
and the ring-buffer intentional-design page (which assumes high EndpointSlice churn but not that it is
cluster-wide and unreferenced).

## Related findings

- arch-01 (security side of the same broadcast), ctrl-proc-01 (EndpointSlice requeue cascade; note ctrl-proc-01 shows no handler reads EndpointSlice, so the edge can be removed outright).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:717-724`: `watcher_config()` = default + optional labels; no field selector.
- `grep -rn "helm.sh\|fields(" edgion-controller/src` finds only metadata-annotation filtering, no `type!=helm.sh/release.v1` selector.
- `edgion-controller/src/conf_mgr/processor_registry.rs:166-178`: `all_watch_objs` excludes only `no_sync_kinds` (default ReferenceGrant, Secret); Service/EndpointSlice/ConfigMap go to every gateway.
- `handlers/endpoint_slice.rs` `parse` returns `Continue(eps)` unconditionally.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:718-724`: `watcher_config()` still default + optional labels, no field selector; file unchanged.
- `grep -rn helm.sh edgion-controller/src` still matches only metadata-annotation filtering (`config/conf_center/kubernetes.rs:317-344`), no `type!=helm.sh/release.v1` selector.
- `processor_registry.rs` and `handlers/endpoint_slice.rs` unchanged; Service/EndpointSlice/ConfigMap are still published to every gateway.
