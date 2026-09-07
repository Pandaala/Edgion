---
name: ai-tests-must-discriminate-the-claimed-boundary
description: Use when reviewing AI gateway tests for weighted selection, quota downgrade, credential replacement, fixed errors, cooldown bounds, usage redaction, or bounded ledgers.
knowledge_type: regression-guards
---

# AI tests must discriminate the claimed boundary

A test should fail when the behavior named in its title is removed or replaced with the nearest
plausible incorrect implementation. Assertions derived entirely from fixture construction, a
matching store/load pair, or static strings that never crossed the tested boundary do not provide
that signal.

The AI gateway regression suite uses these patterns:

- Send a client credential before asserting that provider credentials replaced it.
- Lock every fixed error variant to its exact status, type, code, and envelope shape.
- Describe unreachable defense-in-depth limits honestly; separately test the reachable field and
  entry bounds that keep the defensive limit dormant.
- Choose weighted-selection entropy that distinguishes weighted tickets from both first-eligible
  and uniform-modulo selection.
- Exercise legacy-only, current-only, defaulted, and conflicting request fields with different
  values.
- Test saturation through an accumulating operation, not a store followed by its matching load.
- Test sensitive-data exclusion at the final serialized access-log boundary, including nested
  attempts and exact key sets.
- Assert both the lower and upper cooldown clamps and the resulting admission state.
- For one-shot quota downgrade, exhaust both the requested and downgrade model buckets, then require
  one terminal 429 with no second downgrade disclosure.

Integration quota tests must wait for the preceding request's access-log reconciliation before
asserting the next bucket admission. A long fixed window and a unique identity keep the sequence
isolated and deterministic.

## Resolution references

- `test-10` — 2026-08-10.
