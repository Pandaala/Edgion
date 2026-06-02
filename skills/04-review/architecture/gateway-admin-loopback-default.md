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
- Since the probe-port-separation change (2026-06-02), the shipped manifests no longer carry an
  `admin_listen: 0.0.0.0:5810` override at all — the configmaps rely on the loopback default and the
  admin port is intentionally not cluster-reachable. `/health` + `/ready` moved to a dedicated probe port
  (`probe_listen`, default `0.0.0.0:28000`), so kubelet no longer dials the admin port.
- The K8s readiness/startup probes (`deploy/kubernetes/gateway/deployment.yaml`) are `httpGet` on port
  **28000** (the probe port), which kubelet dials over the **pod IP**. The probe port carries no sensitive
  data, so binding `0.0.0.0` there is safe; the admin port stays loopback. This removes the old tension
  where a pod-IP readiness probe forced `admin_listen` to `0.0.0.0`.

## Fix Suggestions Not Accepted

- "Revert the default to `0.0.0.0` — the loopback default breaks the kubelet readiness probe." — Not
  accepted. Readiness now rides the dedicated probe port (28000, bound `0.0.0.0`), so the admin bind has
  no bearing on kubelet probing at all. The loopback admin default only changes behavior for deployments
  that reach the admin port from a non-loopback address; those must opt in explicitly (documented escape
  hatch: `kubectl port-forward pod/<name> 5810:5810`, not the Service).
- "Add a full Bearer/mTLS/IP-allowlist auth middleware instead." — Out of scope for this change and
  deferred; the data plane has no token-distribution channel today. Tracked in
  `tasks/todo/gateway-admin-api-harden.md` ("explicit won't-do / won't-accept") and `tasks/todo/admin-api-https-and-access-control.md`.

## Re-evaluation Triggers

- A token-distribution mechanism lands on the data plane (then auth middleware becomes viable, and the
  bind default is no longer the only control).
- The sensitive `/configclient` endpoints are scrubbed of secrets (harden Task 3, generic field-level
  redaction), at which point the admin surface no longer leaks secret bytes even if mis-bound (but the
  loopback default + probe-port separation should still stand as defense in depth). Note: harden Task 1
  was redefined into the probe-port separation (readiness moved off the admin port), already shipped.

## Reference Cases

- `tasks/todo/gateway-admin-api-harden.md` — Task 2 (this change); Task 1 / Task 3 are complementary.
- `skills/04-review/edgion-tls/fr-grt-01-admin-api-secret-scrub.md` — the serialization-scrub half of the
  same defense-in-depth pair.
