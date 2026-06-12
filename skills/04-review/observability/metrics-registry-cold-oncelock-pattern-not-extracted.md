---
name: metrics-registry-cold-oncelock-pattern-not-extracted
description: Decision record for observe F-011 — the 12 cold `OnceLock<Counter/Gauge>` fields in `GatewayMetrics` (struct field + `new()` initializer + `record_*` method, three wiring points per metric) are intentionally not folded into a `cold_counter!` / `cold_gauge!` declarative macro.
---

# observe F-011 — Cold-metric OnceLock boilerplate is not extracted into a macro

## Conclusion

`edgion-gateway/src/observe/metrics/registry.rs` declares 12 cold metrics as
`OnceLock<Counter>` / `OnceLock<Gauge>` fields. Each metric is wired in three
places:

1. Struct field declaration on `GatewayMetrics`.
2. `OnceLock::new()` initializer inside `GatewayMetrics::new()`.
3. A dedicated `#[inline] pub fn record_*` method that calls
   `get_or_register_counter(&self.<field>, names::<NAME>)`.

Folding this into a `cold_counter!(field, NAME_CONST)` /
`cold_gauge!(field, NAME_CONST)` macro that emits all three artefacts at once
is **not accepted**. Findings of the form "introduce a declarative macro that
collapses the cold-metric registration into a single line" should be closed as
⚠️ accepted tradeoff without code change.

## Core Rationale

**1. "Silent `None`" risk is not real under the current struct shape.**

`GatewayMetrics::new()` uses a literal struct expression `Self { field: ..., ... }`
listing every field explicitly. Missing a field there is a hard
compile error, not a silent `None`. The "silent `None` if field and `new()`
diverge" hazard cited in the finding can only materialize if the codebase
later switches to `..Default::default()` (or `OnceLock::default()`) — which
isn't the current style and is itself a reviewable change. The real residual
risk is the much weaker "engineer forgets to add the `record_*` method when
adding a new metric", which is "forgetting to add a function" — a class of
risk present everywhere in the codebase, not specific to the OnceLock pattern.

**2. Macros hide the registration mechanics from grep.**

The current code is grep-friendly: `OnceLock::new()`, `get_or_register_counter`,
the `#[inline] pub fn record_*` definitions, and the `names::*` constants all
appear verbatim. A `cold_counter!(slot, name)` invocation hides the
`OnceLock::get_or_init`, the `#[inline]` attribute, and the generated public
method name from text search. Past precedent in this codebase rejects this
exact tradeoff:

- `observability/protocol-logger-oncelock-pattern-not-extracted.md` (F-010)
  declined a sibling refactor that would replace per-protocol
  `OnceLock<Arc<AccessLogger>>` plumbing with a `ProtocolLogger<T>` wrapper or
  `define_protocol_logger!` macro, on the same "macro hides
  `OnceLock::set` failure path and string literals from grep" grounds.
- `architecture/style-consolidation-thresholds.md` codifies the general rule:
  "Three similar lines is better than a premature abstraction; duplication
  itself is not a problem unless backed by real maintenance or reading cost."
- `architecture/conf-handler-simple-map-pattern.md` declined
  a different shape (a `LazyLock<RwLock<HashMap>>` + three-arm `add` / `update`
  / `remove` trio) on a rationale that still applies here: stable Rust offers
  no generic-static keyed on a wrapper type, so any "extract the storage cell"
  refactor leaves per-instance static declarations in place and the macro
  savings shrink correspondingly.

**3. The 1:1 macro shape cannot express the composite methods.**

Not every cold metric is a 1-field-→-1-method mapping. Real shapes that a
`cold_counter!(field, NAME)` arm cannot generate:

- `status_update_failed` increments both `status_update_total` AND
  `status_update_failed` in the same call — two `OnceLock` slots, one method.
- `gateway_connected` / `gateway_disconnected` share the same
  `controller_connected_gateways` gauge but are two methods with opposite
  ±1 semantics.

After excluding these, only 8–9 cold metrics actually fit the
"declare-init-record" 1:1 shape; the remainder still has to be hand-written.
The ~100-line saving in the finding is closer to ~50 lines in practice — at
the cost of one new macro layer that readers have to chase.

**4. Cost-of-change is concentrated in the existing helpers, not at call sites.**

The two helpers `get_or_register_counter` / `get_or_register_gauge`
already abstract the lazy-init mechanism. Adding a new cold metric is three
nearly-mechanical edits in one file — there is no cross-module coordination,
no public API to keep in sync, no runtime behavior to preserve across versions.
The "future-drift hazard" the refactor prevents has not appeared in this file's
history.

**5. Established precedent class.**

Same shape, same conclusion:

- `observability/protocol-logger-oncelock-pattern-not-extracted.md` — extract
  `OnceLock + init + get + log` across `tcp/tls/udp/ssl_log.rs`. Declined.
- `architecture/conf-handler-simple-map-pattern.md` —
  extract a generic wrapper across `LazyLock<RwLock<HashMap>>` + three-arm
  add/update/remove sites. Declined for the same "no stable-Rust
  generic-static keyed on a wrapper type" reason that applies here.
- `cpu-memory/cases/oncelock-global-singleton-deliberate-design.md` — the
  `OnceLock<Arc<...>>`-keyed-to-process-lifetime style is itself a deliberate
  pattern in this codebase, not an accident worth abstracting away.

## Fix Suggestions Not Accepted

- "Introduce `cold_counter!(field, NAME)` / `cold_gauge!(field, NAME)`
  declarative macros that emit struct field, `new()` initializer, and a
  `record_*` method together" — saves ~50 lines after excluding the composite
  methods, hides `OnceLock::get_or_init` and the generated public method names
  from grep, and re-introduces metric-name coupling at the macro level.
- "Move the per-metric arms into a single `cold_metrics! { ... }` table-style
  macro" — same trade-offs as above, additionally couples all 14 metrics into
  one indivisible declaration so any future divergence (custom `#[inline(never)]`,
  label injection, derived metric grouping) forces a macro-arm overhaul instead
  of a single-method change.
- "Generate the registration list at compile time via a build-script or
  `inventory`-style registry" — heavier still; loses static visibility, adds
  proc-macro/build-time cost, and the underlying problem (14 hand-typed lines)
  does not justify the added machinery.

## Re-evaluation Triggers

Re-open this decision only if:

- The number of cold metrics on `GatewayMetrics` roughly doubles (≥25) and
  the file becomes hard to read at a glance.
- A real silent-`None` or wrong-name incident is observed in production
  (not hypothesized in review) and is directly traceable to the three-point
  wiring pattern.
- The struct switches to `..Default::default()` style or similar, removing
  the compile-time field-presence guarantee — at that point the "silent
  `None`" risk becomes real and a macro that emits all three sites together
  starts to earn its keep.
- A new mandatory cross-cutting step is added to every cold-metric path
  (e.g., registering each handle with an external supervisor, attaching a
  shared label set) — at that point the shared step is the real abstraction,
  not the OnceLock cell.

## Reference Cases

- Closed finding: `tasks/observe/observe-F011-registry-extract-fn-registration-boilerplate.md`
  (closed 2026-05-28, ⚠️ accepted tradeoff).
- Source kept as-is:
  - `edgion-gateway/src/observe/metrics/registry.rs` (`GatewayMetrics` struct + `new()` + `record_*` methods)
- Sibling decision records:
  - `skills/04-review/observability/protocol-logger-oncelock-pattern-not-extracted.md`
  - `skills/04-review/architecture/style-consolidation-thresholds.md`
  - `skills/04-review/architecture/conf-handler-simple-map-pattern.md`
