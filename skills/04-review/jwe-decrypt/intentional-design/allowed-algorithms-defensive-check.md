---
name: jwe-allowed-algorithms-defensive-check
description: Use when reviewing JWE Decrypt findings that call the runtime allowedAlgorithms check or JweFailure::AlgDenied unreachable dead code.
status: accepted-tradeoff
finding_id: gw-plugin-auth-a-03
closed: 2026-08-02
knowledge_type: intentional-design
---

# JWE Allowed-Algorithm Runtime Check — ACCEPTED-TRADEOFF

## Conclusion

Keep the runtime `allowed_algorithms` check and `JweFailure::AlgDenied` even though the branch is unreachable through currently validated production configurations. It is a cheap defense-in-depth guard with no meaningful request-path cost.

Fix suggestions that remove the field, branch, failure variant, or response mappings solely because the branch is currently unreachable are **not accepted**.

## Core Rationale

**1. Current validation makes the branch unreachable**

`JweDecryptConfig::detect_validation_error` requires `allowed_algorithms`, when present, to contain `content_encryption_algorithm`. `JweDecrypt::parse_compact_jwe` first requires the request-side `enc` to equal that configured algorithm, so the subsequent allowlist lookup must succeed. `JweContentEncryption` currently has only `A256GCM`, reinforcing the same conclusion.

**2. The redundant runtime guard is intentional defense in depth**

The Gateway validates plugin configuration before constructing a production `JweDecrypt` instance, but retaining the local guard protects the parsing boundary if validation or construction wiring changes incorrectly later. The check is a bounded lookup over an operator-configured algorithm list and does not create a meaningful performance concern.

**3. Removing the scaffolding has negative value today**

Deleting it would require coordinated schema, runtime, documentation, example, and error-mapping churn without changing observable behavior. Keeping the guard also leaves the request parser explicit about its accepted-algorithm boundary.

## Fix Suggestions Not Accepted

- Remove `allowed_algorithms`, `JweFailure::AlgDenied`, and their mappings — broadens a zero-impact cleanup across the public configuration surface.
- Remove only the runtime branch — discards a low-cost local safety check against future validation or construction regressions.
- Add a source comment explaining the current unreachability — duplicates the cross-file invariant without improving runtime safety; this review rule is the canonical explanation.

## Re-evaluation Triggers

Re-open this decision only if:

- `JweContentEncryption` gains another variant.
- `content_encryption_algorithm` changes from a single selected algorithm to negotiation or multi-algorithm acceptance.
- The validation invariant or the production `JweDecrypt` construction path changes.
- Profiling demonstrates that this bounded lookup has measurable request-path cost.

## Reference Cases

- `gw-plugin-auth-a-03`, reviewed 2026-08-02.
- `edgion-gateway/src/plugins/http/jwe_decrypt/plugin.rs`
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/jwe_decrypt.rs`
- `docs/en/user-guide/edgion-plugins/jwe-decrypt.md`

## Resolution references

- `gw-plugin-auth-a-03` — 2026-08-02 (⚠️ accepted tradeoff).
