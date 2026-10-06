# Integrate the Gateway JSON Request-Body Path

## Authoritative checkpoint (2026-10-06)

Status: Completed actual capture-chunk wiring and independent final approval.

Full JSON now feeds borrowed downstream chunks through the canonical capture loop before upstream dispatch. Cached bodies use bounded immutable snapshot chunks. Real EOF, exact parity and caps precede publication/Phase 2. Canonical BodyStorage remains the only forwarding/retry cache. JSON-only spill uses no MaterializeRead; raw visibility materializes once. Fixed-label inspected-byte accounting and provisional RAII capacity charging are connected.

Current implementation, verification and independent review are tracked in
[plan 16](16-json-capture-wiring-and-closure.md) and
[validation task 06](06-json-validation-and-benchmark.md).
These checkpoints supersede the historical interfaces and acceptance statuses
below. Historical measurements retain their original scope and revisions.

## Historical checkpoints (superseded design)

> JSON design supersession (2026-10-05): the active replacement and validation
> record is [the fused JSON checker task](../pending/waf-json-checker/waf-json-checker.md).
> Its single grammar pass, selected retention, canonical Bytes publication and
> Full-mode JSON execution replace the earlier narrow subscriber/snapshot
> APIs described below. Earlier measurements and acceptance reports remain
> historical evidence. Broader transport-streaming, XML and response work
> are not completed by this replacement.


## Meta

| Key | Value |
|-----|-------|
| Status | in-progress |
| Priority | P1 |
| Depends on | Bounded JSON transaction API; task 03 publication contract |

## Reviewed delivery design (2026-10-05)

The first Gateway delivery is an explicitly configured full-body JSON snapshot
adapter, awaited before backend selection and forwarding. Incremental parser
input does not imply transport streaming or cache-free capture. Preserve default
prefix admission and the existing full-body evaluator.

### Configuration

Add optional `spec.waf.requestBody.json`, allowed only with `inspection: full`.
Require explicit numeric `lexical`, `document`, `plan` and `facts` objects mapping
to the independent engine budgets. Reject unknown fields and missing budgets.
No production defaults or automatic activation are selected. Zero budgets are
meaningful restrictive limits, never replaced with defaults. Resolve to
`JsonExecutionLimits` once when constructing the immutable plugin runtime.
Controller validation, inline/resolved compilation and support inspection agree.

### Execution and ownership

1. Execute Phase 1; denial performs no body capture. Body-independent generations
   remain body-independent even with JSON configuration.
2. Attempt `start_json_request_body(Auto, limits)` on the selected full transaction.
   Only initial `Unsupported` selects ordinary publication. All other admission,
   parser, budget and runtime failures fail closed; no post-consumption fallback.
3. Await canonical `ensure_request_body(Some(physical_cap))`. Plugin limits never
   shorten shared capture. Explicitly check returned length against physical,
   canonical WAF and JSON source limits, including an existing cache whose
   accessor does not recheck optional caps.
4. Acquire the canonical immutable snapshot reader. Known-bodyless comes only
   from immutable transport `body_declared == Some(false)`. Check reader length,
   count every bounded chunk, and reject inconsistent metadata, empty nonterminal
   chunks, overshoot or early EOF. Expected length is not EOF: actual reader
   `None` alone permits `Complete` and Phase 2 finalization.
5. Explicit read failure abandons provisional state and uses the body-read policy.
   Cancellation drops the local transaction and reader. No source-remainder
   restart, parser background task or additional body cache exists.
6. Record one Phase 2 outcome/time and install the transaction once. Generation
   Arc and numeric budgets stay pinned across hot updates. Preserve complete-body
   replay/retry and subsequent full-mode mutation semantics.

Preserve the execution timer's existing boundary: capture, snapshot disk reads
and parser publication are excluded. Time `finish_json_request_body` (the
canonical Phase 2 scheduler), matching ordinary `execute_request_body` timing.

