# Unify Auth Identity-header Sanitization

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define one gateway-owned contract for authentication identity headers so client-forged values cannot survive authenticated or anonymous processing, including routes with multiple auth plugins.

## Requirements

- Inventory every auth plugin that reads, removes, or writes consumer username, group, anonymous-consumer, and related gateway identity headers.
- Define canonical owned header names and sanitization timing before any auth result can reach an upstream.
- Remove client-supplied owned identity headers on authenticated success, anonymous allowance, rejection, and transitions between those states according to the approved contract.
- Define deterministic multiple-auth-plugin ordering and prevent a later anonymous or failed plugin from reviving stale identity from an earlier plugin.
- Validate configurable output header names and values before runtime; invalid resolved values must fail closed without leaking internal detail.
- Centralize shared mutation/sanitization behavior rather than duplicating removal lists in each plugin.
- Preserve PluginLog/access-log ownership, sensitive username masking, and generic failure bodies.

## Acceptance criteria

- Forged `X-Consumer-Username`, `X-Anonymous-Consumer`, and every other canonical identity header never reaches the upstream unless emitted by the approved gateway auth result.
- Authenticated, anonymous, rejected, and multi-plugin paths have explicit deterministic header outcomes.
- Invalid configured or resolved identity header names/values fail closed before upstream dispatch.
- BasicAuth, KeyAuth, JwtAuth, HmacAuth, HeaderCertAuth, LDAP, and any other emitting consumer use one canonical mechanism.
- Focused plugin/runtime tests, the dependent upstream-observation integration scenario, workspace check, Clippy, and repository guards pass.

## Related work

- [`Add auth identity-header spoofing integration coverage`](../add-auth-identity-header-spoofing-integration-coverage.md)
