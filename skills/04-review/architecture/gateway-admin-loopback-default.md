---
name: gateway-admin-loopback-default
description: Use when reviewing Gateway admin API network-exposure findings; covers why DEFAULT_GATEWAY_ADMIN_LISTEN now defaults to 127.0.0.1:5810 (loopback) and why reverting to 0.0.0.0 or claiming it breaks K8s readiness is not accepted.
---

# gateway-admin-harden Task 2 — Gateway admin API defaults to loopback bind

> Module: data-plane (Gateway) · Issue type: security · Severity: high · Status: fixed (default changed to `127.0.0.1:5810`)

## Conclusion

`DEFAULT_GATEWAY_ADMIN_LISTEN` (`src/core/gateway/cli/config.rs`) was `0.0.0.0:5810`, so a Gateway
started without an explicit `admin_listen` exposed its admin API to every network interface — and the
admin router still mounts secret-bearing introspection endpoints. The default is now `127.0.0.1:5810`
(loopback), matching Envoy's admin-interface security model (bind localhost, access control via network
topology). This is the network-layer half of the defense-in-depth pair with `fr-grt-01` (serialization
scrub). Re-reports proposing to revert the default to `0.0.0.0`, or claiming the loopback default breaks
K8s readiness, are not accepted — see below.

## Core Rationale

- The admin default is consumed only by `EdgionGatewayConfig::admin_listen_or_default()`, whose only
  caller is `gateway/cli/mod.rs` (`start_auxiliary_services`). init-only path, not the data-plane hot path.
- A secure default makes "expose to the cluster" an explicit operator opt-in (`admin_listen: 0.0.0.0`)
  rather than the factory behavior. Unconfigured operators get the safe default automatically.
- The shipped manifests are unaffected because they set `admin_listen` explicitly:
  `deploy/kubernetes/gateway/configmap.yaml`, `examples/k8stest/kubernetes/gateway/configmap.yaml`, and
  `deploy/standalone/edgion-gateway.yaml` all carry `0.0.0.0:5810`.
- The K8s readiness probe (`deploy/kubernetes/gateway/deployment.yaml`) is an `httpGet` on port 5810,
  which kubelet dials over the **pod IP** — so a pod-IP probe genuinely requires `0.0.0.0`. That is the
  reason the configmap sets it explicitly, and the reason the docs now state: pod-IP `httpGet` probes
  must set `admin_listen` to `0.0.0.0`.

## Fix Suggestions Not Accepted

- "Revert the default to `0.0.0.0` — the loopback default breaks the kubelet readiness probe." — Not
  accepted. The factory K8s manifests already set `admin_listen: 0.0.0.0:5810` explicitly; the loopback
  default only changes behavior for deployments that both (a) omit `admin_listen` and (b) reach the admin
  port from a non-loopback address. Those must opt in explicitly (documented escape hatch).
- "Add a full Bearer/mTLS/IP-allowlist auth middleware instead." — Out of scope for this change and
  deferred; the data plane has no token-distribution channel today. Tracked in
  `tasks/todo/gateway-admin-api-harden.md` ("explicit won't-do / won't-accept") and `tasks/todo/admin-api-https-and-access-control.md`.

## Re-evaluation Triggers

- A token-distribution mechanism lands on the data plane (then auth middleware becomes viable, and the
  bind default is no longer the only control).
- The sensitive `/configclient` endpoints are moved behind `is_integration_testing_mode()` (harden Task 1)
  AND scrubbed (Task 3), at which point the production admin surface is readiness-only and the bind
  default is a smaller concern (but the loopback default should still stand as defense in depth).

## Reference Cases

- `tasks/todo/gateway-admin-api-harden.md` — Task 2 (this change); Task 1 / Task 3 are complementary.
- `skills/04-review/edgion-tls/fr-grt-01-admin-api-secret-scrub.md` — the serialization-scrub half of the
  same defense-in-depth pair.
