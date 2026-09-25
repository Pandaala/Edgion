---
name: unknown-plugin-variant-strict-enum
description: Unknown HTTP, Stream, and TLSRoute plugin types decode to a non-executable sentinel and fail closed; known configs and exhaustive dispatch remain typed.
status: accepted-tradeoff
finding_id: resource-01
closed: 2026-06-14
knowledge_type: intentional-design
---

# Unknown plugin types are readable but never executable

## Current decision

The user requested that new plugin names need no CRD change. HTTP, connection-stage Stream,
and TLSRoute plugin `type` fields are now open strings. Their existing Rust enums retain typed
known configs and add a schema-hidden unit `Unsupported` variant. A shared variant-local
serde helper discards unknown config content; it does not convert invalid known configs into
the sentinel. Missing or non-string discriminators remain decode errors.

Controller preprocessing reports indexed errors and `Accepted=False`, even when an unknown
entry is disabled. HTTP compilation records non-retainable invalidity and seals the existing
failure runtime; diagnostics alone are not sufficient. Stream source generation denies every
demanded stage when either list contains an unsupported type. Direct Stream stage constructors
also reject sentinels before enable or policy checks. Unknown auth plugins must never silently
become successful empty chains.

Explicit sentinel match arms preserve compiler exhaustiveness for every newly added real
variant. No wildcard dispatch or runtime registry is introduced. `#[schemars(skip)]` keeps the
sentinel out of supported-name and policy catalogs.

## Tradeoffs and boundaries

The original unknown name and opaque config are discarded; typed views serialize
`type: Unsupported`. Diagnostics locate the stage and entry rather than repeating raw input.
Kubernetes retains the original submitted spec. Known malformed configs can still fail typed
listing, and old binaries without sentinel support are not made forward compatible.

The former strict-enum decision (resource-01, 2026-06-14) prohibited a fallback that silently
skipped unknown entries. That safety rationale remains. Its claim that unknown discriminators
reached Controller validation and produced status was incorrect: the typed Kubernetes watcher
could fail while decoding a list page, before the handler. Its claim that any explicit fallback
necessarily removes match exhaustiveness was also too broad. This implementation changes the
failure representation while retaining explicit rejection.

## Evidence and regression boundaries

- Resource enums: `edgion_plugin.rs`, `stream_plugins.rs`, `tls_route_plugins.rs`.
- Shared decode helper: `edgion-resources/src/resources/common/mod.rs`.
- HTTP source scan: `EdgionPlugins::unsupported_plugin_errors`; runtime gate in
  `edgion-gateway/src/plugins/http/plugin_store.rs`.
- Stream scan: `EdgionStreamPlugins::unsupported_plugin_errors`; source gate in
  `edgion-gateway/src/plugins/stream/plugin_generation.rs`.
- Guard tests: plugin and stream CRD type coverage, Controller handler/status tests,
  and HTTP/Stream generation tests for disabled and policy-masked unknown entries.

Re-evaluate if exact-name preservation, dynamically loaded plugins, or mixed-version wire
compatibility becomes a requirement. Do not weaken unknown-type rejection to implement them.
