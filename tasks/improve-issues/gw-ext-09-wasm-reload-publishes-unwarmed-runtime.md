# gw-ext-09: Wasm configuration reload publishes an unloaded runtime, so traffic stalls on fetch/compile and a registry outage breaks working routes

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/plugin.rs:77-124` (`build` → `spawn_preload`, best-effort), `plugin.rs:772-780` (`run_request` → `get_or_load().await`), `plugin.rs:789-799` and `613-621` (sync stages → `fail_mode("not loaded")` / skip), `edgion-gateway/src/plugins/http/wasm/vm_manager.rs:540-593` (`get_or_load_with_linker`: single-flight under `slot.state` lock, `LOAD_RETRY_SECS = 30` negative cache), `vm_manager.rs:604-730` (`load_inner_with_linker`: fetch → hash → analyze → compile → warm instance), `vm_manager.rs:856-883` (`fetch_bytes`), `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs:70-71,113-121` (fetch timeout 10 s × 3 attempts)

## Summary
Every reconstruction of a Wasm plugin creates a fresh, empty `WasmRuntime` and publishes it to
the data plane immediately. Reconstruction happens on any change to its EdgionPlugins
resource generation, including edits to unrelated entries in the same resource. Loading the
module (URL/OCI fetch, SHA-256, analysis, Cranelift compile, guest init) starts only as a
best-effort background preload. Until it finishes:
- every request-stage request on the route waits in `get_or_load()` behind the single-flight
  lock;
- standalone response-header entries answer with `fail_mode("not loaded")`, which is HTTP
  500 by default;
- standalone body entries silently skip the guest.

If the fetch or compile *fails*, for example because the registry is temporarily
unreachable, the new runtime has no last-good module to fall back on. The route then fails
or fails open for 30-second retry windows, although the previous generation was healthy and
the module bytes did not change.

## Current behavior (evidence)
- `WasmPlugin::build` → `global_vm_manager().new_runtime(..)` creates a new `RuntimeState`
  with an empty `LoadSlot`. It then calls `plugin.runtime.spawn_preload()` and returns
  immediately. The comment says "best-effort; lazy load covers the miss" (`plugin.rs:105-123`).
- `run_request` → `self.runtime.get_or_load().await` (`plugin.rs:773`). When nothing is
  published yet, it takes `self.slot.state.lock().await`. The preload task holds that lock
  for the whole `load_inner_with_linker`, so requests queue for the full fetch and compile
  (`vm_manager.rs:546-566`). No request deadline applies to this wait. Only later callouts
  use `request_deadline`.
- URL fetches allow `timeout_ms` up to 60 s per attempt (`wasm.rs:586`) with a default of
  10 s × 3 attempts (`wasm.rs:113-121`). A slow registry can therefore hold requests for tens
  of seconds. Compiling a large module (up to `MAX_MODULE_BYTES = 64 MiB`) on the
  `prepare_blocking` pool takes seconds.
- On failure, `LoadState::Failed` and `load_retry_due_ms = now + 30 s` are set
  (`vm_manager.rs:579-591`). Every request then takes `fail_mode("load")`. The previous
  runtime's healthy `ModuleEntry` is retired by `WasmRuntime::drop` when the old generation
  is released (`vm_manager.rs:469-479`), so it cannot serve as a fallback.
- `skills/01-architecture/02-gateway/05-plugin-system.md` (Wasm section) accepts that
  "configuration reconstruction may reset shared data and repeat compilation". It does not
  address request stalls, 500s on sync stages, or losing a working module when a reload
  fetch fails.

## Industry comparison
- **Envoy / Istio**: Wasm `remote` data sources are fetched and the VM is created *during
  config warming*, before the new listener or filter chain takes traffic. On fetch failure
  the update is rejected (NACK) and the old configuration keeps serving. Istio's agent goes
  further and pre-downloads modules (`WasmPlugin` image fetch in istio-agent), rewriting the
  config to a local file before Envoy sees it. Identical `vm_config` (same `vm_id` and code
  hash) reuses the existing VM across updates.
- **Kong / APISIX** (Lua and Wasm filters): configuration is validated and filters are
  compiled before the new router or plugin iterator is swapped in.

## Impact
- Every config edit to a resource that contains a Wasm entry causes a latency spike or
  errors on its routes. For the sync response stages, every response in the window gets a
  500 by default.
- With URL/OCI sources, a registry blip coinciding with any config change turns into an
  outage lasting at least 30 s per retry window, even though the module digest (`sha256`)
  did not change.
- The Controller cannot detect any of this: it validates the config but does not load
  modules. The resource reports Accepted while the data plane fails.

## Suggested direction
1. Warm before publish: make Wasm construction a two-phase step in which the new
   `PluginGeneration` is published only after its Wasm runtimes have loaded, or after a
   bounded warm timeout. Until then, keep routing to the previous generation. The
   `PluginTaskOwner` / `PreparedModule` machinery already separates "prepared" from
   "published".
2. Reuse identical modules across reconstruction: when `(source digest or sha256, vmConfig,
   pluginConfig, timeouts)` is unchanged, hand the previously published `ModuleEntry` or at
   least the compiled `Module` to the new runtime. An `InstancePre`/`Module` can be cloned
   cheaply. Mutable state (pool, shared data, tick) can still be per-runtime if isolation is
   required. Envoy's `vm_id` + code-hash reuse works the same way.
3. On reload fetch or compile failure with a previous good module of the *same* `sha256`,
   keep serving that module and report the failure through status or metrics, as Envoy does
   with a NACK, instead of failing traffic.
4. Bound the cold wait in `run_request` with the request deadline, and map "not loaded yet"
   to 503 rather than 500.

## Notes
- Checked `skills/04-review/wasm/regression-guards/wasm-module-cache-key-runtime-config.md`
  (per-plugin runtime isolation, user-approved). That page accepts *repeated compilation*
  and *fresh shared data*. The new evidence is a different cost: an availability gap and
  loss of the last-good module on every reconstruction. Option 2 can preserve the approved
  isolation of mutable state while sharing only immutable compiled artifacts. The page's
  own re-evaluation trigger is "a compiled-artifact cache is introduced".
- `preload-claim-before-owned-inputs.md` and `tick-quiescence-on-zero-reference-eviction.md`
  cover preload and teardown mechanics, not publication ordering.

## Related findings

- gw-plugrt-02, waf-06 (same per-entry rebuild root cause).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/wasm/plugin.rs:105-122`: `build` creates a fresh runtime via `new_runtime` and only calls best-effort `spawn_preload()` before the plugin is returned for publication.
- `vm_manager.rs:546-566`: with no published entry, `get_or_load` waits on `slot.state` for the full fetch/analyze/compile/warm with no request-deadline bound; on failure `:579-590` sets `LoadState::Failed` with a 30 s retry window.
- `vm_manager.rs:469-479`: `WasmRuntime::drop` retires the previous published entry, so no last-good module survives a reconstruction; `plugin.rs:789-799` sync stage returns `fail_mode("not loaded")` while cold.
- Nuance: OCI artifact bytes are retained in the bounded in-memory cache (`skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md`), which softens the registry-blip case for OCI sources, but URL sources and the compile/guest-init stall are unaffected. `wasm-module-cache-key-runtime-config.md` accepts repeated compilation, not the availability gap.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `wasm/plugin.rs`, `wasm/vm_manager.rs` and `plugin_configs/wasm.rs` are unchanged; `LOAD_RETRY_SECS = 30` is still at `vm_manager.rs:92`, and `build` still only spawns a best-effort preload.
- `05-plugin-system.md` changed only in its AI session capability section; the cited Wasm review pages are unchanged.
