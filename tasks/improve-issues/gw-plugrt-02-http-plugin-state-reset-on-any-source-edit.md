# gw-plugrt-02: Any edit to an EdgionPlugins resource resets all of its HTTP plugins' runtime state

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/publication/publisher.rs:353-366` (`set_http`), `:92-96` (`same_source`); `edgion-gateway/src/plugins/http/plugin_generation.rs:12-27` (`PluginGeneration::new` → fresh `EntryCache`); `edgion-gateway/src/plugins/runtime/entry_bundle.rs:134-138` (cache per generation); `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs:60-66,108-114` (per-instance `OnceLock<Rate>`); `edgion-gateway/src/plugins/http/common/outbound/client.rs:60-80,114-150` (per-instance `HealthState`, client and health task); contrast `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` and `edgion-gateway/src/plugins/common/mod.rs:9-37`

## Summary
HTTP plugin entry instances are cached per *source generation*, and a new generation is created whenever the
serialized `EdgionPlugins` differs in any way (another entry, an annotation, `resourceVersion`). Every entry
of that resource is therefore reconstructed, discarding in-memory state: `RateLimitLocal` counters reset to
zero, managed outbound clients (ForwardAuth/webhook/ExtProc-style dependencies) lose their connection pools
and passive/active health state, and per-instance caches start cold. The stream `ConnectionRateLimit`
deliberately solved the same problem with a process-lifetime registry keyed by `entry_scope`; HTTP did not.

## Current behavior (evidence)
- `Inputs::set_http` (`publisher.rs:353-366`) allocates a new id and a new `PluginGeneration` whenever
  `same_source` (full `serde_json::Value` equality, `:92-96`) is false. `PluginGeneration::new`
  (`plugin_generation.rs:24-27`) starts with an empty `EntryCache`; entries are shared only within one
  generation ("Slots never cross this immutable source boundary", `plugin_generation.rs:12-13`).
- `RateLimitLocal` holds `rate: OnceLock<Rate>` in the instance (`rate_limit_local/plugin.rs:60-66`);
  a new instance starts a new pingora `Rate` (both current and previous windows empty).
- `ManagedOutboundClient` owns a `HealthState::new()` that starts `healthy = true` with zero failures and
  aborts its active-health task on drop (`outbound/client.rs:60-80,113-150,268-280`); a new instance also
  builds a new reqwest client (new keep-alive pool / TLS sessions).
- Policy-only edits keep the generation (`plugin_generation.rs:12-13`), so the reset is specific to source
  edits — which are the common operator action.
- The stream sibling explicitly rejects this model: `skills/04-review/rate-limit/regression-guards/connection-rate-limit-entry-scoped-buckets.md`
  lists "Give each runtime its own disposable bucket — it would drop token state on reload" under
  *Fix Suggestions Not Accepted*, and `plugins/common/mod.rs:9-37` documents `entry_scope` as "stable across
  hot reload". HTTP `RateLimitLocal` ignores that identity.

## Industry comparison
- NGINX `limit_req_zone`/`limit_conn_zone` shared-memory zones survive `nginx -s reload` when name and size
  are unchanged.
- Kong stores local rate-limiting counters in a shared dict keyed by plugin id + identifier + period; a
  declarative config reload that keeps the plugin id keeps the counters.
- Envoy keeps cluster health/outlier state across CDS updates when the cluster definition is unchanged.

## Impact
- `RateLimitLocal` with long intervals (e.g. 1000/hour) grants a fresh full budget on every unrelated edit
  of the same resource; a team editing a CORS or header entry silently resets the limiter for all routes
  that reference the bundle.
- A dependency that is known down (open passive circuit) is considered healthy again after any edit, and
  the gateway re-handshakes to auth/webhook services on every config change.
- Behavior differs between HTTP and stream rate limiting for the same "edit config" action.

## Suggested direction
Key stateful per-entry runtime objects by the stable entry identity (`entry_scope(resource_key, alias, index)`)
plus a hash of *that entry's* configuration, not by the whole-resource generation:
- Option A (recommended): a small process registry (as `LocalGateRegistry` does for stream) from
  `(entry_scope, entry_config_digest)` to the stateful core (`Rate`, `HealthState` + client); new
  generations whose entry config is unchanged rebind to the existing core; release when no generation
  references it.
- Option B: reuse unchanged entry outcomes across generations in `EntryCache` by comparing per-entry
  serialized config, so unrelated entries are not reconstructed at all.
Either way, a change to the limiter's own `rate`/`interval` may legitimately start fresh (or rescale), but
unrelated edits must not.

## Notes
- Checked `plugin-runtime/intentional-design/edgionplugins-key-scoped-last-known-good.md` (about retaining
  DSL/WAF executables, not state continuity) and `scoped-policy-runtime-publication.md` (generation
  ownership). No page accepts state loss on source edits for HTTP; the stream page rejects it.
- Not tracked under `tasks/`.

## Related findings

- gw-ext-09 (Wasm reload publishes an unwarmed runtime), waf-06 (WAF generation recompiled per EdgionPlugins edit). Common root: EdgionPlugins edits rebuild every entry; a per-entry content-digest reuse would fix all three.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/publication/publisher.rs:92-96,353-366`: `same_source` compares the full serialized resource (including metadata), so any edit creates a new `PluginGeneration` with an empty `EntryCache` (`plugins/http/plugin_generation.rs:12-27`).
- `RateLimitLocal` keeps `rate: OnceLock<Rate>` per instance (`rate_limit_local/plugin.rs:109-115`), with no `entry_scope` registry in `rate_limit_local/` or `common/outbound/`, unlike the stream `ConnectionRateLimit` `LocalGateRegistry`.
- `skills/04-review/rate-limit/regression-guards/connection-rate-limit-entry-scoped-buckets.md` explicitly rejects disposable per-runtime buckets for stream; no page accepts the HTTP reset.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `publisher.rs` (`same_source`, `set_http`), `plugins/http/plugin_generation.rs`, `rate_limit_local/plugin.rs` and `common/outbound/client.rs` have no diff.
- No HTTP-side entry-scoped registry was added.
