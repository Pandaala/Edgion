# D0049 — Align IP Restriction Docs with Named IpGroup Schema

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0082, IpRestriction audit
- Category: documentation-correctness
- Canonical task: [`tasks/todo/align-ip-restriction-named-group-documentation.md`](../../../todo/align-ip-restriction-named-group-documentation.md)

## Evidence
IP restriction Rustdoc, skills, migration material, and English or Chinese user examples still contain obsolete flat `allow: ["CIDR"]` / `deny: ["CIDR"]` or old allowlist/denylist forms. The accepted schema requires named `IpGroup` entries with `name`, optional `description`, and `cidrs`.

## Why deferred
The cleanup spans source documentation and the bilingual user/knowledge set. It should update every stale example coherently without changing the already-correct runtime schema or adding compatibility aliases.

## Integration-test impact
No integration-test issue is required because this is documentation alignment with an already validated schema.
