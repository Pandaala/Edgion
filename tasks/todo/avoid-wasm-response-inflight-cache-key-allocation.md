# Avoid Wasm Response In-flight Cache-key Allocation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | optimization |
| Priority | P2 |

## Requirements

- Reproduce concurrent response chunks encountering one cold or in-flight Wasm module load.
- Move owned cache-key creation after the canonical preload claim when safe, without adding locks or changing cache identity/lifecycle.
- Preserve fail-open/trap behavior, bounded metrics, response streaming and request-owned diagnostics.
- Add deterministic unit coverage; leave existing integration tests unchanged.
