---
name: tls-route-plugin-access-log
description: Use when reviewing observability of TLSRoute plugin dispatch, missing resources, or wrong-stage attachments.
knowledge_type: mandatory-design
---

# TLSRoute plugin dispatch uses the canonical TLS access log

## Conclusion and scope

`routes/tls/mod.rs::run_tls_route_plugins` is on the connection hot path for
both Terminate and Passthrough. It must not emit `tracing` at any level.
Plugin allow/deny results and attachment failures use `TlsContext.log` or
`TlsContext.err_log`, consumed by the existing TLS access-log path.

The authority is [log safety, Iron Rule 3](../../../03-coding/01-log-safety.md).
An infrequent failure, runtime log filtering, or a per-resource once-only guard
does not exempt a connection-path event from this rule.

## Rationale and evidence

Both TLS proxies call the shared dispatch before connecting upstream. Stage-2
allow/deny events use `tls route plugin allowed` / `tls route plugin denied`.
A missing resource or a nonempty Stage-1-only attachment fails closed with a
diagnostic in `err_log`. An empty Stage-2 runtime adds no plugin event.
The stage contract and its regression evidence live in
[TLSRoute stage isolation](../../tls/regression-guards/tls-route-stage-isolation.md).

GeoIP is not a Stage-2 plugin. TLS listener GeoIP belongs to the Gateway's
Stage-1 connection filter; its denials remain metric-only. Removing the route
fallback removes its GeoIP reason-filtering helper and `StreamLayer::Tls`;
it does not authorize additional logging or new per-resource metrics.

## Historical evidence and limits

`plugins-stream-F011` (2026-05-27) rejected adding tracing to the former Stage-2
to Stage-1 fallback and documented its `(compat)` access-log events. That was
a logging decision, not acceptance of permanent compatibility behavior.
The fallback and those events were removed under
`remove-tlsroute-stage1-fallback-coherently`; historical instructions to retain
the suffix or grep for it are inactive. Current builds do not produce it.

This page does not establish a new secret/PII exception. Existing canonical
access-log redaction and diagnostic rules continue to apply.

## Verification and re-evaluation

Inspect both proxy callers and the shared dispatcher for new logging sinks.
Functional verification is recorded in the linked stage-isolation page.
Re-evaluate if dispatch ownership or the canonical logging contract changes;
removing an obsolete fallback does not require replacing its observability.

## Resolution references

- `plugins-stream-F011`: tracing proposal declined; historical documentation gap fixed.
- `remove-tlsroute-stage1-fallback-coherently`: fallback removed with coordinated configuration migration.
