# D0098 — Avoid Wasm Response In-flight Cache-key Allocation

- Status: resolved (2026-09-10)
- Severity: low (bounded hot-path allocation)
- Discovered in: B0144 response-body-filter trait analysis
- Maintained knowledge: [Claim Wasm preload before copying task inputs](../../../../skills/04-review/wasm/regression-guards/preload-claim-before-owned-inputs.md)

The original finding concerned response misses copying their cache key before
the canonical preload claim. The maintained knowledge owns the correction,
evidence, and verification limits. Existing integration tests remain unchanged.
