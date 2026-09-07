---
name: no-ack-nack-control-plane-validation
description: Use when reviewing findings that compare Edgion against Envoy/Istio and claim "no ACK/NACK feedback loop / WatchResponse is one-way / Controller cannot tell if a Gateway applied vs. rejected a config version / no version_info + error_detail like xDS DiscoveryRequest / Gateway silently skips a bad event on parse error / no last-good-version pinning / no per-node applied-version tracking". Covers why Edgion replaces the data-plane ACK/NACK model with control-plane ingest validation + K8s .status conditions, and why a feedback RPC is a large cross-layer feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: sync-01
closed: 2026-06-14
knowledge_type: intentional-design
---

# No ACK/NACK feedback loop on the Watch stream — ACCEPTED-TRADEOFF (control-plane ingest validation replaces data-plane ACK/NACK)

## Conclusion

**Applicability limit (2026-09-07):** the accepted absence of a feedback RPC
does not establish that generic Watch decoding cannot fail, or that skipping a
failed update is correct. Ingest status is not a per-Gateway applied-version
acknowledgment. Generic decode failures now end the stream and recover via List,
retaining successful prefix events and existing readiness; see
[generic Watch decode recovery](../regression-guards/watch-decode-error-relist.md).
Treat the historical validation rationale below as applying to business-config
validation, not proof of wire-format round trips or permission to close a
decode-recovery defect as a false positive.

Edgion's config-sync is one-way: `WatchResponse` carries server→client fields
(`data`, `sync_version`, `err`, `server_id`) and the Gateway never sends a
structured "applied / rejected version X because Y" message back to the
Controller. This is an **accepted design tradeoff**, not a defect, because
Edgion validates configuration at the **control plane on resource ingest** and
surfaces the ingest validation outcome as a K8s `.status` condition. This tells
operators what the Controller accepted or rejected and why; it does not prove
what each Gateway has applied. Edgion accepts that visibility limit rather than
adding a per-Gateway acknowledgment protocol.

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
- "Add NACK and per-Gateway last-good-version tracking" — a protocol expansion
  outside the accepted scope. Generic decode-error recovery now retains valid
  events and relists without such a protocol; do not cite this decision against
  that local recovery behavior.
- "Treat the absence of ACK/NACK as a regression vs Envoy" — rejected on the
  accepted scope: Edgion provides ingest validation through `.status`, with the
  explicit limit that it is not per-Gateway applied-version feedback.

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
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` (`apply_generic_watch_payload` returns on first decode failure; `start_watch` ends the stream and relists)
- `skills/04-review/conf-sync/intentional-design/gateway-no-local-config-snapshot.md` (sibling Envoy-comparison config-sync tradeoff, sync-05)
- `tasks/review-envoy/sync-02-no-cross-kind-ordering.md` (sibling config-sync Envoy-comparison finding)
