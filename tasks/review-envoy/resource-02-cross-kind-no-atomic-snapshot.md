---
module: resource-config-model
title: Cross-kind config updates are not atomic — torn state between Gateway and Route is observable per request
severity: high
envoy_reference: xDS atomic snapshot (SnapshotCache) — all resource types are versioned together and the data plane atomically transitions from one consistent snapshot to the next
---

# Cross-kind config updates are not atomic — torn state between Gateway and Route is observable per request

## Envoy's design (the proven baseline)

In Envoy's xDS model, the control plane maintains a `SnapshotCache` where a single snapshot covers
all resource types (LDS, RDS, CDS, EDS) for a given Envoy node. When configuration changes span
multiple resource types (e.g., a new cluster and a new route that references it), the control plane
creates a new snapshot atomically and pushes it. The data plane transitions from the old consistent
state to the new consistent state without any moment where it sees a route pointing to a cluster that
doesn't exist yet. The guarantee is: an Envoy worker either sees the old full-consistent config or
the new full-consistent config, never a mix.

## Edgion's current design

Edgion syncs each resource kind independently over separate gRPC watch streams. Each kind has its own
`ClientCache<T>` with its own `CacheData`, its own debounce window (`BATCH_DEBOUNCE = 100ms`), and
its own `ConfHandler` invocation that mutates an independent runtime store
(`edgion-gateway/src/conf_sync/cache_client/cache_data.rs:11-12`).

On the Gateway side, the per-kind handlers write to independent runtime stores that are each
protected by per-kind `ArcSwap` or `RwLock`:

- `PluginStore` holds `EdgionPlugins` behind an `ArcSwap<PluginMap>`
  (`edgion-gateway/src/plugins/http/plugin_store.rs:134-135`).
- `GlobalHttpRouteManagers` holds `HTTPRoute` data behind per-listener `ArcSwap<DomainRouteRules>`
  (`edgion-gateway/src/routes/http/routes_mgr.rs:250`).
- `GatewayStore` holds `Gateway` in its own `RwLock`
  (`edgion-gateway/src/runtime/store/gateway.rs:67-68`).

These stores are updated by separate Tokio tasks (one per kind), each woken by its own `Notify`
after its own 100ms debounce. There is no cross-kind coordination primitive — no shared transaction
or epoch that guarantees all kinds advance together before any request thread reads the new state.

## The problem / gap

Consider the sequence: an operator adds a new `EdgionPlugins` resource and simultaneously adds a new
`HTTPRoute` that references it via `ExtensionRef`.

1. The `EdgionPlugins` update arrives first and the `PluginStore` is updated (plugins are now
   present).
2. Before the `HTTPRoute` update arrives and is applied, a request hits the data plane. The request
   matches no route (the old route table doesn't know about the new route). Safe.

Now consider the reverse (common during a rolling update or a large multi-resource config apply):

1. The `HTTPRoute` update arrives first and is applied. The new route is now in the route table.
   `ExtensionRefFilter` for this route holds a `(namespace, name)` reference to the `EdgionPlugins`
   resource.
2. Before the `EdgionPlugins` update arrives, a request matches the new route. The filter calls
   `get_plugin_runtime` on `PluginStore` and finds nothing. The behavior at that point depends on
   the per-filter fallback — some plugins fail open (skip the filter), some fail closed (401/503).
   Either outcome is incorrect: the operator's intent was to have the plugin active from the moment
   the route was active.

The 100ms debounce actually widens this window: events that arrive within the same 100ms batch are
compressed together, but events from different kinds that are processed by different debounce tasks
can diverge by up to ~200ms (one kind debounces, then the other).

This is the classic "dangling reference" window that xDS's atomic snapshot model was designed to
eliminate.

## Impact

- Every deployment or config change that spans `HTTPRoute` + `EdgionPlugins` (or `Gateway` +
  `EdgionTls`, or `HTTPRoute` + `Service`) creates a transient torn-state window where requests
  either bypass security plugins or are incorrectly rejected.
- The window is bounded by network jitter + debounce (~100-200ms) but is not zero and is not
  detectable by the operator.
- For security-sensitive plugins (authentication, rate limiting, IP restriction), a brief bypass
  window may violate compliance requirements.

## Verification note

Code confirmed. Minor line-number corrections from source verification:
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs:11-12` is the `BATCH_DEBOUNCE` constant and an adjacent comment; `CacheData` struct starts at line 17. The 100ms debounce claim is accurate.
- `handler.rs:43` does not contain a `GatewayStore` field; `GatewayStore` struct is in `edgion-gateway/src/runtime/store/gateway.rs:8`; the global `Arc<RwLock<GatewayStore>>` is at `edgion-gateway/src/runtime/store/gateway.rs:67-68`. The per-kind independent lock claim is accurate.
- The `ExtensionRefFilter` failure path (`extension_ref.rs`: `prepare_execution` returning `Err(())` → HTTP 500) is confirmed in `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs:134-144`.
- This finding is distinct from `sync-02`: sync-02 covers Service/EndpointSlice and TLS cert ordering; resource-02 covers the HTTPRoute → EdgionPlugins plugin-reference torn-state window via `ExtensionRefFilter`.

## Suggested direction

Adopt a generation/epoch mechanism to make cross-kind consistency observable and enforceable:

1. The Controller should assign a monotonic `config_generation` (or reuse `sync_version`) that is
   shared across all resource kinds in the same logical config change. Resources that are part of the
   same operator transaction should carry the same generation.
2. On the Gateway, an `ExtensionRefFilter` that resolves a plugin reference should consult the
   plugin store with the _route's_ sync_version as a minimum — if the plugin store has not yet
   reached that generation, the filter should use the previous valid plugin (or apply a safe default
   behavior) rather than treating the reference as dangling.

Alternatively (simpler near-term fix): make the `ExtensionRefFilter` distinguish between "plugin
never existed" (which may indicate a genuine config error) and "plugin not yet arrived" (which is
expected during a propagation window). A short per-request retry or a per-request "stale plugin"
fallback policy would reduce the severity of the window without requiring a full atomic snapshot.
