---
name: retry-keeps-logical-backend-selection
description: Use when reviewing credential or routing findings that assume an ordinary upstream retry can switch to another backendRef.
status: false-positive
finding_id: CI-R1
closed: 2026-08-27
knowledge_type: review-evidence
---

# Retry Keeps the Logical Backend Selection — NOT AN ISSUE

## Conclusion

Ordinary HTTP and gRPC retries keep the initially selected logical backend and may only
reselect an endpoint within that backend's Service. A deferred credential mutation therefore
cannot cross a `backendRef` through the current retry path.

Fix suggestions that rebuild credentials merely because an endpoint retry begins are **not
accepted**.

## Core Rationale

**1. HTTP selection is guarded by request-lifetime state**

The HTTP upstream-peer path selects a backend only while both `selected_backend` and
`selected_grpc_backend` are absent. Later attempts reuse that logical selection and run only
endpoint selection for its Service.

**2. gRPC has the same request-lifetime invariant**

The gRPC integration returns early when `selected_grpc_backend` is already populated, so retry
does not rerun weighted `backendRef` selection.

**3. Provider failover is a separate mechanism**

AI provider failover restores its request-header baseline and removes the previous provider's
credential before installing the next candidate. It must not be inferred from the ordinary
HTTP/gRPC endpoint-retry behavior.

## Fix Suggestions Not Accepted

- Recompute credential mutations on every endpoint retry — the logical secret owner is unchanged,
  and this adds hot-path work without changing isolation.
- Clear all deferred mutations at each retry boundary — this would remove credentials that the
  fixed backend still requires.
- Disable endpoint retry for credential-bearing requests — endpoint retry does not cross the
  credential's Service boundary.

## Re-evaluation Triggers

Re-open this decision only if:

- ordinary retries gain cross-`backendRef`, cross-Service, or cross-cluster failover; or
- backend selection state becomes attempt-scoped instead of request-scoped.

Any such design must make credential cleanup and installation explicit at the same failover
boundary.

## Reference Cases

- CI-R1 security review, 2026-08-27
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- `edgion-gateway/src/routes/grpc/integration.rs`
- `edgion-gateway/src/ai/runtime/dispatch.rs`

## Resolution references

- `CI-R1` — 2026-08-27 (false positive).
