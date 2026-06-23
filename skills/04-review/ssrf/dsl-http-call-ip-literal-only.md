---
name: dsl-http-call-ip-literal-only
description: Use when reviewing DSL `http.call` `httpBlockPrivateIps` / `is_ssrf_blocked_ip_literal` findings; covers why the guard is IP-literal-only and does not resolve DNS-name targets (accepted tradeoff).
---

# DSL `http.call` SSRF guard is IP-literal-only — DESIGN-TRADEOFF

## Conclusion

`is_ssrf_blocked_ip_literal` (`edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`) intentionally inspects the **URL host string** only and returns `false` for any host that is not an IP literal (`host.parse::<IpAddr>()` → `Err` → not blocked). When `httpBlockPrivateIps: true` is set, a DSL script can still reach a private / loopback / metadata address by using a **DNS name** that resolves to it (e.g. `kubernetes.default.svc.cluster.local`). This is an **accepted tradeoff** (F-018, 2026-05-26), already documented in `skills/01-architecture/02-gateway/dsl-engine-security-model.md` §7.

Fix suggestions of the form "add a blocking `tokio::net::lookup_host` and apply `is_ssrf_blocked_ip_literal` to each resolved address before sending" are **not accepted** as written — see rationale.

## Core Rationale

**1. The trusted-author boundary, not untrusted input**

Invoking `http.call` requires authoring/modifying a DSL script, which requires HTTPRoute / EdgionPlugin CRD write (namespace-admin RBAC). `httpBlockPrivateIps` is itself default-off (`plugin.rs` → `unwrap_or(false)`), operator-opt-in. No genuinely untrusted input drives this path. Same threat class and Low severity as the sibling redirect-bypass finding F-012.

**2. The proposed "resolve then check" fix is racy and gives false assurance**

`reqwest` re-resolves DNS independently at connection time. Checking a pre-resolved address before `builder.send()` does **not** guarantee the connection dials that address — a TOCTOU / DNS-rebinding window remains. To make the guard sound you must pin the resolved IP into the connection (custom connector / `resolve()` override) **and** re-apply the same check to every redirect hop (F-012). That is a large, cross-`reqwest`-layer change for an opt-in defense-in-depth control — out of proportion to the harm.

**3. Synchronous resolver in the data path is explicitly out of scope**

Security model §7: "A future tightening would resolve before checking — out of scope today because it would require a synchronous resolver in the path." The primary defenses remain correct and intact: NetworkPolicy (egress), RBAC (who can write CRDs), IMDSv2 (cloud metadata).

**4. Industry precedent**

ForwardAuth / Webhook SSRF (see [forward-auth-webhook-ssrf.md](forward-auth-webhook-ssrf.md)) reaches the same conclusion for the same reasons: host-string blocklists cannot defend DNS rebinding / hostname forms, and a blanket private-CIDR block would break the legitimate "call internal service" use case.

## Fix Suggestions Not Accepted

- Add blocking DNS resolution + per-address check before send — racy (reqwest re-resolves), needs IP-pinning + redirect-hop coverage to be sound; out of proportion to an opt-in control.
- Block all private CIDRs unconditionally — breaks the common legitimate case (DSL scripts calling in-cluster services), and is a default-off flag anyway.
- Rename `httpBlockPrivateIps` to imply IP-literal-only — flag name is CRD-public API; renaming is a breaking change. The documentation clarification (below) covers the "misleading name" concern instead.

## Accepted mitigation (already applied)

Documentation clarified so operators are not misled into thinking the flag resolves hostnames:
- `docs/en/.../dsl/api-reference.md` + `docs/zh-CN/.../dsl/api-reference.md`: the `http.call` constraint note and the `httpBlockPrivateIps` field row now state "IP literals only — hostnames are not resolved; use network-level egress controls (NetworkPolicy) for hostname-based SSRF."
- The in-source config doc-comment (`config.rs`, `http_block_private_ips`) was already accurate ("private/loopback IP **literal**", "Hostname-based SSRF still requires network-level egress controls").

## Re-evaluation Triggers

Re-open this decision only if:

- A **real SSRF report** (not a code-review finding) cites this gap in a production deployment — the explicit reopen trigger in security model §7.
- A connection-level IP-pinning mechanism becomes available in the `http.call` client stack, making "resolve then dial the checked IP" sound without a separate synchronous resolver round-trip.

## Reference Cases

- F-018 (2026-05-26) — `tasks/plugins/plugins-http-dsl-F018-...`, closed as accepted tradeoff + doc clarification.
- F-012 (2026-05-26) — sibling redirect-bypass of the same guard; **fixed** by switching the DSL HTTP client to a guarded variant whose `Policy::custom` re-runs `is_ssrf_blocked_ip_literal` on every redirect hop when `block_private_ips: true`. See [dsl-http-call-redirect-per-hop.md](dsl-http-call-redirect-per-hop.md). Distinct from this DNS-name limitation, which remains an accepted tradeoff.
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md` §7 — maintainer-view rationale.
- [forward-auth-webhook-ssrf.md](forward-auth-webhook-ssrf.md) — same host-string-blocklist reasoning for ForwardAuth / Webhook.
- [upstream-address-safety.md](upstream-address-safety.md) — the stricter post-DNS-resolution standard that applies to **upstream dial paths** (DynamicExternalUpstream, RegionRoute, etc.), a different threat surface than the trusted-author `http.call` builtin.
