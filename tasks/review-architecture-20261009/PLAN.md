# Edgion architecture and maintainability review

## Objective and authority

Review every repository-owned `src` file with its relevant callers, consumers,
configuration, lifecycle, tests and architecture. Record each distinct actionable
optimization in a separate file. The user explicitly authorized a dedicated result
folder, contextual subagent review of the entire source tree, and creation of a
better-fitting skill when the generic workflow is unsuitable.

Production source, existing tests/docs/knowledge, manifests, configuration and Git
state are read-only. Only the coordinator writes this result directory. All
reviewers are return-only; no builds, generators, formatters, commits or pushes.
The separately authorized personal skill is located at
`/Users/caohao/.codex/skills/edgion-architecture-review/SKILL.md`.

## Workflow selection

`holistic-code-review` supplies useful coverage and independent-verification ideas,
but excludes refactoring preferences and requires an unavailable Terra model.
This review instead admits independently verified structural debt with concrete
maintenance cost even when behavior is correct. Size/style alone remain insufficient.
The new skill uses available inherited-model agents and preserves project gates.

## Baseline and scope

- Repository: `/Volumes/ExtStore/ws6/Edgion`.
- Initial HEAD: `a85aed2fbb73df274dea2932eef8b1be2f9152a2`.
- Initial baseline: dirty worktree; exact hashes/status in `BASELINE.json`.
- Current audited HEAD: `25cc8c34d1edf980399623e951e03ff0b3f069f1`;
  preceding C reconciliation is `BASELINE-AUDIT-20261011-C.json`: 21 additional
  changed harness sources plus changed fixtures/scripts invalidate affected evidence.
  The preceding B audit recorded 84 changed sources and five new resource-reference sources.
  `STATE.json` preserves drift, invalidation and revalidation history.
- Initial primary scope: 1669 source files across all 10 workspace crates;
  current scope is 1676. Historic reads are preserved separately from current coverage.
- Existing WAF policy/source/test/doc edits and untracked task material are retained.
- No nested `AGENTS.md` was found at initialization; recheck when expanding scope.
- Tests, manifests, CRDs, human docs and knowledge pages are secondary evidence.
- No source roots outside the ten workspace crates were discovered initially.
- Nested repository-owned source fixtures/oracles also remain primary scope:
  `edgion-tests/wasm/header-demo/src/lib.rs` and
  `edgion-waf/tests/oracles/aho-corasick-1.1.5/src/main.rs`.
- Dormant Gateway rustls gaps, MCP/A2A products and ACME DNS-01 are excluded topics;
  their source still receives supported-boundary review where applicable.
- External systems and sibling repositories are not review targets. No network
  actions are needed. Defensive analysis within this repository is authorized.

## Ownership and execution order

The runtime-independent algo/WAF/Wasm/DSL leaves feed resources and shared common
infrastructure; Gateway, Controller and CLI consume that DAG. The harness is also
in scope despite exclusion from default members. Read actual manifests and contracts
when testing an edge; do not treat a diagram as proof.

Start with Gateway session capabilities, then adapters/context, plugin composition,
request/response lifecycle, AI ownership, body/SSE, routing/backends/TLS, observability,
configuration/sync/stores and remaining entry points. Finish Gateway primary coverage
before advancing through resources, common, controller, CLI, algo, DSL, WAF, Wasm
and the integration harness. Cross-crate context reads do not claim primary coverage.

`STATE.json` enumerates every primary file and assigns exactly one partition.
Small coherent subtrees are grouped; large modules are bounded by source size and
file count. Very large individual files keep one owner. Numeric grouping is an
initial scheduling aid: analysts may request a coherent split before proceeding.
No automatic split or keyword scan counts as review completion. Unstarted units
were refined to recursively grouped functional subtrees after initialization;
active assignments and completed coverage were preserved.

## Contextual review lenses

| Lens | Search seeds | Required context |
| --- | --- | --- |
| Feature leakage | PluginSession, WafPhase, AiAttemptPlan, install_, claim_, ctx | Caller stages, adapters, state ownership, mock seams |
| Lifecycle ownership | take_, restore_, terminal, abandon, retry, cleanup | State machine, EOF/trailers, cancellation, final commit |
| Canonical mechanisms | Client::builder, cache, Condition, parse_duration, template | Canonical owner, exemptions, actual consumers |
| Publication and validation | preparse, validate, compile, ArcSwap, partial_update | Resource ingest, transport, runtime generation, hot reload |
| Scattered policy | failure, status, framing, admission, capability | Every producer/consumer, bounds, allowed failure semantics |
| Change amplification | repeated match, conversions, parallel lists, resource kind | Concrete change scenario, exhaustive dispatch benefits |
| State shape | Option, booleans, owner tokens, shared mutable state | Reachable transitions, authority and invariant enforcement |
| Dependency direction | use crate, pub use, leaf imports, facades | Manifest edges, runtime-independent boundaries, consumers |

Read implementations beyond matches; identify legitimate complexity and alternatives.
Do not report exhaustive central dispatch, deliberate JWT/OIDC splits, inline worker
status IO, or a sanctioned body owner as debt without new scoped evidence.

## Knowledge and hard gates

