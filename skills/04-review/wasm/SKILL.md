---
name: review-wasm
description: Review knowledge for wasm, grouped by decision type.
---

# wasm review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Regression guards

- [Shared-data byte gauges aggregate live stores and balance destruction](regression-guards/shared-data-gauge-live-stores.md)
- [Claim Wasm preload before copying task inputs](regression-guards/preload-claim-before-owned-inputs.md)
- [Wasm Body Counter Assertions Use Per-Request Deltas — FIXED](regression-guards/wasm-body-counter-delta-rerun-safe.md)
- [Wasm module cache key must fold in per-plugin runtime config — FIXED](regression-guards/wasm-module-cache-key-runtime-config.md)
- [Wasm OCI token endpoint 5xx must honor the retry policy — FIXED](regression-guards/wasm-oci-token-5xx-retry-classification.md)
- [Wasm Request Bodies Use One Complete Terminal Callback — FIXED](regression-guards/wasm-whole-request-body-callback-contract.md)
- [Wasm URL source identity and error redaction](regression-guards/wasm-url-source-identity-redaction.md)

## Intentional design

- [Bounded OCI artifact memory cache](intentional-design/bounded-oci-artifact-memory-cache.md)
