---
name: retry-non-idempotent-method-guard
description: Use when reviewing findings that connection-error retries replay non-idempotent (POST/PATCH/...) request bodies on a reused connection without consulting the HTTP method; covers why the guard lives in Case 2 + an annotation, not the standard CRD.
status: fixed
finding_id: route-01
closed: 2026-06-16
knowledge_type: regression-guards
---

# Reused-connection retry replays non-idempotent bodies — FIXED (method-safety guard)

## Conclusion

The reused-connection retry path (`error_while_proxy` Case 2,
`edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`) now consults the
HTTP method before replaying a buffered request body. Non-idempotent methods
(POST/PATCH/CONNECT, and all gRPC since it is POST at the HTTP layer) are **not**
retried on a reused keepalive reset by default; idempotent methods
(GET/HEAD/PUT/DELETE/OPTIONS/TRACE per `http::Method::is_idempotent()` / RFC 9110)
still are. A route opts back in with `edgion.io/retry-non-idempotent: "true"`.

Re-report fix suggestions of the following forms are **not accepted**:
- "Add a `retriable_methods` / `allowNonSafeRetry` field to the `HTTPRouteRetry` CRD" —
  `HTTPRoute` is the **standard** Gateway API CRD (`gateway.networking.k8s.io`, vendored
  under `config/crd/gateway-api/`). It is a structural schema, so the API server prunes
  any field not declared upstream → a new spec field is a silent no-op. The knob MUST be
  an annotation (the Gateway-API-blessed vendor-extension mechanism), not a CRD field.
- "Also guard `fail_to_connect`" — that path never wrote request bytes upstream (connection
  never established), so a retry cannot double-execute. Guarding it would needlessly suppress
  safe retries.
- "Also guard Case 1 (status-code `retry.codes`) and Case 1.5 (per-try timeout)" — those
  fire only under an **explicit** operator-configured retry policy (`retry_configured()`),
  i.e. the operator opted into retrying. Case 2 is the only path that retries on the global
  `retry.attempts` default (2) with no per-route config, so it is the only *implicit* exposure.
  Scoping the guard to Case 2 is the accepted decision.

## Core Rationale

Pingora buffers the request body (≤ 64 KB `BODY_BUF_LIMIT`) and replays it to a fresh
connection when a reused keepalive connection is reset mid-request. For a non-idempotent
request that is a genuine silent double-execution risk. The exposure is bounded
(≤ 64 KB body + reused-connection reset + non-idempotent method), but that is exactly the
payload profile of typical mutation APIs (orders / payments / form posts). nginx is
conservative here by default (`proxy_next_upstream` excludes `non_idempotent`); the fix
brings Edgion's default in line.

The guard is a single conjunct on `can_retry`:
`method_retry_ok = session.req_header().method.is_idempotent() || ctx.retry_allow_non_idempotent()`.
Errors arriving already-`Decided(true)` (h2 GOAWAY / REFUSED_STREAM decided inside Pingora)
are unaffected — `decide_reuse` is a no-op unless the state is `ReusedOnly`.

The opt-in flag is a runtime-only field (`parsed_allow_non_idempotent_retry`,
`#[serde(skip)] #[schemars(skip)]`) on `HTTPRouteRule` / `GRPCRouteRule`, parsed from the
route-level annotation in each `preparse()` and stamped on every rule — the same mechanism
as `edgion.io/forward-raw-path`. preparse re-runs gateway-side, so only the annotation
travels over conf_sync, never the derived field.

## Re-evaluation Triggers

- Pingora exposes a per-attempt hook that distinguishes "body actually replayed" from
  "retry decided" — could tighten the guard to only fire when a body was buffered.
- A future requirement to make the guard per-rule (not per-route) — the annotation is
  currently route-level, applied to all rules.

## Reference Cases

- Original task: `tasks/todo/route-01-retriable-methods-non-idempotent-guard.md` (promoted from
  review finding `route-retry-non-idempotent-methods`).
- Sibling per-try-timeout decisions: `routing/per-try-timeout-via-backend-request.md`,
  `routing/per-try-timeout-test-coverage.md`.
- Annotation reference: `skills/02-features/05-annotations/references/annotations-metadata.md`
  (`edgion.io/retry-non-idempotent`).
