# Fix Wasm Shared-data Gauge Generation Identity and Lifecycle

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | Shared-data store and ModuleEntry lifecycle design |

## AI guide summary

Make `edgion_wasm_shared_data_bytes{module}` aggregate every simultaneously live store sharing the same bounded operator-facing module label and return to zero when the final store drops.

## Requirements

- Inventory shared-data store construction and drop across configuration variants, reload generations, OCI refresh generations, cache eviction, request leases, and tick tasks.
- Define a bounded process-wide aggregation identity without adding generation or digest labels that would accumulate stale high-cardinality series.
- Make interleaved mutation and arbitrary store-drop order update one coherent aggregate.
- Preserve per-store CAS, capacity, no-eviction, no-TTL, process-local, and ModuleEntry lifetime semantics.
- Preserve the canonical metrics registry and inventory; do not add request-path logging or tracing.
- Do not modify existing integration-test files.

## Candidate scope

- `edgion-gateway/src/plugins/http/wasm/shared_data.rs`
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`
- `edgion-gateway/src/observe/metrics/registry.rs`
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs`
- English and Chinese Wasm user and metrics documentation as required by the final design

## Acceptance criteria

- Same-label stores contribute additively throughout overlapping lifetimes.
- Mutations and drops cannot lose another live store's contribution.
- Dropping the last store sets the existing series to zero.
- Registration/drop races are deterministic and bounded.
- Inline tests cover overlapping old/new generations and arbitrary drop order; no integration-test issue is required.
- Workspace format/check/Clippy, metrics inventory, no-tracing, English-only, source-policy, and frozen-path guards pass.
