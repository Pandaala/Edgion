# common-01: Egress classifier misses well-known cloud metadata addresses (Alibaba 100.100.100.200, AWS/GCP IPv6 IMDS)

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification)
- **Scope**: local
- **Location**: `edgion-common/src/egress.rs:46` (`classify_ip`), `edgion-common/src/egress.rs:71` (`classify_v4`), `edgion-common/src/egress.rs:268` (`is_ip_allowed_with_observer`); consumers `edgion-gateway/src/util/net.rs:43` (`classify_backend_addr`), `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:2228`, `edgion-gateway/src/geoip/updater.rs:185`, `edgion-gateway/src/plugins/http/wasm/oci/mod.rs:508`, `edgion-common/src/dns/http_fetcher.rs:108`

## Summary
The project's address-safety contract is "always reject loopback and link-local / **metadata**, allow
RFC1918 by default" (`skills/04-review/ssrf/mandatory-design/upstream-address-safety.md`). The single
canonical classifier implements "metadata" purely as `169.254.0.0/16` + `fe80::/10`. Several
production cloud metadata endpoints live outside those ranges and are classified `Global` or
`UniqueLocal`, so they pass every egress and upstream-address guard under the default policy.

## Current behavior (evidence)
`classify_v4` (`egress.rs:71-84`) only recognizes `is_loopback`, `is_link_local`, `is_private`,
`is_unspecified`; everything else is `IpClass::Global`. `classify_ip` (`egress.rs:46-69`) maps
`fc00::/7` to `UniqueLocal` and otherwise `Global`. `is_ip_allowed_with_observer`
(`egress.rs:268-275`) always allows `Global`, and allows `UniqueLocal` unless `block_private` is set
(Gateway default: `EGRESS_BLOCK_PRIVATE = false`, `edgion-gateway/src/plugins/http/common/outbound/egress.rs:24`;
backend dial path `classify_backend_addr` never blocks `UniqueLocal`).

Concretely accepted today under default settings:

| Address | What it is | Edgion class | Result |
|---|---|---|---|
| `100.100.100.200` | Alibaba Cloud ECS/ACK instance metadata (RAM role STS credentials) | `Global` (100.64/10 not recognized) | allowed everywhere, even with `block_private: true` |
| `fd00:ec2::254` | AWS IMDS IPv6 endpoint (Nitro) | `UniqueLocal` | allowed unless `block_private` |
| `fd20:ce::254` | GCP metadata server IPv6 | `UniqueLocal` | allowed unless `block_private` |
| `64:ff9b::a9fe:a9fe` | NAT64-embedded 169.254.169.254 | `Global` | allowed (only reachable where a NAT64 translates to link-local; low likelihood) |
| `0.0.0.0/8` (non-zero), `240.0.0.0/4`, `255.255.255.255`, `224.0.0.0/4` | reserved / broadcast / multicast | `Global` | allowed |

The same classifier gates the main LB dial path (`classify_backend_addr`), ExternalName/DNS jumps,
AI upstream resolution (`edgion-gateway/src/ai/http/attempt.rs:250`), OIDC/forward-auth/webhook,
Wasm callouts and fetches, OTLP/Redis/LDAP/ext_proc dials and the HTTPDNS endpoint check, so the gap
is shared by every path that the review canon requires to reject metadata.

## Industry comparison
- Stripe Smokescreen's default deny list and Go/ruby `ssrf_filter`-style libraries block
  `100.64.0.0/10`, `0.0.0.0/8`, `192.0.0.0/24`, `198.18.0.0/15`, `240.0.0.0/4`, `64:ff9b::/96`,
  `2002::/16` (6to4), Teredo, plus the explicit IPv6 IMDS addresses.
- Istio/Envoy do not classify in-proxy, which is why Edgion's "defense-in-depth classifier" exists at
  all; but where gateways do implement SSRF filters (Kong `ssrf-protection` patterns, APISIX
  `ssrf` checks in `forward-auth`) the metadata list is cloud-specific, not link-local only.

## Impact
On Alibaba Cloud (ACK) — and on AWS/GCP IPv6-enabled nodes — any path the canon declares
metadata-safe can reach the instance metadata service: a DynamicExternalUpstream/ExternalName pointed at
a hostname resolving to `100.100.100.200`, a DSL `http.call`, an OIDC discovery URL, an HTTPDNS
endpoint. The node RAM-role/IAM credentials are the classic SSRF prize. `block_private: true` does not
help for the Alibaba case because 100.64/10 is not "private" in Rust's `is_private()`.

## Suggested direction
1. Add an explicit `IpClass::Metadata` (or fold into `LinkLocal`) for a short, documented list of
   well-known metadata endpoints: `100.100.100.200/32`, `fd00:ec2::254/128`, `fd20:ce::254/128`
   (plus the existing 169.254/16, fe80::/10). Always blocked, same as link-local; no knob.
2. Classify `100.64.0.0/10` as a "shared/private" class that follows `block_private` (CGNAT space is
   used for pod networks by some CNIs, so it must not be unconditionally blocked).
3. Normalize NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`) embedded IPv4 before classification, the
   same way IPv4-mapped is already handled; classify `0.0.0.0/8`, `240.0.0.0/4`, broadcast and
   multicast as non-dialable (`Unspecified`-like).
4. Because every consumer uses exhaustive matches on `IpClass` (by design), adding a variant forces each
   call site to make an explicit decision at compile time.

## Notes
Checked `skills/04-review/ssrf/` (all pages), especially `mandatory-design/upstream-address-safety.md`
(defines metadata as part of the always-blocked set; it lists only 169.254/16 and fe80::/10 as
examples, it does not decide that other metadata endpoints are acceptable) and
`regression-guards/ipv4-mapped-ipv6.md` (same class of normalization bug, fixed for `::ffff:`). No
task under `tasks/` covers CGNAT/IPv6 IMDS. This does not propose blocking RFC1918 or ULA by default.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P2 (was P1)
- `edgion-common/src/egress.rs:45-84`: `classify_v4` recognizes only loopback/link-local/private/unspecified; 100.64/10 (incl. 100.100.100.200) is `Global`; IPv6 only special-cases fc00::/7, fe80::/10, ::1, ::, and IPv4-mapped.
- `edgion-common/src/egress.rs:268-280`: `Global` always allowed; `UniqueLocal` (fd00:ec2::254, fd20:ce::254) allowed unless `block_private`.
- `skills/04-review/ssrf/mandatory-design/upstream-address-safety.md` requires rejecting link-local/metadata on every dial path but its Review Decision section rates a bypass of this defense as "typically P2 security hardening / defense bypass" (not arbitrary user SSRF).
- Downgraded P1→P2 to match that canon: exploitation needs an operator/route-author-configured destination resolving to the metadata IP; still a real, cheap-to-fix gap on Alibaba Cloud and IPv6 IMDS.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `edgion-common/src/egress.rs` is unchanged on this branch; `classify_v4`/`classify_ip` still have no metadata/CGNAT/NAT64 handling.
- `skills/04-review/ssrf/` is unchanged. Updated the AI consumer reference to `attempt.rs:250` (the `resolve` call moved after a match-arm merge).
