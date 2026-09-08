# D0098 — Avoid Wasm Response In-flight Cache-key Allocation

- Status: pending
- Severity: low (bounded hot-path allocation)
- Discovered in: B0144 response-body-filter trait analysis
- Canonical task: [Avoid Wasm response in-flight cache-key allocation](../../../todo/avoid-wasm-response-inflight-cache-key-allocation.md)

On a cold or in-flight Wasm module miss, the response-body filter clones its cache-key String before `prepare_preload` rejects an already claimed load. Concurrent chunks may therefore allocate while one load is already in flight. Move owned-key creation after the preload claim under a separately reviewed Wasm cache-lifecycle batch; existing integration tests remain frozen.
