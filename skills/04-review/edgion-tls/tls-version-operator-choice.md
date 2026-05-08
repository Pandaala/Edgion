# `EdgionTls.min_tls_version` Allowing `TLS1_0` / `TLS1_1`: Deliberate Opt-In, Not Rejected

## Conclusion

The `EdgionTls` CRD's `min_tls_version` field accepting `TLS1_0` / `TLS1_1` is a **deliberate opt-in design**, a channel preserved for legacy client scenarios (old IoT, payment terminals, offline legacy systems). It **should not** be made to return a validation error in the Controller validator, nor be excluded via `#[cfg]` in release builds. The operator takes responsibility for choosing an unsafe version.

## Reasons

1. **The default path is already safe**: when `min_tls_version` is not set, BoringSSL defaults to TLS 1.2+. Only if the operator **explicitly** chooses `TLS1_0` / `TLS1_1` is the attack surface exposed — this is an informed choice, not default drift
2. **User documentation already warns clearly** (see the "Supported versions" table in `docs/{zh-CN,en}/ops-guide/gateway/tls/edgion-tls.md`):
   - `TLS1_0` / `TLS1_1` → "⚠️ Not recommended (deprecated)"
   - `TLS1_2` → "✅ Recommended"
   - `TLS1_3` → "✅ Most secure"
3. **The Rust-side doc comment warns in sync**: doc comments for the `TlsVersion` enum and `min_tls_version` field explicitly cite RFC 8996, recommending `TLS1_2` as the baseline; exposed via JSON schema to `kubectl explain` / schema viewers
4. **Industry consistency check**: Nginx `ssl_protocols` does not restrict; Envoy `tls_params.tls_minimum_protocol_version` does not restrict; rejection happens at the **control plane / product layer** (Istio, cloud LB) rather than in the proxy engine itself. As a proxy engine, Edgion stays consistent
5. **Hard rejection costs more than the defensive benefit**:
   - Cuts off all legitimate legacy deployments (no escape hatch)
   - The cost of an operator's wrong choice falls on their own client connections (visible, auditable); it does not silently spread to other components
   - Forced removal is equivalent to Edgion unilaterally declaring "no support" for a legitimate business scenario

## Decision Rule

When a review/audit report contains the following patterns, close as "operator's informed choice":

1. "TLS 1.0 / 1.1 can be configured by the operator; should be rejected in the validator"
2. "Should reject TLS 1.0/1.1 in release builds via `#[cfg(not(debug_assertions))]`"
3. "RFC 8996 has deprecated; should forcibly remove the TLS 1.0/1.1 enum variants"
4. "Should opt-in at annotation level to enable legacy TLS (currently too permissive)"

Verify three points before closing:

- The `TlsVersion` enum still contains the `Tls10` / `Tls11` variants, not excluded by `#[cfg]`
- `EdgionTlsHandler::validate()` does not return an error for `min_tls_version = Tls10 | Tls11`
- The "Supported versions" table in `docs/{zh-CN,en}/ops-guide/gateway/tls/edgion-tls.md` still contains the explicit ⚠️ annotation; doc comments for `TlsVersion` / `min_tls_version` in `edgion_tls.rs` still cite RFC 8996 deprecation

All three satisfied → deliberate design; point to `tasks/security-audit/05-tls-cert-acme.md` M19 NOT-AN-ISSUE decision and close.

## Exceptional Scenarios (need reassessment)

- Industry best practice migrates to "proxy engine layer rejects TLS 1.0/1.1 by default" (e.g., a future Envoy version hard-rejects) → reassess whether to follow
- Multiple "operator inadvertently configured TLS1_0 leading to compliance incident" feedbacks appear → consider adding an opt-in annotation mechanism (`edgion.io/allow-insecure-tls-version: "true"`), changing the default to reject
- Compliance certifications (PCI-DSS 4.0 / FIPS 140-3) require the proxy layer to forcibly reject → hard-reject per the certification requirement

## Cross-Reference with Similar Opt-In Review Decisions

- `observability/system-error-state-metric.md` (M2 `skip_tls`) — preserve escape hatch + observability backstop
- `real-ip/max-trusted-hops-opt-in.md` (M12) — opt-in enhancement + main defense in config rather than code
- This file (M19) — opt-in degradation + safe code default + multi-layer documentation warnings

The three share a common pattern: **safe default + opt-in channel preserved + multi-layer explicit warning + operator takes responsibility**, used for scenarios that "cannot be one-size-fits-all rejected, nor allow default drift".

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/05-tls-cert-acme.md` M19
- Changes this round: only Rust-side doc comment enhancement, zero code behavior change
- References: RFC 8996 (2021-03 deprecates TLS 1.0/1.1); client removal in Chrome 84 / Firefox 78 / Safari 14 / Edge 84
