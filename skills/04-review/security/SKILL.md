---
name: review-security
description: Review knowledge for security, grouped by decision type.
---

# security review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Fail-closed error body must not leak internal plugin/failure detail](mandatory-design/fail-closed-error-body-no-internal-detail.md)
- [Security Iron Rules](mandatory-design/iron-rules.md)
- [Secret Header Values Must Be Sensitive — FIXED](mandatory-design/secret-header-values-must-be-sensitive.md)

## Intentional design

- [Auth Identity Headers Overwrite Written Names Only — ACCEPTED-TRADEOFF](intentional-design/auth-identity-headers-overwrite-written-names.md)

## Regression guards

- [Authentication challenge realm quoted strings](regression-guards/auth-challenge-realm-quoted-strings.md)
- [AI backend ReferenceGrant decisions are enforced through dispatch](regression-guards/ai-backend-referencegrant-enforced-through-dispatch.md)
- [Anthropic translated responses rebuild their headers](regression-guards/anthropic-translated-responses-rebuild-headers.md)
- [Elasticsearch bulk NDJSON index-action header — serde_json construction](regression-guards/es-ndjson-index-header-serde-json.md)
- [File-backend `namespace` path traversal — FIXED (fr-csync-01, 2026-05-29)](regression-guards/file-backend-namespace-path-traversal.md)
- [Gateway StreamPlugins attachments are same-namespace](regression-guards/gateway-stream-plugins-same-namespace.md)
- [L4 Relay Upstream Connect Timeout — FIXED](regression-guards/l4-relay-connect-timeout.md)
- [L4 Relay Idle Timeout — NOT-AN-ISSUE](regression-guards/l4-relay-idle-timeout.md)
- [Proxy-Wasm Runtime Hardening](regression-guards/proxy-wasm-runtime-hardening.md)
- [`lint_rbac_policy` dangerous-grant check covers `watch` — FIXED (m1-01, 2026-05-29)](regression-guards/rbac-lint-watch-dangerous-grant.md)
- [Wasm source-keyed module cache bypasses per-resource SHA verification — FIXED](regression-guards/wasm-cache-key-includes-sha256-pin.md)
