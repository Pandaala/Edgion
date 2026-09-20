# B0065 — Preflight Policy Hot Reload

## Scope

- Baseline: `a342833c58fae0764f13d753a856bc089764fca5`.
- Pipeline state: `blocked-verification`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`.
- Approved runtime support files: `config/edgion_gateway/conf_handler_impl.rs`, `ctx.rs`, `routes/http/proxy_http/{mod.rs,pg_new_ctx.rs,pg_request_filter.rs,pg_early_request_filter.rs,pg_response_filter.rs,pg_upstream_peer.rs,pg_upstream_request_filter.rs}`, and `runtime/server/listener_builder.rs` under `edgion-gateway/src/`.
- Approved knowledge files: `skills/01-architecture/02-gateway/02-pingora-lifecycle.md`, `skills/04-review/cors/SKILL.md`, and new `skills/04-review/cors/regression-guards/preflight-policy-egc-hot-reload.md`.
- Existing `edgion-tests/**`, schema, Controller, user documentation, and protected user paths remain forbidden.
- Exact commit allow-list: the listed runtime/knowledge files plus B0063-B0065 campaign records and D0011 records/task.

## Context and value gate

- Analysis verdict: `change`; implementation is plan-first because the correction crosses configuration publication, request-generation pinning, listener state, CORS ownership, and final-response behavior.
- `PreflightHandler` currently clones `preflightPolicy` when a listener is built, while EGC hot updates only publish a new immutable snapshot and do not rebuild listeners.
- Resolve compact preflight mode/status in `ResolvedHttpRequestConfig`, capture it with the existing single request snapshot read, and let the handler use that request-pinned policy for both classification and fallback status.

## Required invariants

- Native Gateway API CORS and EdgionPlugins CORS retain sole ownership of their configured requests.
- `AllOptions` does not locally terminate a request that has CORS configuration but lacks `Origin`; only the no-CORS global fallback uses the configured status.
- One request uses one policy generation for classification and fallback status.
- Invalid final status retains the existing unclaimed 500 path.
- Trace reconciliation, final-response claims, canonical access-log ownership, and request hot-path lock/allocation restraints remain unchanged.

## Verification plan

- EGC snapshot initial/update/delete/default and old-request pinning tests.
- Handler mode/delegation/invalid-status tests plus request-context wiring tests.
- Full campaign repository gates and frozen-path verification.

## Deferred coverage

- D0011 tracks the frozen cross-process hot-reload integration scenario.

## Implementation

- Published a compact resolved preflight mode/status in the existing immutable HTTP request configuration snapshot.
- Captured that policy in `new_ctx` through the existing single snapshot lookup and pinned it in the request context.
- Made `PreflightHandler` stateless and used the same request-pinned policy for classification and fallback status; listener construction no longer retains stale EGC policy.
- Preserved native and EdgionPlugins CORS ownership, `AllOptions` pass-through, invalid-status failure ordering, final-response claims, and access-log ownership.

## Independent reviews

- The independent plan review approved the resolved types, defaults, raw invalid status, single-generation capture, stateless handler, field rename, and exact allow-list.
- A fresh final code reviewer returned LGTM with no Must Fix or Should Fix; D0011 remains the only deferred finding.

## Verification

- All 8 focused snapshot, request-context, and preflight-handler tests passed.
- Gateway targeted check/clippy, formatting, diff, no-tracing, English-only, and frozen-path checks passed; clippy retained only the known unrelated Kafka warning.
- The user-authorized prerequisite commit `c8e48ff50` repaired the pre-existing task link; agent-doc validation now passes.
- Full workspace check and clippy passed. Formatting, diff, agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, no-tracing, cargo-deny, English-only, and frozen-path gates passed; clippy retained only the known unrelated Kafka and frozen-test warnings.
- The required Gateway API transform-contract gate cannot run because external commit `a768fbed0` declared `cicd/checks/generate_gateway_api_transform_contract.py` and its test without adding either file. Independent diagnosis found no exact substitute or historical version, so the campaign remains blocked outside B0065.

## Handoff

- Implementation/review complete: yes.
- Staged or committed: no.
- Resume action: separately restore the declared Gateway API transform-contract generator and test, run that gate, then revalidate the complete repository gate set and commit B0065.
