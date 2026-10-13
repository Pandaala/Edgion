---
name: max-trusted-hops-opt-in
description: Use when reviewing RealIp maxTrustedHops findings; documents why the field is opt-in with None default and why making it required or setting a conservative default is not accepted.
knowledge_type: intentional-design
---

# RealIp `maxTrustedHops` opt-in: Design Decision

## Conclusion

`RealIpConfig.max_trusted_hops` is an **opt-in optional field**, defaulting to `None` (Nginx-compatible behavior, no upper limit). This is a deliberate tradeoff for M12 on 2026-04-22 — provides Envoy-style hop cap as auxiliary defense, but is **not** mandatorily enabled, and the default is **not** changed to a specific number.

When a review reports "RealIp should change `maxTrustedHops` to required / set a conservative default / remove this field", apply the rules below.

## Background Threat (M12)

In `recursive=true` mode, the `X-Forwarded-For` list is traversed right-to-left until the first non-trusted IP is encountered. If `trusted IpList entries` is configured too broadly (e.g., `10.0.0.0/8`) and an attacker can initiate requests from an IP within that range, they can construct a "trusted-IP prefix" of arbitrary length to push the malicious IP to the leftmost position, causing `RemoteIp` to be spoofed.

`maxTrustedHops: Some(N)` stops at the Nth trusted hop during recursive traversal and returns the IP at the current position — closing off this "infinitely extending trusted prefix" path.

## Design Constraints

1. **Default `None` (no upper limit)**: maintains Nginx compatibility; zero breakage on upgrade
2. **Aligned with Envoy `xff_num_trusted_hops` concept**: gives Envoy-familiar operators a familiar control surface
3. **Only meaningful for `recursive=true`**: the field exists in non-recursive mode but is ineffective
4. **Main defense is in `trusted IpList entries`**: `maxTrustedHops` is an auxiliary defense, not the main one; documentation makes "two-layer defense" explicit
5. **Field type is `Option<u32>`**: `None` = no upper limit; `Some(0)` is legal and stops at the first trusted parsed forwarded hop in recursive mode. It differs from nonrecursive mode: a trusted rightmost hop there returns the effective downstream peer. Zero never means unlimited

## Decision Rule

When a review report contains the following patterns, close as "design decision":

1. "`maxTrustedHops` should be required to prevent operator omission"
2. "Should change the default from `None` to `5` (or another specific number) as a conservative default"
3. "Should remove `maxTrustedHops`; the code should enforce single hop"
4. "In non-recursive mode `maxTrustedHops` should error (currently ignored)"

Verify three points before closing:

- The type of `RealIpConfig.max_trusted_hops` is still `Option<u32>`, default `None`
- The recursive branch of `RealIpExtractor::resolve_from_header` still uses a counter to implement the cap (the non-recursive branch does not read this field)
- The user documentation `docs/{zh-CN,en}/user-guide/edgion-plugins/real-ip.md` still has the "Security tradeoffs" / "Security Trade-offs" section, making "first defense is precise `trustedIpRefs`; second is `maxTrustedHops`" explicit

All three satisfied → design decision; point to the M12 FIXED entry in `tasks/security-audit/02-access-control.md` and close.

## Exceptional Scenarios (need reassessment)

- Industry best practice unifies on "mandatory hop cap" (Envoy already requires it) → assess whether to also make Edgion required
- Many "operator was attacked because `maxTrustedHops` was not configured" security-incident reports appear → consider changing to a conservative default (with breaking-change documentation)
- Add a validator error for the `recursive: false + maxTrustedHops` combination (currently silently ignored)

## Same-Category Decisions for Other Opt-In Security Enhancements

This decision belongs to the same category as the following review rules: "**Edgion's bespoke syntactic sugar / optional items, preserve the default behavior + opt-in enhancement**":

- `cors/subdomain-wildcard-https-only.md` (M13) — difference: M13 is a breaking fix (changing shorthand semantics); M12 is non-breaking opt-in
- `oidc/nonce-pkce-defaults.md` (M8) — M8 is doc-only; M12 is code + doc

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/02-access-control.md` M12 (Option B — code + doc)
- Implementation files:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/real_ip.rs`
  - `edgion-gateway/src/util/real_ip_extractor.rs`
- New tests: `test_max_trusted_hops_caps_recursive_traversal`, `test_max_trusted_hops_does_not_block_real_client_within_window`, `test_max_trusted_hops_ignored_in_non_recursive_mode`