Use existing payload-free error classifications. Source/canonical byte caps map
to 413; storage/read errors use the shared gate; independent parser/document/fact/
plan limits fail closed even in DetectionOnly. Malformed On/DetectionOnly behavior
is owned by the engine. Emit no chunk logs.

| Failure | HTTP status and existing WAF classification |
|---------|--------------------------------------------|
| Canonical `Runtime` | Delegate to existing runtime error/status mapping |
| JSON source/lexical source bytes | 413 / BodyLimit |
| Other lexical/document/fact/plan limits | 500 / RuntimeLimit |
| Allocation or arithmetic | 500 / Allocation or Arithmetic |
| State or duplicate completion | 500 / PhaseState |
| Unsupported after admission | 500 / Unsupported; never fallback |
| Reader failure, cancellation notification or inconsistent lengths | Existing fail-close body-read gate |
| Malformed syntax | Let engine On/DetectionOnly canonical handling decide |

The existing active-runtime gauge remains canonical logical installed-transaction
accounting, excluding local provisional JSON capacities and RSS. Accurate
provisional capacity observation remains a task 06 gate.

## Frozen implementation scope

- Resources WAF config, config-module exports and mounted WAF tests.
- `config/crd/edgion-crd/edgion_plugins_crd.yaml` and the focused resource target
  `tests/waf_request_body_crd.rs`: this WAF subtree is structural and would prune
  undeclared JSON fields. Independently reviewed amendment adds all nested fields,
  integer/minimum-zero types and required sets. Keep full-only validation at ingest,
  matching existing prefixSize handling; no new admission CEL is added.
- Gateway WAF adapter and mounted WAF tests/fixture support.
- Existing edgion-tests WafDynamic scenario, WAF suites/exports and new JSON
  fixtures under the existing integration/K8s WAF roots.
- The runner's existing loopback preflight exemptions: include exactly
  `Gateway/WafDynamic`, whose selected fixture closure uses only `127.0.0.1`.
- WAF user references, plugin architecture and owning body review page.
- This queue's descriptions and continuation evidence.

Do not change engine classification, RuntimeLimits defaults, capture implementation,
response handling, shared metrics, dependencies or unrelated tasks. S19's separately
approved restoration is the only engine mutation in this delivery.

## Required validation

- Resource schema/deserialization, full-only validation, restrictive zero limits,
  unknown/missing fields and inline/resolved compilation parity.
- Gateway default/prefix/body-independent behavior; initial unsupported fallback;
  split JSON, duplicate/query ordering, malformed/DetectionOnly; admission/feed/
  read/EOF/length failures and cancellation.
- Real canonical memory/spill readers and logical edits without read_all.
- Shared WafDynamic: JSON allow/deny, H1/H2 wire preservation, zero backend exposure
  on rejection, opt-in hot updates, invalid retention, recovery/delete/recreate
  and in-flight generation pinning.
- Targeted engine/resource/Gateway tests and nine repository quick checks.

## Remaining exit gate

This adapter alone does not finish hybrid work or authorize default activation.
Task 06 still owns CPU/memory/wire acceptance and provisional-memory observation.
Simultaneous raw/JSON/snapshot execution is not delivered by this checkpoint.

## Implementation and independent review

The explicit configuration, awaited adapter, structural CRD and mirrored shared
WafDynamic fixtures are implemented. Two independent reviewers approved the
production code; the full-delivery reviewer also checked wire tests, original
bundle digest computation, mirrored fixtures and documents. A second review
approved the added changed-budget/mode in-flight scenario and final test/import
corrections. Red-rule checks covered canonical storage, logging/redaction,
fail-close policy, schema/transport, immutable publication, English content,
unit-test layout and shared integration ownership. No engine classification or
default changes were made.

Final reviewed SHA256 identities:

