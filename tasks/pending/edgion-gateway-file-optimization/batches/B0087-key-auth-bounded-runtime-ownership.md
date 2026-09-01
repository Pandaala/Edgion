# B0087 — KeyAuth Bounded Runtime Ownership

## Scope
- Baseline: `520a38ee`; both owned blobs were unchanged from the queued analysis
- Pipeline state: independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/key_auth/{mod.rs,plugin.rs}`
- Exact implementation file: `plugin.rs`, including colocated tests
- Forbidden files: resources, Controller, PluginRuntime, session and shared auth helpers, documentation, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- KeyAuth retained the full validated resource configuration and a second Arc around its hashed key map even though the compiled plugin generation already owns the request filter.
- Each successful request cloned the selected source, anonymous requests rescanned a generation-constant header allowlist, and fixed denial bodies were formatted dynamically.
- Decision: retain only the exact immutable request policy and hashed credential metadata needed by the generation while preserving every authentication and hot-reload boundary.

## Hard-rule boundary
- Preserve configured source order, supported source kinds, first-nonempty selection with no fallback after an invalid value, missing-only anonymous access, shared auth delay, exact generic 401 wire response, static denial logs, sensitive username masking, header sanitization, and metadata-before-hide mutation order.
- Raw API keys remain transient request inputs used only for SHA-256 lookup; do not log raw keys, digests, Secret contents, or claim zeroization.
- Add no failure-policy knob, tracing, metric, shared API, response buffering, or integration-test edit.

## Implementation and review
- Complexity: simple; no plan-first review required
- Analyst: `b0055_runtime_analysis`; implementer: `b0084_plan_review`; final reviewer: `b0084_final_code_review`
- Exact change: replaced the dynamic name and retained `KeyAuthConfig` with exact generation-owned fields, consumed resolved keys into a direct hashed map, borrowed the selected source through the synchronous success path, precomputed anonymous username forwarding, reused static denial bodies, and unified production and test construction
- Regression coverage: static name identity, caller-config immutability, hashed K1/K2 generation isolation, source priority and no fallback, anonymous policy, success header/hide/log order, exact missing and invalid responses, and invalid-key rejection with anonymous configured
- Review correction: moved the sensitive-mode assertion into an exact subprocess so the test cannot race the process-global masking mode; independent re-review approved the isolation and final production diff
- Deferred adjudication: D0036–D0039 already own the remaining cross-auth credential lifetime, identity-header, integration, and challenge-realm boundaries. No new deferred or integration issue is required.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 13 Gateway KeyAuth, 19 resource KeyAuth, and 3 Controller KeyAuth resolver tests |
| Independent review | passed | production diff approved after exact-subprocess test isolation correction |
| Full workspace gates | passed | format check, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, gateway no-tracing, diff, and frozen paths |
| Frozen integration paths | passed | 20 existing KeyAuth scenarios remain unchanged; no new integration issue needed |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: pending
