# 5. Run all focused CRD checks through one entry point

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-09-26 |
| Status | pending |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make the existing local CRD check script run the complete relevant set of existing custom-CRD guards so contributors do not need to remember separate commands.

## Request and evidence

The user requested saving each of five CRD optimization proposals as a separate task for later work. This task records a proposal from static inspection, not implementation approval or a newly reproduced defect. Revalidate the cited sources when starting.

cicd/checks/check_crd_coverage.sh currently runs crd_coverage and plugin_crd_condition_coverage. Separate tests cover HTTP/Stream open plugin types and canonical CRD patterns. Full test runs already include those targets; this is a focused-entrypoint completeness improvement.

## Scope and approach

- Inventory existing custom-CRD test targets and determine which belong in the focused script.
- Include plugin_crd_type_coverage, stream_plugin_crd_type_coverage, and crd_pattern_canonical alongside the existing targets.
- Inspect condition_set_schema and other schema tests before inclusion; keep unrelated suites outside the command.
- Update the script description and directly relevant contributor instructions; use explicit targets and fail on any failed check.

## Acceptance and verification

- [ ] One documented command runs all selected existing custom-CRD guards without requiring cargo test --all.
- [ ] HTTP and Stream open-type contracts, nested plugin conditions, structural coverage, and canonical patterns are included.
- [ ] The command propagates test failures with a nonzero exit code and leaves committed generated/config files unchanged.
- [ ] Run the unified command and documentation validation, and record the selected targets and actual result.

## Boundaries

Do not introduce duplicate tests, alter production behavior, expand CI into unrelated test suites, or claim that local schema tests reproduce a live Kubernetes API server.

## Starting points

Read repository AGENTS.md, skills/SKILL.md, skills/00-design-rules.md, and the relevant resource architecture and review knowledge before implementation.

- [cicd/checks/check_crd_coverage.sh](../../cicd/checks/check_crd_coverage.sh)
- [edgion-resources/tests/crd_coverage.rs](../../edgion-resources/tests/crd_coverage.rs)
- [edgion-resources/tests/plugin_crd_condition_coverage.rs](../../edgion-resources/tests/plugin_crd_condition_coverage.rs)
- [edgion-resources/tests/plugin_crd_type_coverage.rs](../../edgion-resources/tests/plugin_crd_type_coverage.rs)
- [edgion-resources/tests/stream_plugin_crd_type_coverage.rs](../../edgion-resources/tests/stream_plugin_crd_type_coverage.rs)
- [edgion-resources/tests/crd_pattern_canonical.rs](../../edgion-resources/tests/crd_pattern_canonical.rs)
- [edgion-resources/tests/condition_set_schema.rs](../../edgion-resources/tests/condition_set_schema.rs)

## Priority and dependencies

Recommended first: smallest scope and immediate developer-workflow benefit. No dependency on the other tasks.

Recording this task does not start implementation. No production changes or tests were performed when creating it.
