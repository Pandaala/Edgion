---
name: wasm-plugin-integrity-pin-isolation
description: Review Wasm integrity pins when plugin instances load, reuse, or refresh modules.
knowledge_type: regression-guards
---

# Each Wasm plugin runtime verifies its configured integrity pin

## Conclusion and scope

A configured Wasm plugin owns its private runtime and immutable configuration.
A different plugin cannot reuse its published compiled entry, even when both use
the same source. Each runtime verifies `WasmConfig.sha256` against fetched module
bytes before compilation/publication; a mismatched pin fails loading. Repeated
requests on one runtime reuse only that runtime's successfully verified entry.
OCI refresh also passes through module-byte verification before replacing the
last successful entry. Failure retains the old entry for that same configuration.

The bounded OCI artifact byte cache remains independent: reusing source bytes is
not permission to skip the configured runtime's final SHA-256 verification.
No global `runtime_cache_key` or per-request rehash is needed for this model.
See [plugin runtime isolation](../../wasm/regression-guards/wasm-module-cache-key-runtime-config.md).

## Historical correction

`contract-04` (2026-07-18) found that the former global source-keyed cache let a
second plugin with a contradictory pin reuse the first plugin's successful entry.
The old correction folded a case-normalized pin into the runtime cache key. That
was a valid correction for that cache; its requirements to coalesce identical
pins and preserve compiled entries across fetch-policy changes are superseded
by user-approved plugin instance ownership. Rebuilding configuration may fetch
and compile again, including when only credentials or a pin changed.

## Evidence and verification

Owner: `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`, runtime construction
and module loading before publication. The existing `eq_ignore_ascii_case` digest
comparison retains case-insensitive pin semantics; the absent pin remains an
explicit lack of operator pinning, not an inherited pin from another plugin.

The 2026-09-23 ownership run passed all 226 selected Gateway Wasm tests.
Coverage includes two runtimes using the same source with matching and mismatching
pins, a previously warm successful runtime not satisfying the other's bad pin,
and OCI final-byte pin verification. Historical runtime-key tests do not verify
this replacement ownership boundary.

Re-evaluate if compiled entries are shared across runtimes, verification is moved
after publication, or OCI byte reuse bypasses the final configured pin check.

## Resolution references

- `contract-04`: historical source-cache pin bypass correction.
- `wasm-tick-callout-grant-eviction-race`: runtime ownership redesign replacing
  the global compiled-entry cache.
