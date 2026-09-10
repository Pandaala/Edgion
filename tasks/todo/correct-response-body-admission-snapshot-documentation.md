# Correct Response-body Admission Snapshot Documentation

## Meta

- Type: docs
- Priority: P2
- Status: pending

## Problem and evidence

`skills/04-review/http-body/regression-guards/body-admission-reads-pre-plugin-snapshots.md`
incorrectly names `ctx.request_info.status` as the pre-plugin response snapshot
(table, rationale and re-evaluation trigger). It also presents a DSL `resp.body`
scenario before acknowledging that configuration now rejects it. This is verified
documentation drift, not a production regression or exhaustive body-admission audit.

Current authority and implementation:

- `skills/03-coding/observability/00-access-log.md:72`: request_info.status belongs to terminal logging; response safety uses a separate nonserialized snapshot.
- `AGENTS.md`, HTTP body invariant 2: upstream responses remain streaming and full-response DSL APIs must be rejected.
- `edgion-gateway/src/ctx.rs:2615`: pre_plugin_response_status is the normalized accepted status captured immediately before plugins, never serialized.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs:45` and the call near line 338 capture normalized status.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs:1606` combines live header and pre_plugin_response_status for bodyless agreement; nearby tests around lines 3980/4003 cover rewrites in both directions.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs:258` rejects compiled RespBody; its test near line 541 verifies configuration rejection.

## Scope and acceptance

- Update the owning regression-guard page and directly necessary links only.
- Correct snapshot ownership and re-evaluation triggers; verify nearby claims instead of mechanically replacing every status reference.
- Mark the old resp.body reproduction as historical and explain current validation rejection without proposing response buffering support.
- Preserve useful provenance and the live-plus-snapshot agreement invariant.
- No runtime, schema, logging, metrics or integration-test changes; run documentation validation and applicable guards.
