# Fail Closed on Denied L7 Named Jumps

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make DynamicInternalUpstream reject denied backend references before publishing an internal jump, and make HTTP named-jump resolution fall back to weighted selection only for a genuine missing target. A `RefDenied` or any other non-miss error must remain typed and fail closed.

## Requirements

- Replace DynamicInternalUpstream's raw Service kind/name/namespace scan with the canonical `RouteRules::find_backend_by_name` lookup against compiled backends.
- Route every DynamicInternalUpstream lookup error through its existing `onInvalid` policy before setting an internal upstream jump or recording Success.
- In HTTP upstream selection, fall through from a named jump to weighted selection only for the exact `BackendNotFound` outcome.
- Propagate `RefDenied` and every other non-miss error through the existing typed gateway/access-log error path; never convert them into a legal sibling selection.
- Preserve ordinary missing-target weighted fallback, optional namespace matching, explicit weight-zero named targets, jump-lock behavior, and all current `onInvalid` response/log/Dye semantics.
- Build test fixtures from one schema HTTPRouteRule into its compiled backend view so `ref_denied`, namespace, and weight semantics match production.
- Do not add a second backend matcher or duplicate Controller/ReferenceGrant validation.

## Scope

### In scope

- `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs`.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` named-jump error handling.
- Colocated plugin and upstream-selector regression tests.

### Out of scope

- Canary, which is corrected in B0066.
- L4 TCP, UDP, or TLS backend selection.
- Resource schema, Controller reference resolution, or ReferenceGrant policy changes.
- Existing integration-test modifications.

## Acceptance criteria

- A denied DynamicInternalUpstream target follows configured Reject or Fallback before any jump or Success outcome is published.
- A denied named jump with a legal weighted sibling never selects that sibling and surfaces the typed denied-reference error.
- A genuinely missing named jump retains the documented weighted fallback.
- Optional namespace matching and explicit named selection of a weight-zero backend remain supported.
- PluginLog, Dye outcomes, client error bodies, and access-log error codes retain their existing ownership and redaction rules.
- Focused DynamicInternalUpstream, HTTP upstream-peer, PluginRuntime, workspace, Clippy, and repository guards pass.

## Related work

- [`Fail closed on denied L4 backend references`](l4-backend-ref-denied-fail-closed.md)

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Keep this task separate from B0066 and the L4 task. | It spans another plugin and the shared HTTP named-jump selector, while the L4 task owns different protocols and selection code. |
