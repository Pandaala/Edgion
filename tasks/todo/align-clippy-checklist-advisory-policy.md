# Align the Clippy checklist with the explicit lint policy

## Meta

- Created: 2026-10-28
- Status: pending
- Type: docs

## Evidence and scope

`skills/07-tasks/SKILL.md` asks for warning-free workspace Clippy. Root `AGENTS.md` requires the prescribed Clippy command to pass, while `Cargo.toml` explicitly states warnings are advisory and `.github/workflows/ci.yml` does not pass `-D warnings`. At baseline `75c4d51ecd195193ba6a91d210736c0be4e4676f`, the prescribed command exited 0 with pre-existing warnings; nine affected files matched baseline bytes. Independent diagnostic review confirmed this is a passing command with warnings, not a warning-free result.

## Follow-up

Reconcile the checklist wording with the intended root/CI policy in a separately scoped documentation change. Preserve explicit warning reporting and attribution; do not suppress warnings or weaken checks. Revalidate current policy before editing. This documentation discrepancy does not authorize unrelated source cleanup or block otherwise passing campaign batches.
