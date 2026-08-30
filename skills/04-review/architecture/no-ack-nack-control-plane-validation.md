---
name: no-ack-nack-control-plane-validation
description: Use when reviewing findings that compare Edgion against Envoy/Istio and claim "no ACK/NACK feedback loop / WatchResponse is one-way / Controller cannot tell if a Gateway applied vs. rejected a config version / no version_info + error_detail like xDS DiscoveryRequest / Gateway silently skips a bad event on parse error / no last-good-version pinning / no per-node applied-version tracking". Covers why Edgion replaces the data-plane ACK/NACK model with control-plane ingest validation + K8s .status conditions, and why a feedback RPC is a large cross-layer feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: sync-01
closed: 2026-06-14
---

# No ACK/NACK feedback loop on the Watch stream — ACCEPTED-TRADEOFF (control-plane ingest validation replaces data-plane ACK/NACK)

## Conclusion

Edgion's config-sync is one-way: `WatchResponse` carries server→client fields
(`data`, `sync_version`, `err`, `server_id`) and the Gateway never sends a
structured "applied / rejected version X because Y" message back to the
Controller. This is an **accepted design tradeoff**, not a defect, because
Edgion validates configuration at the **control plane on resource ingest** and
surfaces the result as a K8s `.status` condition — which is precisely the
operator-visible "what is actually running / what was rejected and why" signal
that Envoy's xDS ACK/NACK exists to provide. Edgion places that signal at the
source of truth (the K8s resource) instead of reconstructing it from data-plane
acknowledgments.

Fix suggestions of the form "extend the Watch RPC with a client feedback message
/ add a separate `Ack` RPC carrying `version_info` + `error_detail` / track
per-node per-kind applied versions on the Controller / NACK and pin the Gateway
to the last-good version" are **not accepted** as a narrow review fix.

## Core Rationale

**1. The operator-visible signal already exists, at the right layer**

The finding's worked example is: an operator pushes a misconfigured resource →
the Controller accepts it as valid K8s → pushes it → the Gateway fails to
deserialize/parse and silently skips → no operator-visible signal. That premise
does not hold in Edgion: the resource processor validates on ingest before the
config is ever pushed. For EdgionPlugins, `EdgionPluginsHandler::preparse`
(`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:76-87`)
calls `EdgionPlugins::validate_plugins`
(`edgion-resources/src/resources/edgion_plugins/validate.rs:124`), which mirrors
the gateway `compile_plugins` preparse errors (condition regex compilation, and
per-entry config/alias/stage validation for every enabled entry). Those errors
are written into the EdgionPlugins K8s `.status` condition. The finding's own
"Suggested direction" asks to "reflect it back as a `.status` condition" — Edgion
already does this, on ingest, without an ACK/NACK round trip.

**2. The residual gateway-only gap is narrow and fails closed**

DSL compile / static validation (parser, validator, compiler — the custom
nom-parser/bytecode-VM engine, not Rhai) now runs controller-side too, via the
shared `edgion-dsl` crate that both `EdgionPlugins::validate_plugins` and the
gateway `compile_plugins` call through `DslConfig::compile_and_validate`. The
one remaining gap versus the gateway `compile_plugins` path is DSL VM init
(`Vm::new`, e.g. per-pattern regex DFA precompilation), which requires
constructing the gateway-only DSL VM and so cannot run in the controller (the
controller has no `edgion-gateway` dependency — see CLAUDE.md crate DAG). For
that narrow class the data plane **fails closed at runtime** rather than
serving a half-applied resource. So the set of failures that reach a Gateway
un-pre-validated AND silently degrade traffic is smaller still; closing it with
a full ACK/NACK protocol is disproportionate.

**3. A feedback RPC is a large cross-layer feature, not a narrow fix**

Implementing ACK/NACK touches every layer: a proto change to
`edgion-common/src/conf_sync/proto/config_sync.proto` (new client→server message
or a separate `Ack` RPC with `version_info` + `error_detail`), a Gateway-side
applied/rejected reporter in the watch loop
(`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`), Controller-side
per-node per-kind applied-version bookkeeping, and a write-back path to the
resource `.status` and/or admin API. That is medium-risk cross-layer work
(protocol/state-machine change, cross-node consistency) and belongs in
`tasks/todo/` via brainstorming if ever prioritized — not this workflow.

## Fix Suggestions Not Accepted

- "Extend the Watch RPC with a client-side feedback message / add a separate
  `Ack` RPC with `version_info` + `error_detail`" — large cross-layer protocol
  feature; the operator-visible signal it would carry already exists as a K8s
  `.status` condition produced on ingest. Promote to `tasks/todo/` if ever
  prioritized.
- "Track per-node per-kind applied versions on the Controller and expose them via
  the admin API" — only meaningful once the feedback channel exists; part of the
  same feature, not an independently shippable fix.
- "NACK and pin the Gateway to the last-good version instead of skipping the bad
  event" — last-good retention is a behavior change to the watch apply path;
  control-plane validation already prevents most bad versions from being pushed,
  and the residual DSL-deep-failure class fails closed at runtime.
- "Treat the absence of ACK/NACK as a regression vs Envoy" — rejected on the
  facts: Edgion moves validation upstream to ingest + `.status`, achieving the
  operator-visibility goal of ACK/NACK without the data-plane round trip.

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows a misconfigured resource passing control-plane
  validation, being silently skipped on the Gateway, and degrading traffic with
  no `.status` / log signal — i.e. the gateway-only gap (Rationale 2) proves
  operationally material rather than rare.
- A non-K8s config source is added where there is no `.status` condition to carry
  the validation result, removing the control-plane visibility this tradeoff
  relies on.
- Per-node applied-version observability becomes a hard operator requirement
  (e.g. a fleet-wide rollout-status feature) that ingest validation cannot serve.

## Reference Cases

- `tasks/review-envoy/sync-01-no-ack-nack.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:76-87` (`preparse` → `validate_plugins`, errors → K8s `.status`)
- `edgion-resources/src/resources/edgion_plugins/validate.rs:124` (`validate_plugins`, mirrors gateway `compile_plugins` preparse errors minus DSL VM init)
- `edgion-common/src/conf_sync/proto/config_sync.proto:36-51` (`WatchRequest` / `WatchResponse` — one-way schema)
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:270-388` (watch apply loop; parse-error skip at line 306)
- `skills/04-review/architecture/gateway-no-local-config-snapshot.md` (sibling Envoy-comparison config-sync tradeoff, sync-05)
- `tasks/review-envoy/sync-02-no-cross-kind-ordering.md` (sibling config-sync Envoy-comparison finding)
