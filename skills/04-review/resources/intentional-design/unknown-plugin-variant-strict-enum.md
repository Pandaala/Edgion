---
name: unknown-plugin-variant-strict-enum
description: Use when reviewing findings that compare Edgion against Envoy/xDS proto3 and claim "EdgionPlugin enum has no Unknown/Unsupported catch-all variant / an unrecognised plugin type rejects the whole EdgionPlugins resource at the Gateway / serde from_value/from_str Err drops the resource (or the entire list batch) / rolling upgrade Controller-before-Gateway is unsafe / add a forward-compat Unknown { type_name, raw } variant". Covers why the strict typed enum is intentional and why the catch-all fix is a wide cross-layer feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: resource-01
closed: 2026-06-14
knowledge_type: intentional-design
---

# Unknown EdgionPlugin variant rejects the EdgionPlugins resource — ACCEPTED-TRADEOFF (strict typed enum is intentional)

## Conclusion

`EdgionPlugin` is a strictly typed Rust `enum` with `#[serde(tag = "type", content = "config")]`
and no catch-all variant (`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs:27`).
A plugin `type:` that the compiled binary does not recognise fails deserialization: the
Gateway watch path drops the single offending resource (`event_dispatch.rs:323`, per-event),
and the Gateway list path drops the whole batch (`event_dispatch.rs:95`, atomic `Vec<T>`).
This is an accepted design tradeoff, **not** a defect and **not** a narrow review fix.

The proposed proto3-style fix — add an `Unknown { type_name: String, raw: serde_json::Value }`
catch-all that preserves and skips unrecognised entries — is a large cross-layer change
(custom `Deserialize` impl + new variant rippling through every exhaustive match across three
crates) with a real semantic downside (it would silently swallow operator typos). If ever
prioritized it must go through brainstorming as a `tasks/todo/` feature, not this workflow.

Fix suggestions of the form "add an `Unknown`/`Unsupported` catch-all variant to `EdgionPlugin`"
are **not accepted** as a narrow review fix.

## Core Rationale

**1. The strict enum is an intentional lockstep invariant, not an oversight**

The "no catch-all" property is load-bearing across crates. `EdgionPlugin::type_name()`,
`validation_error()`, `body_requirement()`, `dye_capabilities()`, `dye_type_prefix()`,
`key_header_names()`, and the Gateway's four `create_*_from_edgion` matches are all exhaustive:
a newly added variant fails to compile until each is updated. A catch-all `_`/`Unknown` arm
would dissolve exactly the compile-time guarantee these matches exist to provide.

> **Stage applicability is the one deliberate exception.** `applicable_stages` used to be an
> exhaustive `match` too, but it is now a lookup over `HTTP_PLUGIN_STAGE_EXCEPTIONS`
> (`edgion_plugins/validate.rs`) keyed by `type_name()`, with request-only as the default —
> so the CRD drift guard can read the authority instead of maintaining a copy of it. The lost
> compile-time check is replaced by `stage_exception_keys_are_real_variants` in
> `edgion-resources/tests/plugin_crd_type_coverage.rs`, which validates the keys against
> `schema_for!(EdgionPlugin)`. That is stronger than the match was: it checks the names the CRD
> and `plugin_policy` actually consume, not the enum declaration. This exception is scoped to
> stage applicability; it is **not** a precedent for softening the matches listed above.

**2. A string-tag catch-all masks operator typos that the type check currently rejects**

Envoy's proto3 forward compatibility keys off numeric field numbers on the wire — an unknown
field number is unambiguously "newer schema", never a typo. Edgion's tag is a human-authored
string (`type: BasicAuth`). With an `Unknown` catch-all, a misspelled `type: BasicAtuh` would
no longer be rejected at Controller ingest by `EdgionPlugins::validate_plugins`
(`edgion-resources/.../validate.rs:124`, run from the controller handler at
`edgion-controller/.../handlers/edgion_plugins.rs:86`) — it would silently deserialize as
`Unknown` and the plugin would simply never run. The current hard failure is the defense that
surfaces config typos as a K8s status condition. Trading that away is a net regression in the
common case to buy resilience in a rare one.

**3. serde cannot express the suggested variant cheaply**

`#[serde(other)]` only applies to unit variants and cannot capture the `content`. Implementing
`Unknown { type_name, raw }` requires a hand-written `Deserialize` (or an `untagged` fallback)
for a ~35-variant enum — error-prone, and it changes the deserialization semantics of a core
shared schema type that `edgion-resources`, `edgion-gateway`, and `edgion-controller` all
depend on. This is medium-risk cross-module work, the opposite of a narrow review fix.

**4. The exposure is rare and bounded by deployment hygiene**

The failure only triggers in the version-skew window of a Controller-before-Gateway rolling
upgrade *and* only when that upgrade introduces a brand-new plugin type referenced by a live
resource. Same-version clusters never hit it (the Controller's `validate_plugins` rejects
unknown types at ingest, so they never reach a Gateway). Standard rollout practice — upgrade
the data plane first, or gate new-plugin-type adoption until the fleet is upgraded — avoids it
entirely. This mirrors the Envoy/Istio posture catalogued in the sibling
`gateway-no-local-config-snapshot.md`: a real-but-narrow rolling-upgrade gap, not a baseline
defect.

## Fix Suggestions Not Accepted

- "Add an `Unknown { type_name, raw }` (or `Unsupported`) catch-all variant to `EdgionPlugin`"
  — wide cross-layer change (custom `Deserialize` + ripple through every exhaustive match in
  three crates) that also masks operator typos. Promote to `tasks/todo/` via brainstorming if
  ever prioritized.
- "Make the Gateway list path (`event_dispatch.rs:95`) parse each resource independently and
  skip the bad one instead of failing the whole `Vec<T>` batch" — a narrower variant, but it
  still changes failure semantics (a genuinely corrupt resource would be silently dropped
  rather than loudly failing the relist) and only matters once the underlying catch-all
  question is decided. Not accepted as a standalone review fix; it belongs with the same
  forward-compat feature.
- "Emit a Controller `Condition` reporting partial degradation for unknown types" — depends on
  the `Unknown` variant existing first; it is part of the same feature, not independently
  shippable.

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows a Controller-before-Gateway rolling upgrade losing live plugin
  config because of an unknown plugin type, in a topology that readiness probes + staged
  rollout could not cover.
- Edgion adopts a versioned/numeric on-wire plugin discriminator (so an "unknown" entry can be
  distinguished from a typo without guessing), making the catch-all safe.
- The plugin set begins shipping out-of-tree / dynamically, so the compiled binary can no
  longer be assumed to know every valid `type:`.

## Reference Cases

- `tasks/review-envoy/resource-01-unknown-plugin-type-rejects-whole-resource.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs:27` (the `#[serde(tag = "type", content = "config")]` enum, no catch-all)
- `edgion-resources/src/resources/edgion_plugins/validate.rs:30-32` (the documented exhaustive-match lockstep invariant)
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:95` (atomic list-path `Vec<T>` parse) and `:323` (per-event watch parse)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:86` (`validate_plugins` at ingest)
- `skills/04-review/conf-sync/intentional-design/gateway-no-local-config-snapshot.md` (sibling Envoy-comparison rolling-upgrade tradeoff)
