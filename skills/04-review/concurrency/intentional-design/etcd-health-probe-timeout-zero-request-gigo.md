---
name: etcd-health-probe-timeout-zero-request-gigo
description: Use when reviewing Etcd LinkSys health-probe timeout findings about a misconfigured timeout.request=0 producing a zero-duration tokio::time::timeout that fails every probe — judged accepted GIGO / out of scope, not patched only in the health path.
knowledge_type: intentional-design
---

# Etcd LinkSys health_probe_timeout — request:0 Zero-Duration Timeout (GIGO)

**Status / verdict**: accepted / deferred (2026-06-16, linksys-health-degrade task 02)

When reviewing Etcd `LinkSys` health-probe findings of the form "`health_probe_timeout` (`edgion-common/src/link_sys_core/providers/etcd/config_mapping.rs`) maps an operator-set `timeout.request: 0` to `Duration::from_millis(0)`, so `tokio::time::timeout(0, mc.status())` elapses on the first poll → every monitor probe is reported unhealthy → perpetual reconnect churn, and `ping()` always returns a STATUS timeout": **accepted, not fixed in this task.** Rationale:

- It is a **broader GIGO misconfiguration class, not specific to the health path.** `build_connect_options` (`config_mapping.rs:71-75`) applies the identical `request_ms.min(MAX_REQUEST_TIMEOUT_MS)` with no floor, so the same `request: 0` already passes `with_timeout(0)` to **all** etcd KV operations — the operator's etcd is broken at the operation level regardless of the health probe. Adding a floor only in `health_probe_timeout` would be inconsistent and would mask, not fix, the real problem.
- The proper fix is a **single source of truth**: CRD `minimum: 1` validation on the `timeout.request` field (admission-time), or a shared floor helper used by both `health_probe_timeout` and `build_connect_options`. Either belongs in a dedicated config-validation change, not in the "fix lock + timeout only" scope of task 02 (todo §2).
- Severity is Minor: it requires an explicit, nonsensical operator value (`0`); a blank/unset field correctly falls back to the 5s default, which is the realistic path.

Re-reports of the "request:0 → zero-duration timeout" form against `health_probe_timeout` alone are not accepted — the fix, if pursued, must cover `build_connect_options` and/or CRD validation together. The health-probe timeout itself (prefer configured request, clamp to 60s max, else 5s default) is correct for all sane inputs.
