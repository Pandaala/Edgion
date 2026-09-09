# Reconcile TLSRoute Version Guidance

## Meta

- Status: pending
- Type: docs
- Priority: P3

## Evidence

`skills/02-features/03-resources/07-tlsroute.md` contains a Multi-version
Compatibility section describing v1-to-v1alpha3 fallback and FileSystem version
normalization. Root `AGENTS.md` requires atomic API-version upgrades without
probing, fallback, dual watch, or legacy conversion paths. The feature page must
not override this requirement or serve as proof that fallback is intended.

## Acceptance Criteria

- Inspect the actual Controller, static-input, resource, and CRD paths to identify
  the canonical TLSRoute version and any remaining implementation discrepancy.
- Correct the owning feature page and directly related references to match the
  authoritative policy; preserve useful history as explicitly historical.
- If code violates the policy, record that independently rather than silently
  relaxing the requirement or expanding this documentation task.
- Keep this work separate from L4 incremental-update helper cleanup and preserve
  existing integration files without explicit authorization.
