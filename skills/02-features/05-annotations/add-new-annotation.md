---
name: add-new-annotation
description: Use when adding a new edgion.io/* annotation; checklist of: classify (scope, parse phase), files to touch (controller preparse, gateway runtime, schema reference), validation hooks, and common failure modes.
---

# Add New `edgion.io/*` Annotation

Use this checklist when introducing a new `edgion.io/*` annotation key. Read
[00-annotations-overview.md](00-annotations-overview.md) and
[references/annotations-metadata.md](references/annotations-metadata.md) first
to understand the existing key taxonomy before adding a new one.

## Step 1 — Classify the Annotation

Answer these four questions before touching any file:

| Question | Options |
|----------|---------|
| **Scope** | `metadata.annotations` (resource-level, parsed from K8s object) vs `spec.options` (listener/policy options block) vs Labels |
| **Parse phase** | Controller-side preparse (during resource processing, before sync) vs Gateway-side runtime (read after the resource arrives at Gateway) |
| **Namespace** | Normal user-facing key, system-managed (do not set manually), or test-only (e.g., `metrics-test-*`) |
| **Cardinality** | Gateway-level, route-level, per-rule, or service-level |

Most keys fall into one of two paths:

- **Controller preparse path**: the Controller reads the annotation, derives runtime
  fields, and stores them in the resource's preparse struct before syncing.
  Example: `edgion.io/forward-raw-path` is parsed in `http_route_preparse.rs`
  and stored in `HTTPRouteRule.parsed_forward_raw_path`.
- **Gateway runtime path**: the Gateway reads the annotation directly from the
  synced resource struct at runtime.
  Example: `edgion.io/enable-http2` is read in `listener_builder.rs` and
  `listener_gateway_info.rs`.

## Step 2 — Add the Constant

Always register the key string as a named constant first:

1. Open `edgion-resources/src/constants/annotations.rs`.
2. Add a `pub const YOUR_KEY: &str = "edgion.io/your-key";` entry under the
   appropriate submodule (`edgion` module for standard keys; `test_only` for
   test/diagnostics keys).
3. Use the constant everywhere; never inline the string literal at a call site.

## Step 3 — Checklist of Files to Touch

### 3a. Controller preparse path (`metadata.annotations`)

| Step | File | What to do |
|------|------|------------|
| 1 | `edgion-resources/src/resources/<resource>_preparse.rs` or the resource's main `rs` file | Add a parsed field (e.g., `pub parsed_your_key: Option<bool>`) to the preparse struct |
| 2 | `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/<resource>.rs` | Read the annotation in `on_change` / parse phase; populate the parsed field |
| 3 | `edgion-resources/src/constants/annotations.rs` | Constant (Step 2 above) |
| 4 | `skills/02-features/05-annotations/references/annotations-metadata.md` | Add a row to the relevant table and describe behavior, default, and effect |
| 5 | `skills/02-features/05-annotations/00-annotations-overview.md` | Add a row to the quick-lookup table for the resource type |

> TODO: confirm the exact preparse file path for non-HTTP route types (e.g., GRPCRoute uses a separate handler; TCPRoute / TLSRoute handler is in `routes/tcp/` and `routes/tls/`).

### 3b. Gateway runtime path (`metadata.annotations`)

| Step | File | What to do |
|------|------|------------|
| 1 | `edgion-resources/src/constants/annotations.rs` | Constant (Step 2 above) |
| 2 | Gateway runtime consumer — varies by resource type (see table below) | Read the annotation at the correct point in the processing pipeline |
| 3 | `skills/02-features/05-annotations/references/annotations-metadata.md` | Document the key |
| 4 | `skills/02-features/05-annotations/00-annotations-overview.md` | Quick-lookup row |

Gateway runtime consumer locations by resource:

| Resource | Runtime consumer file |
|----------|-----------------------|
| Gateway listeners | `src/core/gateway/runtime/server/listener_builder.rs`, `src/core/gateway/runtime/store/listener_gateway_info.rs` |
| TCPRoute / TLSRoute | `src/core/gateway/routes/tcp/conf_handler_impl.rs`, `src/core/gateway/routes/tls/conf_handler_impl.rs` |
| Service / health | `src/core/gateway/backends/health/check/annotation.rs` |
| EdgionTls | `edgion-resources/src/resources/edgion_tls.rs` |

### 3c. `spec.options` path (listener options / policy options)

| Step | File | What to do |
|------|------|------------|
| 1 | `edgion-resources/src/constants/annotations.rs` | Constant |
| 2 | The resource type file that parses `spec.options` | Add the options key read |
| 3 | `skills/02-features/05-annotations/references/annotations-options-and-labels.md` | Document the key |

## Step 4 — Validation and Defaults

- **CRD-side**: if the annotation value has a restricted value set (e.g., enum
  strings), add a `x-kubernetes-validations` CEL rule in the CRD YAML under
  `config/crd/edgion-crd/`. For Gateway API resources, add a validating webhook
  or rely on annotation parsing to reject bad values.
- **Missing / absent value**: return the documented default (do not panic).
  Pattern: `annotations.get(edgion::YOUR_KEY).map(|v| v == "true").unwrap_or(false)`.
- **Malformed value**: log at INFO level with resource ns/name and the raw value,
  then fall back to the default. Do NOT reject the resource for a malformed
  annotation unless the error is unrecoverable.
  Pattern followed by `edgion.io/forward-raw-path` (see
  `references/annotations-metadata.md` §forward-raw-path): emit a tracing INFO
  event with ns / name / value and treat as absent.

## Step 5 — Test Coverage

1. Add a unit test in the handler or preparse file covering the annotation's
   parse logic (present/absent/malformed).
2. Add an integration scenario in `examples/test/` that exercises the annotation
   end-to-end. Consult
   [05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)
   for the scenario structure.
3. If the annotation controls a hot-reload-observable behavior, add a dynamic
   (hot reload) scenario — see the integration testing guide's dynamic section.

## Step 6 — Common Failure Modes

| Failure mode | How to avoid |
|--------------|-------------|
| **Parse silently swallows the value** | Always log the malformed-value case at INFO before treating as absent; add a unit test for the malformed path |
| **Runtime consumer reads the annotation before preparse populates it** | If Controller preparse is the canonical source, store the result in the parsed field and read that field on Gateway — never re-parse the raw annotation string on the Gateway side |
| **Key collision with system / test-only namespace** | Check `references/annotations-system-and-test-keys.md` before choosing the key name; avoid names that could be confused with system-managed keys |
| **Legacy key forward-port** | If an older doc or example uses a legacy key (e.g., `edgion.io/stream-plugins`), update the example to the current key; never copy legacy keys into new code |
| **Constant string diverges from the actual key** | Derive the constant from the literal once in `annotations.rs`; all consumers use the constant, never an inline string |
| **CRD YAML and Rust type disagree** | After updating the Rust constant, grep the CRD YAML for the old string; update it if it appears as an example or default value |

## Linked References

- [00-annotations-overview.md](00-annotations-overview.md) — quick-lookup table for all current keys
- [references/annotations-metadata.md](references/annotations-metadata.md) — detailed per-key reference; lists exact source files verified per key
- [references/annotations-options-and-labels.md](references/annotations-options-and-labels.md) — `spec.options` and Labels reference
- [references/annotations-system-and-test-keys.md](references/annotations-system-and-test-keys.md) — system-managed and test-only keys; check before naming a new key
- `edgion-resources/src/constants/annotations.rs` — single source of truth for key string constants
- [../../01-architecture/01-controller/09-add-new-resource/00-guide.md](../../01-architecture/01-controller/09-add-new-resource/00-guide.md) — resource-level wiring guide (required companion when the annotation is part of a new resource)
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md) — integration test structure and dynamic-scenario guide
