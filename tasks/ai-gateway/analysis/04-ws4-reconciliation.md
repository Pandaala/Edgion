# ws4 Workspace Reconciliation — Status, Code Delta, Absorption Record

| Key | Value |
|-----|-------|
| Created | 2026-08-09 |
| Status | RESOLVED 2026-08-09 — merged in this workspace via `git merge origin/feature-06-24` (merge commit `2b6e5b23`, pushed). The ws2 cherry-pick line is abandoned; §3/§3b are kept as the conflict recipe for future reference. |
| Sources | ws4 = /Volumes/ExtStore/ws4/Edgion (branch `feature-06-24`, diverged); two deep-read passes (docs + commit-by-commit code diff) |

> **Outcome**: this workspace (formerly "ws4") absorbed the remote side and is
> now the single main line; the ws2 workspace is retired. Everything below is the
> analysis that produced that decision.

ws4 is the **origin workspace of the AI-gateway program** (the AI-0..AI-7 roadmap
was authored there). Both workspaces share ancestor `d88d573f`; ws2 adds the
sink-terminate feature (`b0c9db66`, `2bd656f9`) and the tasks reorg; ws4 adds
review-fix commits + uncommitted work that ws2 lacks. Neither side contains the
other's changes.

> **⚠️ ws4 IS A MOVING TARGET.** At 2026-08-09 ~13:00 (while this doc was being
> written) ws4 advanced past the analyzed snapshot (`19ecc739`) with at least
> three more commits — `43b81d27` "defer outlier ejection arbitration" (the
> gw-07/gw-10 line, so gw-07 is now committed there), `fd4be141` docs-05 fix,
> `b9405d3f` test-01 fix — and its working tree carries further in-flight
> quota/attempt_runtime changes. An active session is closing
> `review-ai-work` findings there (finding files are deleted as fixed). The
> commit table in §3 and the copied `todo/open-findings/` + resilience-tracker
> snapshots in ws2 reflect the `19ecc739` state. **At WS-00 merge time:
> (1) confirm the ws4 session has finished; (2) re-enumerate the range as
> `d88d573f..<ws4 HEAD>`; (3) re-copy `todo/open-findings/` and the resilience
> tracker; (4) the gw-07 "commit in-flight work first" step may already be
> done.**

## 1. Program status reconciliation (ws4 AI-0..AI-7 → ws2)

