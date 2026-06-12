---
name: stream-init-failed-pattern-duplication-not-extracted
description: Use when reviewing findings that propose extracting the "clone → validate_and_init → tracing::error → init_failed" pattern that appears in `StreamIpRestriction::new` and `TlsRouteIpRestriction::new` into a shared trait + generic helper; documents why the duplication is intentionally kept.
---

# Stream IP-restriction `init_failed` guard duplication — DESIGN-TRADEOFF

## Conclusion

The ~12-line `let mut cfg = cfg.clone(); if let Err(e) = cfg.validate_and_init() { tracing::error!(...); true } else { false }` pattern is intentionally kept inlined in both `StreamIpRestriction::new` (`edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs`) and `TlsRouteIpRestriction::new` (`edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs`). The two sites use different config types from different protocol layers and a shared trait + generic helper would add an abstraction whose net code saving is in the single-digit-line range. Closed as ⚠️ accepted tradeoff (`plugins-stream-F003`, 2026-05-26).

Fix suggestions of the form "introduce a crate-internal `ValidateAndInit` trait with a blanket / two impls and a generic `init_with_validate<C: Clone + ValidateAndInit>(cfg, label) -> (C, bool)` helper, then route both `*::new` through it" are **not accepted**.

## Core Rationale

**1. Two callers, ~12 lines each, on the init path only**

Both occurrences are plugin constructors, called once per plugin instance at config-apply / hot-reload time. Neither sits on the data plane. The duplicated block is:

```rust
let mut cfg = cfg.clone();
let init_failed = if let Err(e) = cfg.validate_and_init() {
    tracing::error!(error = %e, label = "...", "Failed to initialize ...; all connections will be denied");
    true
} else { false };
```

Counting the proposed trait declaration, two impls, and the generic free function, the abstraction itself runs ~15 lines — net saving is in the single-digit-line range. The "drier" form does not pay for the new indirection.

**2. The two config types live in different protocol layers**

`ConnectionIpRestrictionRules` (used by `StreamIpRestriction`) is the L4 connection-level rule set under `types/resources/edgion_stream_plugins/`. `IpRestrictionConfig` (used by `TlsRouteIpRestriction`) is the HTTP-layer config under `types/resources/edgion_plugins/`, reused at the TLS-route stage 2 hook because that hook carries SNI-routed metadata. They are not sibling types of the same concept — they were chosen for their respective layers and their futures diverge (e.g. `IpRestrictionConfig` carries operator-facing `message` / `status` fields for HTTP error responses; `ConnectionIpRestrictionRules` cannot have those because there is no HTTP response at L4 connection time). A shared `ValidateAndInit` trait pretends they share a fail-closed initialization concept beyond a coincidental method-shape match.

**3. The fail-closed "truth source" is inside `validate_and_init`, not the wrapper**

Any genuine change to fail-closed semantics — what counts as an init failure, what default behavior takes over, what telemetry to emit — has to land inside each config's own `validate_and_init` (different validators on different rule structures) or inside each plugin's denial branch (different deny-reason strings, different `StreamPluginResult::Deny(...)` payloads). The wrapper's only contribution is "clone + log + return tuple"; the divergence risk the finding flags ("one update forgets the other") would still bite the `validate_and_init` bodies, which the helper does not abstract over. The helper buys structural similarity without buying semantic coherence.

**4. Contrast with the accepted sibling extraction (`plugins-stream-F001`)**

The same review pass landed `plugins-stream-F001` as a real-fix (`pub(crate) fn access_decision_to_stream_result(decision, on_deny)` in `stream_plugin_trait.rs`, consolidating the 5-arm `AccessDecision → StreamPluginResult` match across **three** stream plugins). F003's cost / benefit is materially worse:

| Axis | F001 (accepted) | F003 (declined) |
|---|---|---|
| Call sites | 3 | 2 |
| Abstraction needed | A single free function | A new trait + 2 impls + a generic free function |
| Type alignment | All 3 sites already produce `AccessDecision` (same enum) | 2 sites use different inherent `validate_and_init` methods on 2 unrelated structs |
| Behavior pinned at the boundary | Yes — deny literals stay byte-identical, downstream parsers locked | No — the wrapper only covers log+clone; divergence risk lives in each `validate_and_init` |

The duplication threshold for this kind of stream-plugin pattern is "3 sites of identical concrete code with a single owner ready to receive the abstraction." F003 hits neither half.

**5. Project policy on small-DRY refactors**

Other declined-extract precedents at comparable cost / benefit: `plugins-http-dsl-F009` (wholesale `call_builtin` extract declined) and [`plugins-http-F004`](../rate-limit/header-injection-duplication.md) (rate-limit header injectors diverge by design). Cross-layer abstraction for ≤10 net-line savings is not worth a new trait surface.

## Fix Suggestions Not Accepted

- "Introduce `ValidateAndInit` trait + generic `init_with_validate<C>` helper, route both constructors through it" — net saving ≤ single-digit lines; adds a cross-module trait for two callers in different protocol layers; does not abstract the actual divergence risk (which lives in each `validate_and_init` body).
- "Move the helper into `plugins/stream/mod.rs` as `pub(crate)` so both ip-restriction sites can import it" — same trade-off, just relocates the abstraction; still introduces a trait the two config types do not naturally share.
- "Macro that expands the boilerplate inline" — replaces 12 readable lines with a macro invocation; harder to grep, no win.

## Re-evaluation Triggers

Re-open only if:

- A third stream-side plugin appears with the same `clone → validate_and_init → log → init_failed` shape (the duplication count crosses the 2 → 3 threshold and the abstraction becomes proportionate).
- `ConnectionIpRestrictionRules` and `IpRestrictionConfig` are unified into a shared rule type (independent refactor) — the trait then has a natural single owner.
- A real fail-closed divergence bug is reported (one site updated, the other forgotten, producing observable user-visible behavior), confirming the latent risk became active.

## Reference Cases

- `plugins-stream-F003` — original finding, 2026-05-26 (this file).
- Sibling decisions: `plugins-http-dsl-F009` (giant-match extract declined), `plugins-http-F004` (rate-limit header-injection extraction declined; see [`../rate-limit/header-injection-duplication.md`](../rate-limit/header-injection-duplication.md)).
- Source: `edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs:13-29`, `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs:20-34`.
