---
name: per-try-timeout-via-backend-request
description: Use when reviewing routing/retry findings; covers why Edgion has no perTryTimeout CRD field and how per-try timeout is provided via timeouts.backendRequest.
status: fixed
finding_id: route-02
closed: 2026-06-10
knowledge_type: intentional-design
---

# route-02 — No per-try timeout field — FIXED-VIA-STANDARD-FIELD

## Conclusion

Edgion intentionally has NO `perTryTimeout` field on `HTTPRouteRetry` and no
`edgion.io/*` annotation for it. Per GEP-1731, `timeouts.backendRequest` IS the
per-try timeout: with a retry policy present, an attempt exceeding it is
canceled and retried. Findings that flag "missing per_try_timeout (Envoy
parity)" are closed by this behavior.

Fix suggestions of the form "add a perTryTimeout field to HTTPRouteRetry" or
"patch the vendored Gateway API CRD" are **not accepted**.

## Core Rationale

**1. Gateway API already specifies the mechanism**

GEP-1731: "any retry attempts which reach the configured BackendRequest
timeout duration without a response SHOULD be canceled if possible"; its Envoy
survey equates `per_try_timeout` with `backendRequest`. kgateway documents the
standard field as the per-try timeout; Istio / Envoy Gateway map their
`perTryTimeout` knobs to the same semantics.

**2. Gateway API CRDs are upstream artifacts**

`config/crd/gateway-api/*.yaml` must stay identical to the official release; a
vendor field in `rules[].retry` would be pruned by the K8s API server anyway
unless the CRD were forked.

**3. Implementation**

`configure_peer_timeouts` (`pg_upstream_peer.rs`) folds the read/write bounds
with a source label (`fold_peer_timeouts` → `PeerTimeoutSource`);
`error_while_proxy` Case 1.5 retries upstream Read/Write timeouts whose source
is `BackendRequest` when `retry_configured()`, guarded by attempts / request
deadline / grpc deadline / retry buffer / nothing-written-downstream.
Deadline-sourced timeouts are never retried (Envoy parity). The retry records
`EdgionErrorCode::BackendRequestTimeoutRetry` in the ordered diagnostics.

## Fix Suggestions Not Accepted

- Add `perTryTimeout` to `HTTPRouteRetry` — redundant with `backendRequest` per GEP-1731; the field would be pruned by the API server in K8s mode.
- Patch the vendored Gateway API CRD — upstream artifact, must not diverge.
- `edgion.io/retry-per-try-timeout` annotation — deferred as YAGNI; only reconsider if operators need the per-try ceiling decoupled from the I/O timeout.

## Re-evaluation Triggers

Re-open this decision only if:

- Upstream Gateway API adds a `retry.perTryTimeout` (or equivalent) field.
- Operators demonstrate a need for a per-try wall-clock ceiling different from
  the per-I/O `backendRequest` bound (would revive the annotation option).

## Reference Cases

- Origin finding: `tasks/review-envoy/route-02-no-per-try-timeout.md`, promoted to `tasks/todo/route-02-per-try-timeout.md`, closed 2026-06-10.
- Spec: `docs/superpowers/specs/2026-06-10-per-try-timeout-design.md`
- Docs: `docs/en/user-guide/http-route/resilience/retry.md` (per-try section)
