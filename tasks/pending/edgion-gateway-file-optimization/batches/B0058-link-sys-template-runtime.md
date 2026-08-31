# B0058 — LinkSys Template Runtime Audit

## Scope
- Baseline: `e8eb6ff7`
- Pipeline state: reviewed; pending full verification and commit
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/template/engine.rs`; `edgion-gateway/src/link_sys/template/mod.rs`
- Allowed support files: none; this batch is read-only pending wider-task adjudication
- Forbidden files: every existing `edgion-tests/**` path, template consumers, schemas, shared crates, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; B0057 and B0058 batch records; one reviewer-approved D0024 record; `deferred/INDEX.md`; and one canonical `tasks/todo/` task if approved

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `e8eb6ff7`
- Coverage summary: compared both LinkSys template files with the canonical outbound-request evaluator and every live consumer requiring missing-as-empty, missing-as-error, Secret resolution, JSON-variable escaping, restricted-variable policy, or pre-append byte budgets.
- Concrete finding: Gateway currently has two runtime `${type:name}` segment walkers and variable resolvers with incompatible implicit policies. The LinkSys engine itself exposes separate empty/error missing-value evaluators and never resolves Secrets, while outbound request rendering duplicates the walk to add Secrets, JSON-variable escaping, and remote-IP handling.
- Decision candidate: defer both target rows. A local extraction would preserve the competing evaluator and cannot safely reconcile consumer-specific missing, escaping, Secret-capability, or output-bound semantics.
- Existing-task adjudication: `tasks/todo/upstream-authority-bounded-template-rendering.md` owns only the authority 63-byte case and explicitly excludes general migration; no canonical cross-consumer evaluator task was found.

## Proposed wider boundary
- Canonical mechanism: one compiled-segment walking/resolution seam with explicit missing-value, escaping, Secret-capability, and overflow-safe byte-budget policies.
- Minimum runtime files: LinkSys template engine/facade, outbound request rendering/call boundary, and the ctx-set, dye, authority, and proxy-rewrite consumers.
- Conditional schema boundary: if a new operator-configured admission or budget field is required, explicitly include the outbound request and external HTTP resource schemas and canonical docs; otherwise do not invent a cap.
- Required preservation: compile-once/no second-pass behavior, restricted timeout variables, failure policy, metrics, Secret redaction/provenance, exact consumer error classes, and no rendered sensitive values in logs.
- Focused coverage: exact budget edges and escape expansion, missing empty/error, resolved/absent Secret behavior, remote-IP empty behavior, and consumer-specific authority/proxy limits.
- Integration-test impact: keep all existing integration tests frozen. Source-local unit tests can cover the identified semantics; create a separate integration issue only if a future implementation proves a cross-process over-limit scenario cannot be verified locally.

## Independent review
- Reviewer: `b0055_runtime_analysis`
- Initial verdict: both rows correctly defer; task framing required correction before approval
- Must Fix resolution: the canonical task now makes destination/deadline restricted-variable admission explicit, uses per-consumer missing/Secret/escaping/budget capabilities, limits its allocation claim to the output buffer, establishes and carries Secret-derived provenance through sensitive header materialization, preserves every existing failure contract, and absorbs the overlapping authority task.
- Should Fix resolution: the task names the exact documentation contracts and excludes proxy rewrite's separate legacy rewrite language.
- Final verdict: Approved with no residual Must Fix or Should Fix
- Deferred issue: D0024 and its canonical task are approved as real, nonduplicate, and wider than B0058

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Cross-consumer source analysis | passed | two live evaluator families and policy divergence confirmed |
| Independent defer review | passed after two correction reviews | restricted admission and Secret-header provenance were added before approval |
| Focused template/consumer tests | passed | 68 tests across the engine, authority, proxy rewrite, outbound request, ctx-set, and session adapter |
| Full formatting | passed | `cargo fmt --all` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | exit 0 with existing repository warnings |
| Agent documentation | passed | 1,114 files, 36 skill entries, and 711 source-path scopes checked |
| SSA force guard | passed | 1,432 files checked; one existing allowlisted site |
| Metrics inventory | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages share `edgion_v3` and one locked revision |
| Gateway no-tracing guard | passed | data-plane boundary and cold-function registry clean |
| Final frozen integration paths | passed | user-authorized concurrent baseline unchanged by B0058 |

## Handoff
- Ledger/changelog updated: yes; both target rows close as deferred to D0024
- Commit: pending
