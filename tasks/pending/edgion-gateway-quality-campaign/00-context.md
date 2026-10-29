# Context and recovery

Read the original specification and user attachment before resuming. Attachment: `/Users/caohao/.codex/attachments/e58c8250-9eb8-486d-abd2-d9d05bc02564/pasted-text-1.txt`.

Mandatory entry points: repository `AGENTS.md`, applicable nested `AGENTS.md`, `skills/SKILL.md`, `skills/00-design-rules.md`, `skills/07-tasks/SKILL.md`, and `skills/04-review/knowledge-routing.md`. Every technical agent reads originals independently and progressively selects relevant architecture, coding, review and testing leaves. Explicit gates include binary-owned logging, canonical access-log ownership, redaction, observability restraint, cross-cutting unification, body/resource bounds, supported feature combinations, test preservation and English-only disk content.

Workflow: `/Users/caohao/.codex/skills/edgion-gateway-quality-campaign/SKILL.md` and its artifact, batch and subagent contracts. User scope takes precedence over its default integration freeze and includes all ten crates and automation. Do not switch to individual finding-fix workflows.

No semantic completion is inferred from inventory, searches, lint or compilation. Store exact test identity mappings for every removal/merge. Evidence-insufficient code remains, with a revisit condition. No unsupported Gateway rustls implementation, behavior redesign, feature removal or unrelated refactoring.

## Bootstrap verification

At baseline `75c4d51ecd195193ba6a91d210736c0be4e4676f`, `cargo check --workspace --all-targets` passed (2m04s); `cargo fmt --all` passed without tracked changes. Agent-doc validation, SSA-force, metrics inventory (102 metrics), Pingora source policy, Gateway API transform contract and gateway no-tracing guards passed. Workspace Clippy and unit-test layout guard are still running. These establish check baselines, not semantic file coverage or behavior equivalence.

Coverage planning selected: `skills/01-architecture/02-gateway/SKILL.md`, `skills/05-testing/00-unit-testing.md`, `skills/04-review/SKILL.md`, `skills/04-review/knowledge-routing.md`. Build batch additionally selects relevant `skills/09-misc/cicd/{00-local-build,01-docker-build,02-github-workflow}.md` and TLS backend policy. Unit-test layout baseline passed (1,113 mounted files, 9 production crates). Clippy has emitted pre-existing WAF/controller unit-test warnings; its final outcome is pending.

Workspace Clippy exited 0 (1m51s) with existing warnings in WAF prefilter tests, Controller TLS/identity-sync tests, integration harness tests/fixtures, and Gateway outbound call tests. This is not a warning-free result. A read-only diagnostic agent is resolving the manifest's advisory-lint statement against the task checklist's warning-free wording before a commit gate. Full authorized `cargo test --all` baseline is running (session 4176; output `/private/tmp/edgion-pruning-baseline-tests.log`).

Diagnostic `/root/baseline_diagnostic` verified all nine warning-bearing files match baseline bytes. Root `AGENTS.md:265`, `Cargo.toml:31-32` and CI's exact command establish an advisory-warning policy; baseline Clippy is pass-with-existing-warnings, not warning-free. `skills/07-tasks/SKILL.md:190` has conflicting checklist wording, recorded for separate correction. Safe continuation approved without warning suppression or unrelated crate edits. The diagnostic's redundant lock-waiting Clippy was cancelled (130); it is not a replacement result and did not interrupt full tests.