Each technical agent independently starts at `AGENTS.md` and `skills/SKILL.md`,
then loads original architecture, `skills/04-review/SKILL.md`, routing rules and
owning topic pages. Required gates: graded design rules; binary-owned logging;
gateway traffic observability through canonical access log; redaction;
observability restraint; cross-cutting unification; data-plane performance above
mock convenience; supported BoringSSL; canonical request BodyStorage and bounded
streaming responses; schema/transport invariants; controller workqueue discipline
where relevant; English-only artifacts. Test bodies/fixtures are read, not changed.

Prior review-batch directories were not found. Existing owning knowledge and
`tasks/pending/`/`tasks/todo/` records remain read-only evidence and duplicate checks.

## Analysis, independent review and persistence

One coordinator plus at most three read-only agents: normally two contextual
analysts and one independent verifier. Default inherited models; no hidden model
substitution. Analysts provide per-file dispositions and terminal counts. Verifiers
actively challenge structural cost, alternative feasibility and preserved invariants.
Every published optimization is tied to its approved candidate hash and evidence
dependencies. The seed concern is saved immediately as pending verification.

Only the coordinator serially updates results. Checkpoint each completed partition
and decision. `COVERAGE.md` and `INDEX.md` derive from authoritative state. Preserve
full scope and continue pending partitions across turns. Changed evidence invalidates
affected coverage and findings; unrelated drift is recorded without erasing valid work.

## Completion proof

Fresh source inventory matches recorded scope; all primary files actually reviewed
once; context and important seams examined; every candidate has independent disposition;
confirmed recommendations deduplicated and tied to current evidence; results and counts
consistent; no run-attributable changes outside the authorized result directory (except
the separately authorized skill creation). No claim of complete review while any file,
seam or candidate is pending. No tests or runtime fixes are claimed by source-only review.

## 2026-10-11 evidence revalidation

Continue the current harness anchor and its affected support. Then revisit changed
earlier anchors serially according to STATE, without claiming their historic
completion as current. New resource-reference files have exactly one owner, PART-607.
Independent approvals depending on changed evidence return to candidate status until
revalidated. DRIFT-DEPENDENCIES-20261011-B.json records the affected dependencies;
STATE-PRE-20261011-B.json preserves all preceding reads and approvals.

## Current-C checkpoint

The C audit preserves historic reads separately from current evidence.
DRIFT-DEPENDENCIES-20261011-C.json records 54 affected partitions and 25
affected candidates; STATE-PRE-20261011-C.json preserves the prior checkpoint.
The seed capability finding is independently revalidated against current publication
and driver seams. ARCH345 is resolved by the current response-body-probe fixture;
this is static evidence, not an executed Kubernetes test. PART457, PART459,
PART460 and PART461 terminal reports are saved with checked current hashes.
ARCH347 is independently confirmed; PART460 supplements existing ARCH280 pending
independent review. Active assignments and exact coverage remain in STATE.json.

## Concurrent WAF capability extraction

BASELINE-AUDIT-20261011-D.json records the same HEAD with four newly changed
Gateway sources and one new traits/waf_session.rs source (1675 current primary files).
DRIFT-DEPENDENCIES-20261011-D.json invalidates only recorded affected effective
dependencies, preserving all prior reads and invalidations. The new file has one
primary owner, GW-WAF-CAPABILITY, pending the Gateway revalidation anchor.
The historical seed approval is suspended because the WAF operation surface
has changed again. Current analysis stays in the harness; seed verification
adds no Gateway primary coverage. SCOPED-DRIFT-WAF-CAPABILITY-20261011-D.json
records the coordinator actual diff/dye scope read. Completion remains pending.

## Current-E reconciliation and continuation

BASELINE-AUDIT-20261011-E.json records the WAF commit and concurrent AI capability
extraction: twelve changed Gateway sources and one new traits/ai_session.rs
source. DRIFT-DEPENDENCIES-20261011-E.json suspends affected effective dependencies
without erasing historic full reads. GW-AI-CAPABILITY is the unique pending owner
of the new file; primary review stays in the harness before revisiting Gateway.
The residual-AI seed revision became stale before coordinator promotion and is
archived in VERIFICATION-ARCH-001-AI-RESIDUAL-PRE-20261011-E.json. Current seed
capability source/task ownership is being independently revalidated again.
ARCH348 includes only its approved original and PART464 supplement; later
ForwardAuth/ExtProc evidence remains pending independent extension review.
Completed report persistence and active assignments are recorded in STATE.json;
INDEX/COVERAGE count current evidence only. No implementation/check execution
or Git mutation is part of this review.

## Task-only F reconciliation

BASELINE-AUDIT-20261011-F.json retains 1676 sources with no source changes versus E. Only existing AI implementation acceptance records changed; their source review is approved and overall acceptance remained blocked in the personally read result receipt. RULE-NAVIGATION-DIFF-20261011-F.txt binds the exact original-rule/index diff. Sixty-eight unchanged-primary reports receive bounded rule/navigation dependency receipts, with zero new primary reading credit and no substantive independent claim rewrite. Current primary coverage and pending candidate/extension work remain authoritative in STATE.json.
