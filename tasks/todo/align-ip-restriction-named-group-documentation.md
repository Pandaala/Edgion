# Align IP Restriction Docs with Named IpGroup Schema

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | documentation-correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0082 |

## AI guide summary

Replace obsolete flat CIDR-list examples with the named IpGroup schema everywhere IP restriction is documented.

## Requirements

- Audit HTTP, stream, and TLS-route IP restriction Rustdoc, skills, migrations, and English/Chinese user guides.
- Represent each allow or deny entry with `name`, optional `description`, and `cidrs`.
- Remove old `allowlist`/`denylist` keys and flat-string array examples instead of preserving compatibility aliases.
- Keep DirectPeerIp/RemoteIp, `defaultAction`, partial-good CIDR behavior, and reference semantics accurate.
- Do not change schemas, runtime behavior, or integration tests.

## Acceptance criteria

- Every discoverable IP restriction example parses against the current schema shape.
- English, Chinese, source Rustdoc, and agent knowledge agree on named-group semantics.
- Documentation validation and repository checks pass.
