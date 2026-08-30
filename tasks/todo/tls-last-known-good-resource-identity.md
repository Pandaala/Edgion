# Retain TLS last-known-good only for the same resource identity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Bind TLS last-known-good retention to resource identity rather than only `namespace/name`. A reconnect full snapshot currently cannot distinguish an invalid update to the same Kubernetes object from deletion followed by creation of a different object at the same key, so it can retain and serve certificate identity and listener/host side effects from a deleted generation.

## Requirements

- Retain a valid certificate for an invalid replacement only when the replacement is the same logical resource generation.
- A recreated resource with a different Kubernetes UID must clear all previous matcher side effects and remain invalid/fail-closed.
- Define a conservative identity rule for FileSystem and etcd inputs, where Kubernetes UID may be unavailable.
- Keep partial delete-to-add and reconnect full-snapshot behavior consistent.
- Preserve an empty full snapshot as an unconditional complete clear.
- Do not weaken certificate, hostname, mTLS, or secret validation.

## Scope

### In scope

- TLS store identity metadata and last-known-good decisions.
- Conf-sync full and compressed event inputs needed to preserve identity.
- Kubernetes, FileSystem, and etcd identity semantics.
- Focused store and reconnect tests.

### Out of scope

- TLS matcher algorithm or Gateway rustls parity.
- General conf-sync protocol redesign unrelated to resource identity.
- Certificate issuance or rotation policy.

## Acceptance criteria

- An invalid update with the same Kubernetes UID retains the valid last-known-good certificate.
- A delete/recreate with a different UID removes the old host/listener/certificate side effects and does not serve the old identity.
- A reconnect full snapshot produces the same result as the equivalent partial delete/add history.
- FileSystem and etcd inputs have documented, conservative, deterministic identity rules and corresponding tests.
- Missing identity never silently authorizes retention across a suspected recreation.
- Valid recovery re-arms rejection transition logging, and empty full-set clearing remains unchanged.
- The handshake matcher remains lock-free and performs no certificate parsing or tracing.
- Targeted TLS/conf-sync tests, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/04-tls/SKILL.md`
- `skills/01-architecture/03-controller-gateway-link/SKILL.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Fix the partial `add` path locally, but defer reconnect/full-snapshot identity. | Compressed `add` explicitly identifies a new generation; a full snapshot supplies no event history and needs a cross-source identity contract before changing last-known-good behavior. |
