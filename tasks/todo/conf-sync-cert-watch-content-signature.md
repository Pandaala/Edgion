# Conf-Sync Certificate Watch Content Signature

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

The Controller and Gateway conf-sync certificate watchers suppress reloads when
the observed `(mtime, length)` tuple is unchanged. A real same-size certificate,
key, or CA replacement can therefore be ignored when timestamps are preserved or
have insufficient resolution, even after the filesystem watcher delivered an
event. Design one shared content-sensitive decision model for both binaries; do
not patch only one watcher or force a reconnect for every directory event.

## Confirmed findings

- Both watcher implementations use only modification time and file length as the
  reload signature.
- The notify-event path and periodic re-stat backstop use the same signature, so
  neither path recovers from an unchanged tuple.
- PEM rotations commonly preserve approximate file sizes, and timestamp behavior
  depends on the filesystem and Secret-mount replacement mechanism.
- Unconditionally reloading on every directory event would recycle all conf-sync
  streams for unrelated metadata changes and can create reconnect churn.

## Requirements

- Detect content changes for the active certificate, key, and CA files even when
  length and modification time are unchanged.
- Keep Controller and Gateway watcher behavior equivalent; prefer a shared helper
  if the workspace dependency graph permits it.
- Do not retain raw PEM content after comparison. A bounded in-memory digest is
  permitted as the signature, but neither the digest nor PEM/key material may be
  logged or exposed through metrics or admin surfaces.
- At the scheduled periodic backstop interval, read and hash all three files even
  when their metadata tuple is unchanged; keep the frequency, bytes, and scratch
  memory bounded so this does not become continuous polling.
- Preserve validate-before-swap and the currently installed channel/server on an
  invalid replacement.
- Coalesce a Secret-mount event burst into at most one validated reload attempt.
- Define how transient missing files affect the signature and retry behavior.

## Acceptance tests

- Same-length replacement with an unchanged synthetic timestamp triggers one
  reload decision.
- Byte-identical replacement with different metadata does not recycle streams.
- The retained comparison state is bounded to fixed-size metadata/digests and
  contains no raw certificate, key, or CA bytes.
- A changed certificate with a mismatched key is rejected while the previous
  channel/server identity remains installed.
- Missing-to-present and present-to-missing transitions retain the documented
  retry and error-latching behavior.
- Controller and Gateway decision tests use the same vectors.
- Normal workspace guards and focused watcher tests pass.

## Scope

### In scope

- `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs`
- `edgion-controller/src/conf_sync/conf_server/cert_watch.rs`
- A backend-independent shared signature helper when dependency boundaries allow
- Focused certificate watcher tests

### Out of scope

- Changing the active certificate-set selection at runtime
- Removing validate-before-swap
- Logging certificate or key material
- Broad conf-sync reconnect-policy changes

## Related context

- `skills/00-design-rules.md`
- `skills/01-architecture/03-controller-gateway-link/`
- `AGENTS.md` conf-sync transport-security rules
