# Review decisions

## Design review

Independent reviewer `design_review` approved the plan after reading project rules,
original knowledge and implementation. No blocking design defect found.

- Keep unit sentinels rather than retaining unknown type/config text; actionable
  diagnostics identify `spec.type` or `spec.data.type` without echoing input.
- Preserve lenient transport so invalid replacements remove stale state.
- Reuse existing provider wrong-kind retirement and ConfigData atomic index removal.
- Require full/partial transitions, credential-source cleanup, strict WAF/trust,
  overlay fallback, mixed list/watch and known malformed config regressions.
- Update all obsolete CRD parity expectations. Static guards are not live admission
  tests; record that distinction in final verification.

Implementation starts after this approval. Code review results follow separately.

## Code review round 1: integration and documentation

Reviewer found a blocking Kubernetes test-path error: LinkSys status is partitioned
in `status.controllers[]`, while the new assertion read native `status.conditions`.
RegionRoute's shared helper had the same issue for its EdgionPlugins consumers.

Corrected both helpers to prefer the canonical `spec.currentStatus.conditions`
projection, with native status fallback and generation validation. Foreign
controller envelopes cannot satisfy assertions. Added focused helper regressions.
Reviewer found no other blocker in the dynamic assertions or documentation.
Full/partial runtime and Controller tests remain subject to the production review.

## Code review round 1: production and lifecycle

Independent reviewer `production_review` found no production defect. It verified
variant-local decoding, known auth strictness, desired/result clearing and manager
fences, provider retirement, both ConfigData indexes, dependency requeues and strict
consumer behavior. Existing credential-manager memory-retirement tests complement
the new Controller source-replacement test.

One test expectation was wrong: an unsupported HTTPDNS target returns the System
resolver, not the next lower-priority configured DNS server. Corrected the test
name and assertion to preserve the documented production behavior. Two compile
nits in new tests (missing map imports and comparison requiring Debug) were also
corrected before final checks. The subsequent full diff review is recorded below.

## Code review round 2: complete task diff

Reviewer `design_review` re-read the complete production, unit/dynamic tests, CRD,
guard/workflow and documentation diff after corrections. No new Must Fix or
Should Fix. It verified both earlier fixes, variant-local secret discard, preserved
known credential decoding, full/partial retirement/index cleanup, Controller
dependency and credential-source cleanup, and distinct strict/fallback consumers.
This is static review approval; fresh build/test execution remains a separate gate.

## Supporting CI correction

Fresh execution exposed a pre-existing failure in the unchanged overlay-signal
guard: RegionRoute computes a match-result `signal` then records it once, while
the script recognized only direct string arguments. The four signal expressions
were present in the unchanged production code.

Updated only the guard to recognize an immediately consumed match result at the
same indentation, counting standalone signal expressions within that block. Kept
direct calls supported. Verified actual sources pass and seven mutations fail:
each missing signal, removed recorder, signal outside the match, and log strings
without signal expressions. Independent `production_review` approved this focused
correction. It remains a source-structure guard, not a runtime execution proof.

## End-to-end correction: required config envelope

The native LinkSys dynamic run reached the real Admin update path and failed with
HTTP 400: serializing the unit sentinel omitted the CRD-required `config` field.
Earlier static review and transport round-trips did not cover normalized schema
validation. The design reviewer approved a narrow correction: serialize only these
two resource sentinels with an empty `config: {}`, leaving plugin wire formats and
known payload validation unchanged.

Added exact wire-shape assertions and Controller create/update regressions using
the committed CRDs, including a negative missing-config assertion. Updated dynamic
checks and operator recovery instructions. Fresh execution and independent review
of this correction were required before completion; outcomes follow below.

### Serializer implementation adjustment

The variant-local serde hook compiled in isolation but conflicted with
`schemars(skip)` in the real resource derive. Independent inspection of schemars
1.2.1 confirmed it synthesizes serde skip attributes and does not accept the
`!serialize_with` override. The design reviewer approved explicit exhaustive
`Serialize` implementations for the two resource enums, using one borrowed
adjacent-envelope helper. Known configs delegate directly to their existing
serializers so redaction remains intact; no intermediate JSON value is allocated.
Known tag/config parity and round-trip tests are required for every variant.

### Final correction review

Independent `production_review` approved the exhaustive serializers after actual
resource compilation passed. All 17 known variants retain tags and concrete config
serialization; schema-tag parity and WAF redaction are covered. Resource regression
target passed 4/4. The final dynamic assertions check the empty config at both
Controller and Gateway. No blocking review finding remains. Subsequent API, workspace and dynamic
execution gates all passed, as recorded in `02-implementation.md`.

## Final acceptance

Final format/check/clippy and repository guards passed. Contract tests passed
29/29; the unsupported lifecycle filter passed 76/76 (including the new Admin API
regression). Dependency requeue and status projection tests passed 5/5 and 2/2.
After canonical artifact rebuilding, `LinkSysProviderReplacement` and
`RegionRouteValidation` both passed against the native shared topology.
All actionable findings from design, two full code review rounds and correction
reviews are resolved. Live Kubernetes admission remains an unexecuted verification
boundary; this is not represented as an API-server admission test.
