# Align HMAC Auth Quick-Start Secret Format

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | docs |
| Priority | P1 |
| Issue | N/A |
| Source | D0036 credential ownership audit |

## AI guide summary

Correct the English and Chinese HMAC Auth quick starts so their Kubernetes Secret examples use the `credentials.yaml` YAML-list payload that the Controller resolver actually requires.

## Requirements

- Update both `docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` and `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/hmac-auth.md`.
- Replace the incompatible top-level `username` and `secret` data entries with a `stringData.credentials.yaml` block containing a YAML list of credential mappings.
- Keep the sample compatible with the default `usernameField`, `secretField`, and `minSecretLength` values and with the existing `secretGroups[].secretRefs` example.
- Keep the English and Chinese examples structurally and semantically aligned.
- Do not change the resolver, resource schema, authentication behavior, or existing integration tests.

## Scope

### In scope

- Documentation examples and nearby explanatory text needed to describe the required Secret payload.
- Documentation/link validation of both language variants.

### Out of scope

- Runtime compatibility for the obsolete two-key Secret shape.
- HMAC signing semantics, credential ownership, or Secret resolution changes.

## Acceptance criteria

- Both quick starts show `credentials.yaml` as a valid YAML list with at least one `username`/`secret` mapping.
- The examples match `resolve_credential/hmac.rs` and can be parsed by the same YAML-list shape.
- English and Chinese documentation remain synchronized.
- Documentation validation and `git diff --check` pass; existing integration tests remain unchanged.
