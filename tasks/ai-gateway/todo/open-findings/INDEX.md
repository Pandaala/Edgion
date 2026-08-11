# Review batch: AI Gateway work

Batch ID: `review-ai-work`  
Scope: the full AI Gateway change set — 5 commits soft-reset into the index, 238 files / ~31k insertions, diffed against `a2c7119e`  
Date: 2026-08-04 (second batch appended to the first-pass findings)  
Status: **complete** (first pass + second-pass deep dig + unified 7-dimension re-review)

## Coverage ledger

| Unit | Status | Pass |
|------|--------|------|
| Gateway AI runtime | first pass: no findings | 1 |
| Security | no medium+ findings | 1 |
| Broad defect scan | no bugs | 1 |
| Resources + CRD + controller | first pass: no findings | 1 |
| Docs + tests accuracy | 3 findings (fixed) | 1 |
| Controller / schema deep dig | 1 finding | 2 |
| AI dispatch deep dig | 2 findings | 2 |
| Tests false-green dig | 1 finding | 2 |
| AI runtime core (dispatch, routing, attempt, quota, circuit breaker) | 3 findings | 3 |
| Streaming + Anthropic/OpenAI translation + response-body path | 6 findings | 3 |
| Controller + resource schema + CRD | 5 findings | 3 |
| Security, secrets, cross-cutting unification | 4 findings (1 P0) | 3 |
| Observability + documentation accuracy | 7 findings | 3 |
| Tests — false greens and coverage gaps | ~30 findings, grouped into 9 entries | 3 |
| Regression risk in shared gateway paths | 6 findings | 3 |

Two pass-3 findings were put through an independent refutation pass: the P0 (`gw-03`) was **confirmed** on all seven evidence links; the quota finding (`gw-04`) was **confirmed but downgraded** with two impact claims refuted.

## Findings index

### P0

| ID | Dimension | Title | Status |
|----|-----------|-------|--------|

### P1

| ID | Dimension | Title | Status |
|----|-----------|-------|--------|
| ctrl-01 | Correctness / status | AI rule invalidity flips Accepted and PartiallyInvalid contradictorily | closed — [skills/04-review/routing/ai-rule-accepted-escalates-only-when-nothing-serves.md](../../skills/04-review/routing/ai-rule-accepted-escalates-only-when-nothing-serves.md) |
| `gw-01` | Correctness | CostWeighted ignores `max_completion_tokens` | closed 2026-08-04, file removed (shared `request_output_bound` parser; rule in `skills/04-review/routing/ai-cost-weighted-request-output-bound-parsing.md`) |

### P2

| ID | Dimension | Title | Status |
|----|-----------|-------|--------|
| [gw-16](./gw-16-bodyless-upstream-response-emits-no-envelope.md) | Correctness | A bodyless upstream response yields an empty body and no error envelope | open |

### P3

| ID | Dimension | Title | Status |
|----|-----------|-------|--------|

### Fixed in an earlier pass

| ID | Title | Status |
|----|-------|--------|
| `docs-01` | zh-CN metrics inventory broken link | closed 2026-08-04, file removed (fix verified on disk; residual link error → `docs-07` item 4) |
| [docs-02](./docs-02-zh-metrics-genai-series-drift.md) | zh-CN metrics GenAI series drift | fixed |
| [docs-03](./docs-03-zh-access-log-missing-genai.md) | zh-CN access-log missing genai | fixed |

One issue per `<area>-NN-<slug>.md`. `INDEX.md` is batch metadata only. Where a single theme produced many small instances, the file carries a table of instances rather than splitting into one file per line — those are marked as grouped in the title.

## Suggested order of attack

1. Everything else by severity.

## Residual notes (not findings)

- Anthropic suite order depends on 429 credential cooldown (fragile if reordered).
- Thinner coverage: unclassified proxy `ErrorType`, 429 `Retry-After` e2e consistency, credential-pool reconcile × outlier cross-module.
- `RedisQuotaSink::admit_all`/`apply_all` result folding is reachable only through a live `RedisLinkClient`; a unit test needs the folding loop extracted into a pure function first.
- Anything reached only through `prepare_first_attempt` / `commit_ai_attempt_context` needs a live `pingora_proxy::Session` and has no unit coverage: `apply_backend_request_headers`, `provider_uri` wiring, header stripping, `effective_max_output_tokens`.
- Verified clean and deliberately not filed: provider-upstream SSRF (egress classification is applied at dial time), credential material in `Debug`/`Serialize`/error bodies, Redis key construction and Lua argument validation, all documented byte caps, the `EdgionAIService` CRD↔struct field correspondence, RBAC in both manifests, workqueue discipline, and delete→create coalescing in the new handlers.

## How this batch was produced

Seven parallel review agents, one per dimension, each given the graded design rules, the relevant frozen design docs, and the list of already-known findings to avoid re-reporting. Findings that made a strong claim were then put through an independent agent instructed to refute them. The controller P1 was confirmed by running the single test it predicted would fail. No `cargo build`/`clippy`/integration run was performed; every other claim is from reading, and the coverage sections of each pass record what could not be verified.
