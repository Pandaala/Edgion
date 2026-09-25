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
The same method, replay-backing, attempt-budget, and deadline gates also veto
already-decided `InvalidH2` and `H2Downgrade` errors. These local protocol
outcomes do not establish that the origin skipped the request. The AI attempt
dispatcher explicitly clears a preexisting retry decision when it returns
`Final` or fails.

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
  i.e. the operator opted into retrying. Case 2 and Pingora's ambiguous
  already-decided H2 protocol outcomes can retry without per-route policy;
  both now apply the method-safety guard. This does not change the explicit
  status or per-try-timeout policy.

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
`decide_reuse` is a no-op for an incoming `Decided(true)`. Pingora 0.9 can
produce that state for local H2 `GOAWAY(PROTOCOL_ERROR)` as `InvalidH2`, even
when an origin may already have processed a POST. The override must explicitly
clear the decision when the method, backing, budget, or deadline gate fails.
Protocol-qualified `REFUSED_STREAM` and peer GOAWAY decisions remain subject
to the separate Pingora-decided ceiling. Their safety depends on the precise
stream condition, not on every error called `H2Error` being safe.

P090-008 remains the acceptance task for controlled-origin side-effect counts,
the complete retry policy table, and remaining H2 cases. This page records the
corrected guard, not completion of that broader matrix.

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
- A Pingora change to H2 error classification or retry decisions — recheck
  which outcomes prove the stream was not processed.

## Reference Cases

- Original task: `tasks/todo/route-01-retriable-methods-non-idempotent-guard.md` (promoted from
  review finding `route-retry-non-idempotent-methods`).
- Sibling per-try-timeout decisions: `routing/per-try-timeout-via-backend-request.md`,
  `routing/per-try-timeout-test-coverage.md`.
- Annotation reference: `skills/02-features/05-annotations/references/annotations-metadata.md`
  (`edgion.io/retry-non-idempotent`).
- Pingora 0.9 acceptance task: `tasks/pingora-0.9.0/008-overridden-retry-policy-review.md`.
