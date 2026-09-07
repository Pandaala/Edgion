---
name: integration-readme-cli-must-match-runner
description: Use when reviewing integration README commands, runner flags, or prepare artifact documentation.
knowledge_type: regression-guards
---

# Integration README CLI must match the runner

## Conclusion

Published integration commands must use options accepted by the current runner. Preparation documentation
must list the artifacts the build phase actually produces and must not imply that runtime compilation fills
missing artifacts.

## Core rationale

The detailed workflow remains canonical in `skills/05-testing/01-integration-testing.md`. The scripts README
is a short entrypoint whose examples use `--resource`/`--item`, `--no-prepare`, `--full-test`, and `--suites`
from `run_integration.sh --help`. A capability regression test rejects known removed options across the
integration entrypoint documents and pins the prepared artifact inventory.

## Fix suggestions not accepted

- Documenting a desired flag before implementing its parser and semantics.
- Keeping obsolete examples as aliases when the pre-release project has no compatibility requirement.
- Saying “all binaries” without an exact artifact list at the build/runtime boundary.
- Duplicating the full testing guide in every README.

## Re-evaluation triggers

When the runner adds or removes an option, update its help text, directly affected README examples, and the
documentation regression guard in the same change. When prepare changes its artifact set, update the exact
inventory and the build-boundary tests together.

## Reference cases

- `edgion-tests/integration/scripts/README.md`
- `edgion-tests/integration/scripts/integration/run_integration.sh`
- `edgion-tests/integration/scripts/utils/prepare.sh`

## Resolution references

- `ITEST-013` — 2026-08-26.
