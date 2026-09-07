---
name: small-object-clones-reconcile-config-sync
description: Use when reviewing findings that flag small-object clones, to_vec(), or config.clone() on reconcile or config-sync paths as P3 performance issues; acceptance bar requires request-path proof or MB-scale copy.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Small-Object Clones on Resource Reconcile / Config Sync Paths Are Not Issues

**False-positive scenario**: on "config-change-triggered" paths such as Controller `resource_processor` / Gateway `conf_handler`, seeing `.clone()` / `.to_vec()` / full HashMap copies on a single resource, single entry, single service is flagged as "P3 unnecessary clone / should switch to Arc<T>". Typical examples:

| Location | Pattern | Trigger frequency |
|------|------|---------|
| `handlers/secret.rs` clones the same secret once for init_accumulator and once for secret_store | Secret resource reconcile | Per Secret create/update |
| `lb/lb_policy/policy_store.rs::get(service_key) -> Vec<LbPolicy>` | DashMap entry policies `.to_vec()` | HTTPRoute reconcile |
| `plugins/runtime/plugin_runtime.rs` `add_from_httproute_filters_inner` / `create_request_filter_from_edgion` / `create_upstream_response_filter_from_edgion` calling `XxxFilter::new(config.clone())` | Inputs are `&[HTTPRouteFilter]` / `&EdgionPlugin`; filter constructors take config by value | PluginRuntime build (reload / route change) |

**Reality**: these clones are all on reconcile / config-sync paths; call frequency = K8s resource change frequency (human/operator scale, with bursts at most at reload time); **not per-request**. The cloned objects are usually single resources (KB scale); the entire path is dominated by etcd RTT / gRPC serialization; a few String/HashMap clones' alloc + memcpy are immeasurable in totals.

To switch them to Arc one usually needs:
- Modifying public type signatures (`WorkItemResult::obj`, `PolicyStore::get` return type, etc.), cascading to many callers/tests;
- Or adding closure APIs (`with_policies(&self, key, F)`) that change upper-layer usage.

No observable benefit on the cold path, but real change surface and regression risk.

Special note on the plugin_runtime line (added 2026-05-08): audits often misread this as "eliminate clone"; in reality:
- The current call-chain shape is `&[HTTPRouteFilter]` / `&EdgionPlugin` → `match` to `&Config` → `XxxFilter::new(config: OwnedConfig)`. To get owned `T` from `&T` you **must** clone.
- Moving the clone into the constructor (changing it to `new(&Config)` with internal clone) is just relocation; the alloc count is unchanged.
- To truly eliminate the clone, the entire call chain must be passed by value (`Vec<HTTPRouteFilter>` / `Vec<RequestFilterEntry>` / `EdgionPlugin`), cascading to `RouteRuntime` construction, hot-update logic, and write-style changes for all `WrongStage` branches still needing to borrow plugin to get name — impact far exceeds the "local clone optimization" label.
- Switching to `Arc<Config>` would touch every plugin struct/trait; not worthwhile.
- Trigger frequency is reload / route change only; not the request path.

**Verdict**: whether on the hot path decides everything—
- Reject: "P3 small-object clones on reconcile / config sync paths" findings.
- Reject: "plugin_runtime build-time config.clone()" findings (API shape decides; relocation does not save; full-chain by-value change is too large).
- Accept: "full_set whole map copy and synced per second" amplification scenarios (existing cases: LinkSys `replace_all` uses `Arc::clone` to share with the dispatch task, avoiding whole HashMap copy; ACME `challenge_store::lookup` switched to `Bytes`, saving one String alloc + memcpy per HTTP-01 verification request).
- Acceptance bar: the finding must prove either "triggered per second on the request path" or "single copy ≥ MB scale"; otherwise close per this entry.
