# B0035 — LinkSys Runtime Store

## Scope

- Baseline: `80b53a89666476ddcebb2b3aac3dbf3b1e84f0ac`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- File: `edgion-gateway/src/link_sys/runtime/store.rs`
- Approved knowledge support: `skills/04-review/link-sys/intentional-design/health-metric-free-function-and-cardinality.md`
- Approved deferred records: D0007, its index row, and `tasks/todo/add-etcd-linksys-health-flip-reporting.md`.
- Forbidden files: every other source or knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the source/knowledge/deferred paths above plus B0034/B0035 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0035 analysis turn.
- Decision: `change`, simple. Scrape-time `edgion_link_sys_health` sampled published Redis, Elasticsearch, Kafka, and OTLP generations but omitted Etcd despite its identical persistent-runner store and synchronous atomic health state.
- Ownership decision: reuse the canonical generic `record_current_health` operation. Webhook remains under its separate manager lifecycle and metric ownership.
- Preserved semantics: only `runner.current()` published generations are sampled; candidates, deleting/retired generations, stale-series-on-delete behavior, dispatch, replay, instance caps, and shutdown remain unchanged.
- Observability boundary: reuse the existing validated `namespace/name` label and gauge once per scrape; emit no request-path metric, endpoint, credential, error detail, or new label.
- Freshness revalidation: valid at `80b53a896`; B0034 changed async log-drop attribution and records only.

## Implementation

- Added `ETCD_RUNTIME` to the canonical published-health sampler with `EtcdLinkClient::healthy`.
- Added a deterministic no-network test that publishes one uninitialized Etcd client, scrapes an isolated recorder, asserts its single zero-valued series, then deletes and joins the runner under the shared store lock.
- Updated health ownership knowledge to list all five persistent-runner providers and distinguish Webhook. Corrected flip-counter wording to reflect that Etcd currently has the gauge and transition logs but not the flip counter.

## Independent review

- Reviewer: `b0001_review`, independent from the B0035 analyst and implementer.
- Verdict: source and corrected knowledge approved after checking publication ownership, atomic health semantics, cardinality/redaction, deterministic recorder isolation, cleanup/join, Webhook separation, metrics inventory, and exact scope.

## Deferred issues

- D0007 records the Etcd sub-scrape flip-counter gap. Closing it requires a binary-owned transition seam because the shared Etcd client must not depend on Gateway metrics.
- The existing `tasks/todo/link-sys-cross-provider-partial-replacement.md` remains separate and was not modified.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | Store 50 and exact published-health/Etcd gauge regressions passed. |
| Independent review checks | passed | Gateway check/Clippy, formatting, metrics inventory, no-tracing, agent docs, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `a4c0e207240f69ec2c250f1895377119c2c2e4b0`.