- Gateway adapter: `8f8b09e369b3a62786684be2896f5ef08b0234b9d1b261ba44c43288f33e5338`
- Gateway tests: `21700846082ad30fd8333f62a6681e3b8761288923827c64bab9df0cb695dce4`
- Resources WAF config: `6fbaf2f96f3e672646bbcc8d9050d9bf42009c0b8e29c4c3ca50b62b7482001e`
- CRD parity target: `3861ecc2c8cb8ae57070c98bf222ce3c5f328607baf8f1bba81d8d7701b3c37c`
- Plugins CRD: `4f55dbabc8da80a7e4963c51f14c806bf16408e4db525622e72c253bac76cd63`
- WafDynamic scenario: `7efd17ba32db9819d2346dda3d1a51b3ceb97871b5ca9e313f2b319d62fed5b3`
- JSON wire module: `4b27b1748e632dbeb15e323abdc9fd8200c8983e010c6b11a28aa22cbb118065`

Initial Gateway compilation exposed missing outer config exports and test-only
enum equality assumptions; both were corrected without broadening production
context types. Final verification results are recorded when commands finish.

## Fresh verification (2026-10-05)

| Command | Result |
|---------|--------|
| `cargo test -p edgion-waf --lib --quiet` | 519 passed, 2 ignored after S19 withdrawal |
| `cargo test -p edgion-waf --test json_execution_api --quiet` | 5 passed |
| `cargo test -p edgion-gateway --lib plugins::http::waf::tests` | 29 passed |
| `cargo test -p edgion-resources --lib resources::edgion_plugins::plugin_configs::waf::tests` | 22 passed |
| `cargo test -p edgion-resources --test waf_request_body_crd` | 1 passed |
| `./cicd/checks/check_crd_coverage.sh` | 19 structural and 8 condition tests passed |
| `cargo fmt --all` | Passed |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| Agent docs, SSA, metrics inventory, Pingora source policy, Gateway API transform and unit-test layout guards | All passed |
| `git diff --check` | Passed |
| Gateway data-plane tracing guard | Passed |
| Shared `Gateway/WafDynamic` integration, `--no-prepare` | Passed, scenario 81s; final residue/requeue/metrics/config guards passed |

Logs are `/tmp/waf-json-{gateway,resources,crd}-tests.log`,
`/tmp/waf-json-crd-coverage.log`, `/tmp/waf-json-workspace-{check,clippy}.log`
and `/tmp/waf-json-guard-*.log`. Full `cargo test --all` was not requested or run.
Coherent integration binary preparation passed. The first WafDynamic invocation
stopped before startup because macOS lacked aliases used by unrelated LB/GeoIP
suites. Independent YAML inspection confirmed all four EndpointSlice addresses
in this scenario's selected fixture closure are `127.0.0.1`. The one-line runner
amendment was independently approved; generic Gateway and full sweeps retain
alias checks. Bash syntax and the targeted portability guard passed. No system
network configuration was changed.

Runner reviewed SHA256:
`d97759e4e00688f10ac9a19c51732c69daf3b9e784b3c134b1987740b3f6338a`.
The shared WafDynamic rerun exited zero. Report:
`tmp/integration_testing/testing_20261005_103649/report.log` (PASS; 16 passed,
0 failed, 2 expected skips for separate preparation and unnecessary aliases).
The registered dynamic scenario passed in 81 seconds on one Controller/Gateway
topology. It covered downstream/upstream H1/H2 body length/hash/version,
memory/spill snapshots, JSON allow/deny/malformed/duplicate/root-array/query
ordering, DetectionOnly and zero-source limits, invalid status plus retained
generation, deletion/recreation, cancellation/recovery, and in-flight rule,
budget and mode pinning. Existing prefix/full/bundle/selector phases also passed.
Final metrics-label, unknown-config, requeue and cache-residue checks passed.
The runner cleaned up all owned services. It escalated Gateway shutdown to
SIGKILL after its existing five-second grace period; graceful shutdown was not
an acceptance claim of this run. No Kubernetes runtime or performance gate was run.
Logs: `/tmp/waf-json-integration-prepare.log`,
`/tmp/waf-json-dynamic-integration.log`, and the work directory's
`test_logs/Dyn_WafDynamic.log`.
