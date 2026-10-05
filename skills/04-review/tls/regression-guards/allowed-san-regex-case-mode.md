---
name: allowed-san-regex-case-mode
description: Review EdgionTls typed SAN regex case flags, bounded compilation, serde reconstruction, and hot reload.
knowledge_type: regression-guards
---

# Allowed SAN regex case mode

## Conclusion and scope

`EdgionTls.spec.clientAuth.allowedSans[].ignoreCase` sets the default case mode
for `RegularExpression`. The default is false; true enables the regex engine's
Unicode simple case folding. Inline `(?i)` / `(?-i)` flags override that default
within their scope. An operator's explicit false does not prohibit inline `(?i)`.

This applies to the SAN value only. The ASN.1 `AllowedSanType` must still match,
and `OtherName` still requires the same OID. DNS Exact uses RFC 6125 matching;
IP Exact compares parsed addresses. Other string comparisons retain ASCII case
folding when `ignoreCase` is true.

## Rationale and evidence

D0016 identified that the CRD accepted `ignoreCase` for RegularExpression while
`AllowedSan::compile` and `allowed_san_config_error` passed only the source pattern
to the bounded compiler. Common executed the resulting case-sensitive regex,
unexpectedly rejecting certificates whose SAN differed only in case. The finding
did not demonstrate an authorization bypass.

Both callers now pass the flag to the single `compile_bounded_regex` function in
`edgion-resources/src/resources/allowed_san.rs`, which configures
`RegexBuilder::case_insensitive`. Controller `validate_cert` uses this option
through `allowed_san_config_error`; Gateway `TlsStore` calls
`compile_allowed_sans_for_runtime` after deserialize and before publication.
`compiled_regex` remains excluded from serde and schemars.

The configuration contract is documented in the EdgionTls CRD, the
[feature schema](../../../02-features/03-resources/09-edgion-tls.md), and the
[operator guide](../../../../docs/en/ops-guide/gateway/tls/edgion-tls.md).
Configuration-time compilation follows
[design rule U](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory).

## Alternatives and limits

Rejecting `ignoreCase: true` for regexes was considered; honoring the exposed
option gives it useful semantics without adding a new configuration shape.
No inline-flag rewriting or lowercase copy is required. Common continues to
execute the compiled matcher during the handshake, with no regex compilation
or tracing added there.

Source length remains bounded to 1,024 bytes and program/DFA limits remain
1 MiB. An invalid or uncompiled entry in the active allowlist makes Common
matching fail closed. Invalid configuration updates remain subject to
[same-key last-known-good retention](../intentional-design/last-known-good-follows-resource-key.md):
the previous valid TLS configuration, including its mTLS policy, may continue
to serve. Case folding can enlarge a compiled program; admission and Gateway
compilation therefore must use the same flag and bounds.

## Verification and re-evaluation

Current-run verification passed. The regression coverage includes:

- Resources tests for both flag values, inline and scoped overrides, Unicode
  folding, and agreement between admission/runtime rejection of invalid or
  oversized regexes.
- Common tests for both modes, uncompiled rejection, and SAN type/OID isolation.
- Gateway TlsStore tests for serde reconstruction, full/partial publication,
  repeated flag changes, and retained generation independence.
- `AllowedSanRegexIgnoreCase` on the shared integration mTLS listener: fresh
  handshakes observe sensitive/insensitive/sensitive/insensitive updates while
  a control handshake remains valid and an unrelated SAN stays rejected.
  The original fixture is restored on success and failure.

Executed checks:

- `cargo test -p edgion-resources --lib resources::allowed_san::tests`: 11 passed,
  including the `[a-z]{5000}` case-folding expansion boundary.
- `cargo test -p edgion-common --lib allowed_san::tests`: 14 passed.
- `cargo test -p edgion-gateway --lib regex_ignore_case_reconstructs_after_serde_and_updates_published_generations`:
  1 passed.
- `cargo test -p edgion-resources --test crd_coverage`: 19 passed.
- On one shared topology started with `start_all_with_conf.sh --suites
  base,HTTPRoute/Basic,EdgionTls/mTLS`: static mTLS 11/11 passed, dynamic
  `AllowedSanRegexIgnoreCase` passed, and the restored static mTLS suite passed
  11/11. Services were stopped after the run.
- Applied `cargo fmt --all`; workspace all-target check and Clippy passed on
  the final code. Agent-doc consistency, SSA force, metrics inventory, Pingora
  source policy, Gateway API transform contract, unit-test layout, and Gateway
  tracing guards passed.
- Kubernetes suite parity (57/57 dynamic registrations), its regression test,
  fixture ownership, dynamic runner regression, and dynamic enumeration passed.
- Independent read-only review: LGTM after correcting Kubernetes registration
  and the phase suite's explicit HTTPS port kind.
- Additional deep independent Astra review: LGTM after clarifying the
  last-known-good boundary; independently reran the 11 Resources, 14 Common,
  and 1 Gateway regression tests successfully.

The top-level local runner stopped at the macOS loopback-alias preflight before
starting services; it requires aliases used by unrelated LB/GeoIP suites. The
targeted shared-topology run above exercised the real Controller, conf_sync,
Gateway and mTLS handshake using only 127.0.0.1. No actual Kubernetes cluster run
or full workspace test run was performed.

The restoration probes confirm the original handshake behavior after submitting
the original fixture. They do not independently inspect the Gateway's restored
resource contents; the preceding case-insensitive generation has the same
accept/reject outcome for these probes.

Re-evaluate if regex library case semantics change, compilation bounds change,
new SAN matching consumers appear, or configuration publication stops rebuilding
the skipped matcher after deserialization. Filesystem integration results do
not establish Kubernetes deployment verification.

## Resolution references

- D0016 / `allowed-san-regex-ignore-case-contract`: ignored accepted flag;
  fixed with a unified bounded compiler option, regression coverage, matched
  schema/operator documentation, independent LGTM, and successful closure checks.