| ws4 item | ws2 counterpart | Status |
|---|---|---|
| AI-0 streaming foundation | shipped code (+ WS-06 decoder v2 planned) | **Implemented** — ws2 even goes beyond (mid-stream terminate). Design authority: `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md` (now copied into ws2, fixing the dangling reference at `streaming_response_processor.rs:4`) |
| AI-1a single-provider passthrough | shipped code | **Implemented** (`ai_proxy` + `EdgionAIService`) |
| AI-1b schema translation | shipped (Anthropic) + WS-02 (Gemini etc.) | **Implemented for Anthropic**; breadth pending |
| AI-2 metering & cost | shipped code + WS-07 tail | **Implemented** (quota, ledger, mid-stream termination) |
| AI-3 resilience & routing | shipped code | **Implemented and verified in ws4** (G-phase: crd 10/10, ai:: unit 139/139, integration green — see `archive/completed-tasks/ai-gateway-resilience-and-routing/07-changelog.md`, now carried over). ws2's stale checkboxes replaced by ws4's live tracker |
| AI-4 guardrails | WS-03 | Planned (ws4 unique design sections absorbed — §4) |
| AI-5 semantic layer | none (out of scope except tokenizer item) | Decision memo kept: `analysis/05-semantic-layer-memo.md` |
| AI-6 MCP & agent | WS-01 + WS-04 | Planned; ws4 hardening specifics absorbed (§4); authz sub-plans parked per closed decision 2 |
| AI-7 inference-aware routing | none | Decision memo kept: `analysis/06-inference-routing-memo.md` (verdict: default don't; GIE shrank, revisit on real self-hosted demand) |

## 2. What was carried from ws4 into ws2 (this change)

| Carried | To | Why |
|---|---|---|
| `tasks/todo/ai-gateway/` (whole tree, verbatim incl. historical Chinese HANDOFF) | `tasks/ai-gateway/archive/program-origin/` | Lossless archive of the origin program docs; unique rationale prose (Pingora-vs-Envoy boundary, bounded-state rules, dependency-unlock decisions) |
| `COMPETITIVE-ABSORPTION.md` | `analysis/03-competitive-absorption.md` | Highest-value single file: ~15-product research (LiteLLM/Portkey/OpenRouter/Cloudflare/APISIX/archgw/APIM/Bedrock/Helicone/TrueFoundry/agentgateway/Higress/ContextForge/AgentCore/GIE), absorbed-item decisions with weights, explicit rejections (anti-relitigation), two Pingora-form architecture warnings |
| `tasks/ai-gateway/todo/open-findings/` (20 files) | `tasks/ai-gateway/todo/open-findings/` | **Active state**: completed 3-pass review of the AI change set with 17 still-open P2/P3 findings (see §5) |
| resilience task live tracker (5 updated docs + `HANDOFF.md` + `07-changelog.md`) | `archive/completed-tasks/ai-gateway-resilience-and-routing/` | The only record of AI-3's actual progress/verification; ws2 copies were the stale pre-verification snapshot |
| `response-body-buffer-runtime.md` | `tasks/ai-gateway/todo/` | Still-active design note for the bounded *non-streaming* buffered-response mode (`read_response_body` is still `Unimplemented` in ws2, session_adapter.rs:1175); its invariant-#2 amendment plan must be reconciled with our decision-3 wording |
| phase-1 spec + AI-0/AI-1a plan files | `docs/superpowers/specs|plans/` | Cited by shipped code; decision rationale record |

Deliberately **not** copied now (will arrive with the code merge, §3): doc edits
riding ws4 commits (anthropic-translation `01-design.md`/`04-wire-contract.md`
pause_turn + tool_choice:none updates; metering `01-decisions.md` monotonic
high-water-mark decision; `skills/04-review/CLOSED-FINDINGS.md` rows and
per-finding skill files incl. `provider-cooldown-survives-concurrent-success.md`).
Copying them without the code would make docs describe behavior ws2 doesn't have.

## 3. Code delta — ws4 commits ws2 LACKS (merge decision needed)

Real bug fixes, several user-visible; recommended strategy: **ws2 stays
canonical, cherry-pick the ws4 range in original order** after committing ws4's
in-flight gw-07 work there. Expected manual conflicts: `ai/usage.rs` (worst:
ws2 terminate rewrite × ws4 `publish_reported_usage`→`observe_reported_usage`
monotonic ArcSwap — semantically orthogonal, both must survive; ws2 has 7 call
sites to update), `ai/anthropic/response.rs` (both added different tests +
1-line pause_turn fix), `ai/dispatch.rs` (mechanical).

| sha | what | kind |
|---|---|---|
| d60cfc09 | restore canonical Pingora fork source (removes local `[patch]`, pins `edgion#0b6ae51e`) — **defer/adapt**: ws2's b0c9db66 may depend on local fork changes; verify the local pingora checkout is pushed to the fork branch first | build |
| 7b05ba19 | EBTP targetRefs: 16-cap/duplicate rules only for EdgionAIService targets; Service unlimited | fix |
| 6c4d4923 | BackendTLSPolicy → EBTP requeue key mismatch (dependent EBTPs never requeued) | fix |
| 18d94399 | AI admission honors PluginPolicy (Bypass doesn't count; Block preserves rule) | fix |
| 66d36b37 | partial credential-pool failures keep `ResolvedRefs=False` + `PartiallyInvalid` detail | fix |
| e5df850a | `/store-stats` iterates ALL_RESOURCE_INFOS (residue inventory complete) | fix |
| d237ce5f | **security**: enforce ReferenceGrant on AI backends before dispatch/mutation | fix |
| 08a1375d | replace dead spec references in route delegation docs/comments | docs |
| 35430827 | compression moved to `pg_response_filter` with known-length guard — unknown-length streams never compressed (stall fix beyond SSE-only) | fix |
| 803c92c9 | conflicting token bounds → 400 client error (was 5xx) | fix |
| 3fb32aa6 | quota `canonical_model`→`logical_model` rename + bucket-partition docs | docs/rename |
| cb80c02c | Anthropic `tool_choice:"none"` must not carry `disable_parallel_tool_use` | fix |
| 20841766 | Anthropic `pause_turn` stop reason → OpenAI `"stop"` | fix |
| e57a75e7 / 18d7a7c9 / ad615847 / 78ad302a | routing-ordering, failover-boundary, Anthropic-wire, secret-rotation-fixture test hardening | tests |
| a10af498 | **not test-only**: reported-usage publication → monotonic high-water `ArcSwapOption<ReportedAiUsage>` (`observe_reported_usage`) + quota flush-retry tests | fix+tests |
| 76e4fb9b | expose AI outlier/telemetry stores in `/store-stats` (residue check) | fix |
| 19ecc739 | `genai.model` cardinality contract docs | docs |

**ws4 uncommitted (must be committed in ws4 first or hand-ported)**: gw-07 fix —
`backends/outlier_detection/state.rs` splits explicit Retry-After cooldown from
failure ejection so a concurrent success can't cancel a provider cooldown or
steal its half-open probe (+ regression test in `ai/credential_outlier.rs`,
docs, CLOSED-FINDINGS row, decision skill file).

**Reverse direction**: ws4 lacks ws2's entire mid-stream-termination feature and
the tasks reorg; after the cherry-pick, ws2 supersedes ws4 and ws4 should be
retired or fast-forwarded.

## 3b. WS-00 merge rehearsal results (2026-08-09 17:40, ws4 HEAD `2943e225`)

A throwaway git worktree replayed the whole range onto ws2 HEAD `2bd656f9`.
ws4 had advanced to **27 commits** by then; the 7 added after the first
analysis close more review findings: `ed0f58bf` provider cooldowns (= gw-07,
now committed there), `9b02207f` skip failed service endpoints (gw-08),
`43b81d27` defer outlier ejection arbitration (gw-10), `fd4be141` unknown
field handling (docs-05), `b9405d3f` one-shot quota throttle (test-01),
`27bac280` time-boundary stability (test-07), `2943e225` streaming client
disconnect (test-08).

**Verified plan (26 picks, 3 conflicts):**

1. **Skip `d60cfc09` entirely.** Its `mod.rs` half is already in ws2, and its
   Cargo.toml/lock half must NOT be applied: the local fork checkout
   `/Users/caohao/ws2/pingora` is at `0b6ae51` — identical to the commit ws4
   pins and **fully pushed** to `origin/edgion` (verified: `git log
   origin/edgion..HEAD` empty) — but it carries **uncommitted PROXY-protocol
   work**, and removing the `[patch]` block would cut that in-flight fork work
   out of the build. Revisit when the fork work lands.
2. **`7b05ba19` → `skills/04-review/CLOSED-FINDINGS.md`**: append-only review
   ledger; resolve with a union merge (set
   `skills/04-review/CLOSED-FINDINGS.md merge=union` in the repo's
   `.git/info/attributes` — note: per-worktree `info/attributes` is ignored,
   it must be the common git dir).
3. **`a10af498` → hand-merge** (the only real one): `ai/usage.rs`,
   `ai/anthropic/response.rs`, `docs/{en,zh-CN}/ai-gateway/quotas-and-cost.md`.
   ws4 replaces the four token atomics + `reported_usage: AtomicBool` with
   `reported_usage: ArcSwapOption<ReportedAiUsage>` and monotonic
   component high-water marks (`publish_reported_usage` →
   `observe_reported_usage`); ws2 appended `gateway_terminated: AtomicBool` to
   the same struct for the terminate feature. **Resolution**: take ws4's field
   replacement AND keep ws2's `gateway_terminated` field/methods; take ws4's
   `observe_reported_usage` body wholesale; keep ws2's `terminate_*` functions;
   rename all **7** ws2 call sites (`usage.rs:365,917`,
   `attempt_runtime.rs:856,1009`, `anthropic/response.rs:325,576`,
   `pg_logging.rs:768` — the latter two files' non-conflicting halves are
   renamed by the pick itself).
4. **`2943e225` → `edgion-tests/src/server/test_server.rs`** (mechanical: ws4
   reworks the mock SSE endpoint for client-disconnect coverage).
5. Everything else (21 commits) applies cleanly, including the security fix
   `d237ce5f`, the streaming-compression fix `35430827`, both Anthropic
   correctness fixes, and all controller fixes.

**Rehearsal outcome (validated end to end)**: replaying the 26 picks in a
throwaway worktree with the resolutions below builds clean
(`cargo check --workspace --all-targets`) and passes **189/189** `ai::` unit
tests. Exact adaptations needed — ws4's new tests were written against the
pre-terminate API, so they must be ported to ws2's async surface:

| Site | Adaptation |
|---|---|
| `usage.rs` imports | keep both sides (`async_trait` + `arc_swap::ArcSwapOption`) |
| `usage.rs` / `response.rs` test-block conflicts (4 regions) | keep ws4's inserted tests, then restore ws2's `#[tokio::test] async fn` signature for the test that follows |
| ws4 tests calling `run_chunk(...)` | `#[test] fn` → `#[tokio::test] async fn`; add `.await` (3 multi-line call sites need it by hand) |
| ws4 test `streaming_quota_tracks_...monotonically` | its `send` closure must become a `macro_rules! send` with `$(,)?` (a closure cannot await); trailing `run_chunk` needs `.await` |
| `AnthropicResponseProcessor::new_with_quota` | ws2 takes a 7th arg — pass `terminate_on_over_limit: false` in the ws4 test |
| `UsageExtractionProcessor::new_with_quota` | ws2 takes 4 args — pass `(shared, quota, /*requested_streaming*/ true, /*terminate*/ false)` |
| `usage.rs` test `openai_streaming_processor_updates_quota_only_for_semantic_deltas` | ws2's `feed` helper is async — make the test `#[tokio::test] async fn` and `.await` both `feed` calls |
| `docs/{en,zh-CN}/ai-gateway/quotas-and-cost.md` | union: keep ws2's "mid-stream termination" section AND ws4's cumulative-snapshot paragraph |
| `edgion-tests/src/server/test_server.rs` | keep ws2's `force_sse` block (it computes `streaming`) and append ws4's `trace_id` extraction; drop ws4's duplicate `streaming` line |

**Rollback**: ws2 HEAD before the merge is `2bd656f9`; the pre-merge snapshot of
ws2's uncommitted AI docs is backed up outside the repo (session scratchpad).

**Working-tree sequencing** (ws2 has 17 staged renames from the tasks reorg,
which makes `git cherry-pick` refuse): back up `tasks/ai-gateway/`, undo the
reorg (`git reset` + restore old paths), cherry-pick (ws4's doc edits then land
naturally at the old paths), then redo the reorg with `git mv` so the moved
files carry ws4's updates forward, and restore the new-only documents from the
backup.

## 4. Unique ws4 analysis absorbed into roadmap files (this change)

- **WS-01 MCP** ← ws4 AI-6: transport hardening checklist (Origin validation /
  DNS-rebinding, CORS `Access-Control-Expose-Headers: Mcp-Session-Id`,
  `MCP-Protocol-Version` negotiation, `Last-Event-ID` passthrough), tools/list
  hash-locking (rug-pull detection), JSON-RPC-shaped policy rejections,
  session-target-dead fast-fail (no cross-instance retry), aggregation spec
  details (per-target FailClosed default, synthetic initialize for stateless
  members). Parked (closed decision 2): OAuth PRM (RFC 9728), token exchange
  (RFC 8693), identity RBAC — kept as input for the future authorization
  program in `archive/program-origin/06-mcp-and-agent/`.
- **WS-03 guardrails** ← ws4 AI-4: structured refusal (HTTP 200 +
  `finish_reason: content_filter`), single evaluator contract instead of N
  vendor adapters, parallel guard execution + per-guard timeout, PII-aware
  data-residency routing (`block|mask|route-to`, fail-close on unreachable
  target).
- **WS-02 providers** ← ws4 AI-1 §7/§8: capability matrix so failover skips
  incapable candidates (no silent downgrade); non-chat endpoints = translate
  chat+embeddings only, metered passthrough where safe, explicit reject
  otherwise.
- **WS-07 tail** ← ws4 AI-2 §5/§7/§8 (backlog, needs user opt-in): virtual
  keys + org/team/user budget hierarchy, unit-abstract metering for non-chat
  (seconds/images, not hardcoded tokens), session/trace grouping fields.

## 5. Open findings inventory (from `todo/open-findings/INDEX.md`, re-triage against ws2 HEAD needed)

- P2: gw-07 (fix in flight in ws4 — see §3), gw-08 endpoint failure retried per
  credential, gw-10 outlier eject lock O(N) per failure (non-AI traffic too),
  gw-16 bodyless upstream response emits no error envelope, docs-05 CRD prunes
  unknown fields (docs claim reject), test-01 Throttle re-admit untested,
  test-07 wall-clock-flaky tests, test-08 client-disconnect untested, test-09
  adaptive reconcile untested.
- P3: gw-09 predispatch deadline, gw-12 ServiceImport dropped from WRR, gw-13
  telemetry reconcile full walk under lock, gw-15 non-Service backends bypass
  metric handles, gw-18 usage processor re-buffers Anthropic body, gw-19 header
  strip is denylist, gw-20 SSE empty-data event dispatched, gw-22 SSE
  degradation flags unconsumed, docs-04/06/07, test-10 unfalsifiable asserts.
- INDEX also records verified-clean areas (SSRF, secret redaction, Redis keys,
  RBAC, workqueue) — do not re-review those.
- Note: several findings were filed against ws4 HEAD; after the cherry-pick,
  re-check each against merged ws2 (gw-18 in particular interacts with WS-06's
  single-decode plan; gw-20/22 fold into WS-06 scope).

## 6. Contradiction checks vs the five closed decisions

- ws4 AI-6 authz sub-plans (OAuth PRM, token exchange, tool RBAC) would reopen
  decision 2 if absorbed verbatim — parked, not scheduled.
- ws4 `response-body-buffer-runtime.md` plans an invariant-#2 amendment for the
  *non-streaming buffered* mode — different clause than our dissolved decision
  3 (bounded hold-and-release); if that mode is ever scheduled, write one
  combined invariant note, not two competing ones.
- ws4 AI-1/AI-2 Bedrock plans are superseded by decision 4 (popular three) — do
  not resurrect Bedrock lines when reading inherited docs.
- No conflict on stdio (ws4 had already scoped it out) or benchmarking.
