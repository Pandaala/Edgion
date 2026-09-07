# Redact and Bound Wasm URL Source Identity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | security |
| Priority | P1 |
| Issue | N/A |
| Depends on | Canonical Wasm source identity design |

## AI guide summary

Replace complete configured Wasm URLs in metrics labels and load errors with one canonical bounded, stable, credential-free source identity.

## Requirements

- Inventory every consumer of `WasmSourceSpec::metrics_label()` and every Gateway error or log that can render a configured source URL.
- Define one canonical bounded identity that cannot include userinfo, query parameters, fragments, credentials, or attacker-controlled unbounded path text.
- Preserve useful operator correlation across equivalent source generations without adding digest, generation, or request labels that accumulate stale high-cardinality series.
- Sanitize outbound fetch and cache errors before binary-owned logging, PluginLog, access-log diagnostics, and metrics.
- Keep outbound HTTP/TLS/egress resolution in the existing canonical mechanisms.
- Update metrics inventory and English/Chinese Wasm documentation where the operator-visible label contract changes.
- Do not modify existing integration-test files.

## Candidate scope

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs`
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`
- Wasm metrics inventory and user documentation selected by the final design

## Acceptance criteria

- URLs containing userinfo, query secrets, fragments, and long paths never appear in labels, errors, logs, or diagnostics.
- Source labels are bounded and stable for the documented identity boundary.
- Existing local, inline, URL, and OCI source selection and cache semantics remain unchanged.
- Unit tests cover redaction, bounds, equivalent identities, and fixed fetch failures; no integration-test issue is required.
- Workspace format/check/Clippy, agent-doc, metrics inventory, no-tracing, English-only, source-policy, and frozen-path guards pass.
