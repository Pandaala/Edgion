# Freeze the Structured Streaming Contract

## Authoritative checkpoint (2026-10-06)

Status: Current JSON contract implemented; general classifier promotion deferred.

The fused checker contract replaces the old subscriber/document/fact API. `start_json_inspection` returns a scoped transaction guard with feed, complete and finish operations; real EOF and complete-source prelude admission precede publication. The reserved `StreamingStructured` classifier is not promoted globally. The old S1–S19 contracts and number-normalization references below are historical.

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
| Depends on | Existing Phase 2 execution classifier |

## Latest recovery summary (2026-09-30)

> Superseded recovery pointer: the current continuation checkpoint is recorded
> at the end of this file. Historical process IDs below are not live-job evidence.

Workspace `/Volumes/ExtStore/ws6/Edgion`, branch `feature-08-30`, HEAD `da048aef`
(S18 locally committed, no push; parent S17 `9aca8545`). The commit contains exactly
json_rules.rs, its mounted tests, waf_body_snapshot benchmark and this task record.
Only this post-commit recovery update remains dirty within WAF scope. Unrelated
CRD/mesh/plugin tasks are preserved and excluded.

S18 shares scalar metadata: Fact and QueryHeader each measure 24 bytes, with one
aggregate actual-capacity bound and coordinated reservations. Both independent
reviewers approved two source rounds and final measurement documentation. Fresh
verification passed: 519 library tests, 2 ignored; 5 public API tests; formatting,
workspace check/Clippy and all nine prescribed pre-commit stages. No full workspace
test run was requested. Source hashes and final evidence are recorded below.

Frozen benchmark 88f79815...be8a97 produced validated baseline/final body and query
runs plus two timing repeats. Wide/duplicate spill peak extra requested heap fell
45.3/49.7 percent; query-many/32 rules fell 42.0 percent. One-rule queries add 16
bytes and extra allocation events. Duplicate/malformed timing repeats remain about
10-11 percent slower; no throughput improvement or Gateway activation is claimed.
Raw logs remain under /tmp/waf-s18-*.log; tables preserve both timing runs.

S19 candidate is implemented in json_rules.rs and mounted tests; two independent
source review rounds passed with hashes a57350...f3e710 and d32d2e...44cb99. The
114-test JSON run and engine ABBAx2 measurements passed. Timings are mixed and
control paths drift similarly; no speedup or resolution of S18 regression is proven.
Full WAF tests passed (522 library,2 ignored;5 API). Workspace check and Clippy
exited0, all six guards exited0. Sequential runner18448 exited0 including
release snapshot build. Isolated snapshot ABBA measurements are live in session97223
(/tmp/waf-s19-snapshot-paired.log); no build/test jobs remain live.
Formatting passed and source identities stayed frozen. Preserve these processes.
Next: finish gates/build, capture candidate snapshot binary, then isolated ABBA
BodyStorage timing/query comparisons against /tmp/waf-s19-snapshot-baseline. Obtain
independent evidence review and commit only if the bounded fast path remains sound
and measurements show no material regression; otherwise remove the experiment.
Reconcile stale tasks03-06/README before Gateway activation in a following batch.
Then define bounded activation policy and implement the awaited full-JSON snapshot
adapter with unsupported-only initial fallback, actual EOF, cancellation and
wire/hot-reload evidence. Do not follow task 05's stale unconditional no-cache or
relay-callback framing. No production JSON activation or defaults have been chosen.
The original overall WAF improvement goal remains active. Continue scoped batches,
independent multi-round review, local commits and no push.

## Goal

Define the exact bounded semantics that allow a Phase 2 rule unit to use
`StreamingStructured`. The compiler must never classify a rule as streaming
when incremental execution can change its SecLang result.

## Work

- Inventory the Phase 2 JSON and XML targets used by the admitted CRS profile.
- Define which operators and transformations can consume incrementally
  published scalar values.
- Define duplicate-key, array-index, path, escaping, malformed-input, and EOS
  behavior.
- Separate monotone early-decision rules from rules that must finalize at EOS.
- Define the independent limits for depth, token bytes, decoded bytes, entries,
  path bytes, retained bytes, and parser work.
- Specify downgrade rules from `StreamingStructured` to `BoundedSnapshot`.
- Require `Unsupported` when no bounded exact interpretation exists.

## Tests

- Table-driven classifier tests for every accepted and rejected target,
  operator, transformation, chain, negation, and stateful-action combination.
- Tests proving that an unsupported structured combination cannot silently
  enter either the raw or structured streaming lane.

## Exit gate

The compiler capability matrix is documented and executable tests prove every
Phase 2 rule unit receives one conservative execution class.

## Recovery checkpoint (2026-09-29)

Baseline: `862d5add`, branch `feature-08-30`, workspace
`/Volumes/ExtStore/ws6/Edgion`. Batch S1 corrects overclaimed execution
capabilities in `classify_request_body_unit`; no structured lane is enabled.
Unrelated CRD/mesh/plugin-wiring task edits belong to other work and are excluded.

Stable review queue for this batch:

1. `edgion-waf/src/seclang/compiler.rs`: classifier and body-macro dependency
   analysis reviewed; correction implemented; two independent review rounds approved, final checks passed; committed locally as `582dae37`.
2. `edgion-waf/src/seclang/body.rs`: JSON publication reviewed without production
   change; duplicate-subtree regression added in its existing unit test file.
3. `edgion-waf/src/seclang/transaction.rs`: raw stream admission, selectors and
   complete publication reviewed without production change; macro admission
   regression added in its existing unit test file.
4. `edgion-waf/tests/unit/seclang/{compiler,body,transaction}_test.rs`: modified
   tests independently reviewed; final verification passed (400 tests, 2 ignored).

The initial independent investigation confirmed body macros were ignored by the
classifier, structured targets were promoted without an execution proof, and
unsupported XML paths were misclassified. Current JSON is last-key-wins with
sorted object traversal; duplicate objects replace entire earlier subtrees.
Task 03's source-order publication conflicts with its exact reference-order gate.
Do not resolve that conflict by silently changing the reference semantics.

S1 committed locally as `582dae37` on 2026-09-29; no push.
Next: inventory the admitted CRS profile and finish the bounded structured
contract. The CRS checkout was not found locally, so historical stage counts
are not fresh evidence. Task 01 remains open after S1: parser limits, exact
incremental publication semantics and a supported structured subset still need
implementation/proof before advancing task 02.

## S1 executable capability boundary

| Compiled Phase 2 dependency | Current class |
|---|---|
| Existing eligible raw phrase member | `StreamingRaw` |
| No body target or body macro | Existing raw/body-independent classification |
| Any `ARGS`/name/count, file, multipart, supported XML target or `REQBODY_PROCESSOR` | `BoundedSnapshot` |
| Body macro in operator, action, setvar key/value or targetless action | `BoundedSnapshot` |
| Raw body operator outside the existing exact raw subset | `BoundedSnapshot` |
| XML path other than `/*` or `//@*` | `Unsupported` |
| Response target in Phase 2 (normally rejected by semantic validation) | Defensive `Unsupported` |
| Structured incremental execution | Reserved; no promotion yet |

`Unsupported` dominates other member/target classes. This classification does
not add configuration rejection or an XML evaluator: complete-body runtime
errors remain unchanged. Chains, negation, captures, transformations and state
mutations cannot opt a structured target back into streaming. Negation in the
current transaction is per selected value, not a collection-wide absence test.

A future structured implementation must preserve last-key-wins replacement of
whole JSON subtrees, reference-visible value order, numeric normalization,
array-length pseudo-values, literal dotted paths, and malformed/EOS behavior,
or explicitly resolve the conflicting contract before changing those semantics.
No early scalar hit currently proves a final JSON verdict. Arbitrary malformed
suffixes and later duplicates can invalidate that conclusion.

## External source evidence inspected for S1

- [Coraza v3.7.0 JSON processor](https://github.com/corazawaf/coraza/blob/v3.7.0/internal/bodyprocessors/json.go):
  reuses a path buffer with append/truncate and bounds recursion, but reads the
  complete body and flattens into a map. Adopt the reusable-path/bounded-depth
  approach for later work; this is not evidence for exact chunkwise execution.
- [serde_json v1.0.145 Value deserializer](https://github.com/serde-rs/json/blob/v1.0.145/src/value/de.rs):
  object entries insert into a map, replacing duplicate keys. The local body
  regression verifies the actual dependency behavior used here independently
  of this upstream example version.
- The original Curiefense repository/source URLs were unavailable during this
  inspection. No Rust WAF performance or semantic claims are inferred from
  search summaries; primary-source Rust WAF comparison remains open.

This batch establishes correctness prerequisites, not measured throughput gains.

## S1 verification checkpoint

- Diff identity: baseline `862d5add` plus the four WAF source/test files listed
  above (18 added/1 removed production lines, three test files), task 01 and
  README status. No dependency, Gateway, schema, or transport edits.
- Two reviewers independently read repository rules and original WAF/control
  and body-prefix knowledge, then reviewed the entire batch twice. Both
  approved. Round 1 suggestions led to chain-child structured/macro, action
  template and SetVar-key coverage; round 2 rechecked these additions and
  upstream source citations. No unresolved production finding.
- Initial WAF suite passed 397 tests/2 ignored before XML and transaction
  coverage additions. An intermediate new response-target fixture failed
  because semantic validation rejects it before classification; removed that
  invalid fixture, retaining unsupported XML coverage. Final `cargo test -p edgion-waf --lib --quiet` passed: 400 passed,
  0 failed, 2 ignored (exit 0), log `/tmp/waf-s1-tests.log`.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets` both passed (exit 0); logs
  `/tmp/waf-s1-check.log` and `/tmp/waf-s1-clippy.log`.
- Formatting/diff checks, agent-doc consistency, SSA-force, metric inventory,
  Pingora source policy and unit-test layout passed. Gateway API transform
  guard initially lacked PyYAML; rerun with the existing
  `/tmp/edgion-p090-005-tools/bin/python3` environment passed.
- Full workspace tests, CRS acceptance, dynamic Gateway integration and
  performance benchmarks are not run for S1. No throughput claim is made.

Reviewed and verified code SHA-256 (before local commit):

- `edgion-waf/src/seclang/compiler.rs`: `b5f71324178c8ca3fdcbda5fdaa6d9c238f3d23a082ea4565143028fd98bd0d5`
- `edgion-waf/tests/unit/seclang/body_test.rs`: `592e727620a0481209648d20a2c36b2d318c852819f3d2a797d07482da731496`
- `edgion-waf/tests/unit/seclang/compiler_test.rs`: `6dfed3e3844a8dd69df6b6dcc6a730e4ba4bfabe23558e35640ff1c547cfa1a1`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `a324bc3e170d89def13b57e1c2682b06b00f5559453adb3879263590c3520674`

## S2 recovery checkpoint (2026-09-29)

Baseline `582dae37`; S1 is committed and the prior turn made verified progress.
S2 investigates the exact structured contract and implements the independent
incremental tokenizer foundation without enabling any rule execution lane.
The publication-contract choice has been presented to the user: preserve
last-key-wins/reference order with bounded key/result metadata (recommended),
or explicitly change duplicate/order semantics. No dependent publisher change is made in S2. With no contrary preference
received, planning proceeds with the conservative recommended route; existing
request semantics do not change.

S2 file queue and ownership:

1. `edgion-waf/tests/ftw_inventory.rs`: add an explicitly ignored inspection
   gate reusing `compile_ftw_generation`, with required corpus/output paths;
   never confuse executable-subset inventory with strict production admission.
2. `edgion-waf/src/seclang/json_stream.rs` (new), its matching unit-test file,
   and `seclang/mod.rs`: tokenizer implementation in progress. Tokens are
   provisional source-order lexical events only; no transaction/Gateway wiring.
3. This checkpoint and task 02: record the tokenizer's independent budgets,
   exact observed verification, and remaining publication gates.

The first independent contract audit confirms that exact last-key-wins
replacement needs bounded subtree identity/key/result metadata, including
unambiguous segmented internal paths. It cannot promise constant memory
independent of unique keys. Existing publication entry/decoded limits count
surviving values; tokenizer budgets must count consumed lexical input separately.
Empty HTTP-body acceptance belongs to the adapter; empty lexical JSON is invalid.

The Git retrieval failed with an SSL error (session 98204, exit 128).
The v4.25.1 release archive download succeeded and was extracted under
`/tmp/edgion-waf-crs-source/coreruleset-4.25.1`. The annotated upstream tag
resolves to commit `3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`.
S2 tokenizer, pinned inventory and independent two-round reviews are complete;
checks are recorded in task 02. Next: finish eligibility and result-metadata
budget proof before task 03 integration. Do not advance task 01 or claim a structured lane
until its semantic proof and publication contract are complete.

## S2 current corpus evidence

Fresh inventory uses the existing exploratory executable-subset compiler,
not the strict production acceptance manifest. It requires explicit corpus and
output paths and is ignored by ordinary test runs. Missing paths fail when the
inspection test is explicitly selected.

- CRS: v4.25.1, commit `3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`.
- Archive SHA-256: `0539e66e7627fe71c160a644d8fb7ab6e450d53c9de208be5f95a35c70e1a154`.
- Rules input SHA-256: `4b51b7a909056215848b248195c6d9ffb4f140836a835b7e0ae9b3d5e95ff46e`.
  Hash all 50 regular `rules/*` files sorted by UTF-8 filename; for each file,
  hash big-endian u64 name length, name bytes, big-endian u64 content length,
  content bytes. Reject nonregular entries. The test checks the actual inputs.
- Phase 2: 289 admitted units, 25 chains; 215 `BoundedSnapshot`, 74
  `StreamingRaw`, zero structured or unsupported classes.
- JSON-capable targets: 187 plain `ARGS` target records, one literal `ARGS`,
  one count `ARGS`; 183 plain `ARGS_NAMES` plus three exclusions; one
  `ARGS_COMBINED_SIZE`. These are target-record counts, not distinct rules.
- XML paths: 174 `/*`, 174 `//@*`; 173 rule units combine XML and Args/ArgsNames.
- Projection SHA-256:
  `f996b6d70f2d52152bbd5da782b4c62e7a58466e10c02571ccc22c47777d3b36`.
  The projection includes ordered admitted IDs/member targets/operators/
  transformations. Action and macro dependencies are not inventoried, so this
  report alone does not prove structured eligibility.

Reproduce (write outputs outside the repository):

```bash
EDGION_WAF_CRS_TESTS_DIR=/tmp/edgion-waf-crs-source/coreruleset-4.25.1/tests/regression/tests \
EDGION_WAF_CRS_INVENTORY_OUTPUT=/tmp/edgion-waf-crs-phase2.json \
cargo test -p edgion-waf --test ftw_inventory pinned_crs_phase_two_capability_inventory -- --ignored --nocapture
```

The first extraction passed. Independent review required binding extracted
inputs and freezing the projection; those gates passed final verification.
The modified-corpus negative probe rejects input before report publication. This is inventory evidence, not a rerun of the FTW verdict suite.

## S3 recovery checkpoint (2026-09-29)

Baseline `c25eb692`; S2 tokenizer and inventory were committed locally, without
push. S3 is in progress: inspect exact transaction observability and add
reference-contract regressions before implementing the bounded result publisher.
No structured lane is promoted. Unrelated mesh/CRD/plugin task changes remain
outside this batch.

Stable queue:

1. `transaction.rs::evaluate_unit`, `evaluate_member_matches`,
   `apply_member_match`, `TxState::clear_matches`: inspection confirms each unit
   clears inherited matches before evaluating. Standalone units collect all
   matching selected values, applying actions once per match. Matched-value
   count and byte limits apply even without a payload reader.
2. `body.rs::flatten_json`, `BodyBuilder::push`: inspection confirms final
   body limits apply to surviving values after duplicate replacement, while
   tokenizer limits account for lexical input independently.
3. `tests/unit/seclang/transaction_test.rs`: reference-contract regressions
   implemented; initial full WAF suite passed 426 tests with two ignored.
   Two further reviewer-requested lifetime/scheduling tests also passed;
   final full WAF suite passed 428 tests with two ignored.
4. Tasks 01/03: replace imprecise observer assumptions with verified lifetime,
   accounting and scheduling requirements after independent investigation.

Two independent investigations completed. Review round 1 found no blocker;
reviewers requested stronger retained-charge and unreachable-error coverage.
Round 2 approved the final four-file diff after both requested tests passed.
Final checks passed; committed locally as `e8519ff5`, without push. No production code
is changed. Next implementation: bounded ordered summary composition and
duplicate subtree replacement, with differential accounting/error tests before
transaction integration or classifier promotion.

## S3 candidate execution contract

This is an implementation target, not an enabled capability. The classifier
stays conservative until executable differential evidence covers the following
contract. The initial subset is deliberately useful for constant phrase rules;
additional operators and actions require their own proof.

| Dimension | Initial candidate | Snapshot until separately proven |
|---|---|---|
| Unit | One member, Phase 2 | Chains |
| Targets | One plain `ARGS` or `ARGS_NAMES`, all or literal name | Multiple/repeated targets, exclusions, count, dynamic selectors, mixed collections |
| Operator | Static `@pm` / `@pmFromFile`, positive or per-value negated | Dynamic operands, captures, other operators |
| Transform | No transform / lowercase using the existing transform engine | Other transforms and `multiMatch` |
| Actions | Static metadata, pass/deny, logging flags | Setvar, controls, skip/skipAfter, runtime templates, capture |
| Completion | Valid JSON EOS, then ordinary scheduled rule evaluation | Early structured intervention |

This subset does not redefine negation as absence of any positive match. A
negated rule matches every selected value for which the positive predicate is
false; an empty collection supplies no match. Query-derived ARGS candidates
precede JSON candidates even when the document arrives incrementally.

### Identity, publication and source budgets

Use typed object-key and array-index identities, distinct from external dotted
`json.*` names. Equal external names do not imply equal identities. Duplicate
object keys replace the complete old subtree, including its summaries and
deferred predicate/final-publication errors. Lexical syntax, UTF-8 and number
errors remain fatal even in a subsequently overwritten value. Object traversal sorts decoded keys;
arrays retain numeric order and emit their length after their children. Empty
objects emit nothing, empty arrays emit a zero length, and null emits empty
bytes. Normalize numbers through the existing `serde_json::Number` semantics.

A surviving scalar or array-length pseudo-value of path length `p` and value
length `v` contributes two entries, `3*p + v` decoded bytes and `p + v` to
ARGS_COMBINED_SIZE. The normal JSON publication additionally contains body
length, processor name and combined-size metadata. Forced raw publication is
a separate payload dependency. Body work includes source length plus path
length for every surviving visited node, including empty containers. Body
retention accounts for entry descriptors and decoded lengths.

The reference validates JSON before traversing surviving values. Its depth,
path, entry and decoded limits do not reject an overwritten subtree merely
because that subtree would exceed a final-publication limit. Preserve that
distinction. Independent lexical and publisher-state budgets may reject such
input earlier, but are additional streaming resource limits, not equivalent
implementations of the old limits. Their defaults, error mapping and admission
policy must be explicit before enabling the lane.

### Ordered result summaries and virtual match accounting

`evaluate_member_matches` collects every match for a standalone unit before
`apply_member_match` applies actions. A boolean OR loses both multiplicity and
deterministic failure behavior. Retain bounded per-subtree/per-rule summaries
with selected count, match count, selected/transformed lengths, matched name
and value lengths, operator work, and deferred failure position. Fold surviving
subtrees in reference order; do not retain scalar payload bytes.

For a pure, noncapture rule, write `l_i` for matched name plus transformed-value
length and `e = size_of::<TxEntry>()`. With unchanged nonmatch TX state of size
`base`, after match `k` the reference TX size is:

```text
base + k*e + sum(l_i, i=1..k) + l_k
```

Thus a final count and total length are insufficient: retain or compose the
maximum ordered prefix of this expression to reproduce TxByteLimit. A long
first match followed by short matches can exceed the limit temporarily even
when the final state would fit. Keep the final `l_k` for committed-state size.
Within a standalone member, `max_operator_evaluations` bounds collected
matching records, not every selected candidate; the phase schedule separately
uses that limit for reached rule units. Selected-value and operator-work limits
remain independent. Error precedence follows
the existing evaluator (selection, all operator evaluations, then staged
match application, then commit retention), not token arrival order.

Matched payloads are not generally visible to the next independent unit:
`evaluate_unit` clones committed TX and calls `clear_matches` on the clone.
Same-unit actions and chain children can see them; capture slots are ordinary
TX entries and survive separately. However, a nonmatching unit does not commit
its cleared clone, so the previous committed matched-state byte charge
remains. `retained_bytes`, response-head publication and response-window
admission still include that charge. Payload elision must preserve an explicit
virtual accounting contribution through this lifetime, distinct from the
actual publisher allocation cap. Do not drop the charge at every rule boundary.

### Deferred execution and remaining gate

Provisional predicate errors belong to a subtree and rule position. Duplicate
replacement can remove them, and an earlier enforced denial can make later
rules unreachable. Report deterministic rule errors only when the ordinary
schedule reaches the rule, after successful publication; detection-only mode
continues and can expose those errors. Parser and actual state-budget failures
remain immediate. Speculative computation still requires its own hard work
limit; deferred errors do not authorize unlimited scanning.

Task 01 remains open: implement bounded summary composition and prove these
error/lifetime semantics, inventory action/macro dependencies in the pinned
corpus, fix concrete state-budget defaults and error mapping, and connect the
eligibility matrix to executable compiler tests. Task 03 must not claim full
variable publication after discarding scalar bytes.

## S3 verification checkpoint

Scope: baseline `c25eb692` plus `tests/unit/seclang/body_test.rs`,
`tests/unit/seclang/transaction_test.rs` and tasks 01/03. Thirteen reference
regressions establish publication, rule and retention constraints. Production
code and dependency resolution are unchanged; no streaming lane is enabled.

- `cargo test -p edgion-waf --lib --quiet`: exit 0, 428 passed, two ignored;
  `/tmp/waf-s3-tests.log`. Targeted structured-reference tests: nine passed.
- `cargo check --workspace --all-targets`: exit 0, `/tmp/waf-s3-check.log`.
- `cargo clippy --workspace --all-targets`: exit 0, `/tmp/waf-s3-clippy.log`.
- Format/diff checks, agent-doc validation, SSA force, metrics inventory,
  Pingora source policy, unit-test layout and Gateway API transform guards
  passed. The transform guard used the existing PyYAML-equipped
  `/tmp/edgion-p090-005-tools/bin/python3`.
- Two independent reviewers read the original project rules and WAF/body
  knowledge, inspected the entire scoped diff and approved two review rounds.
  Their suggestions added retained-charge/nonmatch and deferred-error tests,
  distinguished lexical from revocable predicate errors, and clarified the
  matching-record limit. No unresolved blocker.
- No full workspace tests, Gateway integration or throughput measurement was
  run. Dependency audit was not repeated: no manifest/lockfile change; S2's
  recorded existing advisory baseline remains unresolved.

Final test-file SHA-256:

- `body_test.rs`: `b0341b3addc5109cb5fbad998eecaadc4157ddf8a95b7460f10332fd80d6381d`
- `transaction_test.rs`: `f800ef550f36180591ad4b3c090801374f494544baadd59d843ceeb9c001bb1a`

Task 01 remains in progress and task 03 remains pending. This batch fixes the
reference proof and planning contract; it does not complete structured
execution, choose new runtime budget defaults or establish a speedup.

## S4 recovery checkpoint (2026-09-30)

Baseline `e8519ff5`; the preceding batch made verified progress and is committed
locally without push. S4 implements the fixed-size ordered match-retention
algebra required by the candidate structured executor. It neither publishes
JSON variables nor promotes the compiler lane.

Stable queue and ownership:

1. New `edgion-waf/src/seclang/json_summary.rs`, its unit tests and
   `seclang/mod.rs` exports: implemented and approved in two independent
   review rounds. Summary composition
   preserves match count, total lengths, final length and ordered prefix peak.
   Descriptor charge is fixed at construction because changing it after folding
   can change which prefix has the maximum charge. No payload or allocation.
2. `tests/unit/seclang/transaction_test.rs`: completed differential proof against
   complete-body pure phrase execution and exact transaction byte thresholds.
3. Read-only subtree design: completed; task 03 records open-container
   reduction and the need for bounded ordered accounting metadata.
4. This task records verification and independent review results. Unrelated
   mesh/CRD/plugin task changes remain outside the batch.

Two independent reviewers approved both rounds. Six algebra tests and the
80-case transaction differential test passed; full WAF suite passed 435 tests
with two ignored. Workspace check, Clippy and all static guards passed.
The scoped S4 batch was committed locally as `7419276b`, without push. The full publisher still
needs selected-value/operator-work summaries, deferred errors, key/subtree
state budgets and transport lifecycle proof; this algebra alone is insufficient.

## S4 verification checkpoint

Scope: baseline `e8519ff5`, the new `json_summary` module and unit tests,
`seclang/mod.rs` exports, one transaction differential test, and tasks 01/03.
No dependency, schema, configuration, Gateway or transaction production change.

- The fixed-size summary stores five usize counters, has no allocation or
  payload ownership, and composes in constant time. This is a code-level
  property, not a measured end-to-end throughput claim.
- Six algebra tests passed. Exhaustive replay covers 5,461 sequences of length
  zero through six, three descriptor charges, two base sizes, all ordered
  three-part partitions and both association orders. Boundary cases cover
  usize overflow, mismatch atomicity, zero-length matches and earlier peaks.
- The complete-transaction differential test passed 80 document/base-state
  combinations. Nonempty cases additionally pass at the predicted peak and
  fail at peak minus one. An initial empty-setvar fixture was invalid and was
  corrected to omit the action for the zero-base case.
- `cargo test -p edgion-waf --lib --quiet`: exit 0, 435 passed, two ignored;
  `/tmp/waf-s4-tests.log`.
- `cargo check --workspace --all-targets`: exit 0, `/tmp/waf-s4-check.log`.
- `cargo clippy --workspace --all-targets`: exit 0, `/tmp/waf-s4-clippy.log`.
- Format/diff, agent docs, SSA force, metrics, Pingora source policy, Gateway
  API transform and unit-test layout guards passed. The transform guard used
  the existing PyYAML-equipped `/tmp/edgion-p090-005-tools/bin/python3`.
- Two independent reviewers inspected original rules/knowledge and the complete
  scoped diff twice. Both rounds approved without blockers; they verified
  composition arithmetic, mutation atomicity, differential coverage and the
  deliberately narrower retention-only guarantee.
- No Gateway integration, full workspace tests or throughput measurement was
  run. No dependencies changed; the previously recorded advisory baseline
  remains unresolved and no clean dependency audit is claimed.

Reviewed source SHA-256:

- `edgion-waf/src/seclang/json_summary.rs`: `ad10b6b6ccf926025571d226b720053534679e846b77021f4e254a9ff2dd5c89`
- `edgion-waf/src/seclang/mod.rs`: `550a81cff897383a07407fc44d88dad2ce9f218a3405e9ccde526ed99a46c6c1`
- `edgion-waf/tests/unit/seclang/json_summary_test.rs`: `206c820cbf3ccd0911aec9ae2ed4a99c9f3e61d7e5590de8223181fe463d6782`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `e36b9df8c056e1dfb7ba26250ca68e10ea5305910135eed8c38f15ddc7a5cae3`

Task 01 remains open. Next: implement bounded open-container reduction with
ordered payload-free accounting events, preserving duplicate replacement,
error precedence and cancellation cleanup before execution-lane promotion.

## S5 recovery checkpoint (2026-09-30)

Baseline `7419276b`; S4 is committed locally without push. S5 implements the
bounded open-container reducer and ordered publication-accounting events needed
for the structured proof. Task 01 remains open; task 03 is not declared complete
and no production execution lane is promoted.

Stable queue:

1. New `seclang/json_document.rs`, matching unit tests and module exports:
   implemented and approved in two independent review rounds. The reducer wraps
   the canonical tokenizer, keeps provisional borrowed scalar callbacks, retains
   bounded keys/frames/event metadata and exposes surviving reference-order
   events only after valid completion.
2. `body_test.rs`: independent complete-body differential verification passed
   for path/value ordering, duplicate replacement and publication accounting.
3. Independent semantic audit: completed against the original body parser and
   project rules; two final review rounds approved the full batch.
4. Tasks 01/03: record verified limits, lifecycle scope and remaining rule replay
   gaps. Unrelated mesh/CRD/plugin task changes remain excluded.

Targeted and complete WAF tests passed. Workspace check, Clippy and all static guards passed. The scoped S5 batch
was committed locally as `ba733a6e`, without push.
No throughput or complete structured-execution claim is made.

## S5 verification checkpoint

Scope: baseline `7419276b`, new `json_document` source/unit-test files, module
exports, `body_test.rs` and tasks 01/03. No dependencies, configuration schema,
Gateway wiring, transaction executor or compiler capability class changed.

- The reducer owns a reusable path, open typed frames, sorted object members,
  and a linked event arena with a reused freelist. No scalar payload is retained.
  Completed documents retain only bounded event capacity until dropped.
- Limits are explicit constructor inputs without production defaults: allocated
  reducer capacities, live key bytes, member slots, events, path bytes, depth
  and work. Tokenizer limits are separate; caller callback work/state is not
  included and must be bounded by the future executor.
- Initial review corrected per-slot exact growth to bounded geometric growth,
  and charged event storage by generic slot size. This prevents uncharged large
  result copies and quadratic append-growth charging. Reverse-sorted object
  insertion still incurs explicitly charged shifts.
- Eleven reducer unit tests passed, covering every split, numeric normalization,
  empty-container events, all independent limits, zero bounds, overwritten
  state limits, large generic results, exact actual-capacity accounting, 500
  duplicate replacements, sticky failure/abandonment and payload-free Debug.
- Independent body oracle passed: 18 fixtures at every two-chunk split, 128
  generated nested documents at three split positions, 24,000 competing-limit
  combinations plus dense work thresholds and source-cap boundaries. Callback
  payload copies exist only in test fixtures. Malformed-input reference tests
  distinguish prelude limits from traversal errors.
- An initial reverse-order work test used different result types for measured
  and bounded runs; corrected to the same type. The retained-capacity boundary
  test finds the minimum admitted cap because bounded adaptive growth may use
  less capacity than a generous-budget run.
- `cargo test -p edgion-waf --lib --quiet`: exit 0, 450 passed, two ignored;
  `/tmp/waf-s5-tests.log`. Targeted `json_document` tests: 14 passed.
- `cargo check --workspace --all-targets`: exit 0, `/tmp/waf-s5-check.log`.
- `cargo clippy --workspace --all-targets`: exit 0, `/tmp/waf-s5-clippy.log`.
- Format/diff, agent-doc, SSA-force, metrics inventory, Pingora source policy,
  Gateway API transform and unit-test layout guards passed. The transform guard
  used `/tmp/edgion-p090-005-tools/bin/python3` with PyYAML.
- Two reviewers independently read project rules/original knowledge and
  inspected the full source/test/contract batch twice. Round 2 approved the
  growth/copy fixes and strengthened coverage without unresolved blockers.
- No full workspace tests, Gateway integration or throughput benchmarks were
  run. Dependency resolution is unchanged; the earlier advisory baseline
  remains unresolved and no clean audit is claimed.

Reviewed code SHA-256:

- `edgion-waf/src/seclang/json_document.rs`: `36f00d1e70bcc29298171e2f40685f3adc86d28faeb0a88ac4543826d05787bc`
- `edgion-waf/src/seclang/mod.rs`: `edaf19e269788a6dcbf77730bcec7b7de90acd74f3ef07f10e63bc817fd51063`
- `edgion-waf/tests/unit/seclang/json_document_test.rs`: `701122434674b37e390f9f858f965dd9fdb69c2059b4bf46ed8c932f105edce2`
- `edgion-waf/tests/unit/seclang/body_test.rs`: `66af9d778b84b4ad25ee7c2363a967702f25b77f78f9955d950fbd34be25a819`

Task 01 remains open. Next: exact publication-budget replay and eligible rule
subscriber/result replay, including query-before-body order, deferred errors
and virtual TX retention, before promoting the execution lane. The reducer
foundation does not itself establish those transaction guarantees.

## S6 recovery checkpoint (2026-09-30)

Baseline `ba733a6e`; S5 made verified progress and is committed locally without
push. S6 extracts the canonical publication accounting from BodyBuilder and
uses it to validate trusted completed JSON metadata without materializing
payload entries. Existing processors remain consumers of the same checks.
No rule execution or Gateway lane is enabled.

Stable queue:

1. `body.rs`, optional `json_document.rs` adapter and `mod.rs` exports:
   implementation in progress. Preserve entry/name/decoded/work check order,
   finish-time combined-size/retention checks and format-specific semantics.
2. `body_test.rs`: completed-document validator differential coverage plus
   independent literal cross-format baseline vectors in progress. Comparing
   only two consumers of newly shared accounting is insufficient on its own.
3. Independent baseline audit: HEAD source verified for raw, URL encoding,
   JSON, XML, multipart, fallback and empty-body metadata/limit ordering.
4. Tasks 01/03: record verification scope and unresolved adapter/rule-replay
   boundaries. Unrelated task edits remain outside this batch.

The completed-document validator does not accept failed, active or abandoned
parses as empty documents. Empty HTTP bodies and malformed-input prelude
precedence remain separate adapter responsibilities. Next: compare literal
baselines, validator output and error precedence, then run two independent
full-batch reviews before local commit.

### S6 verification checkpoint

Implementation and tests are complete against `ba733a6e`: shared BodyAccounting,
completed-document validation and public summary exports. Both independent
round-1 reviewers found no production blocker after checking original rules,
reference semantics and allocation/error paths. The final diff adds production
validator comparisons to the existing every-split/generated/competing-budget
oracle, literal cross-format baseline totals, raw/head-overflow comparisons,
and active/failed/abandoned rejection. Final independent review is next.

Workspace check and Clippy are running with logs `/tmp/waf-s6-check.log` and
`/tmp/waf-s6-clippy.log`. Final WAF library test log is
`/tmp/waf-s6-tests-final.log`; results must be confirmed before commit.

Both independent reviewers approved round 2 on the complete final batch;
no unresolved correctness or contract finding remains. They independently
verified cross-format precedence, successful-only metadata access, raw/head
arithmetic and lifecycle coverage. Reviewed source SHA-256:

- `body.rs`: `36ac2cd03ad1605510b4e1e266bdaa625848a65a8c6262dcf9dd85eeadacc554`
- `mod.rs`: `18decab81caab8b6adfcf77579647767281b3e532522aa6b6c2b03b3dbf0c3ca`
- `body_test.rs`: `7cfd68ef57aaff9adb2ccaacaf71b4cd8df1a8e9b68f1daae2968f8a71279347`

Agent-doc, SSA, metrics inventory, Pingora source policy, Gateway API transform,
unit-test layout, formatting and diff whitespace guards passed. Compilation,
Clippy and the final library test run remain in progress.

### S6 final verification (2026-09-30)

All final gates exited 0 on the reviewed source hashes above:

- `cargo test -p edgion-waf --lib --quiet`: 453 passed, 2 existing ignored;
  `/tmp/waf-s6-tests-final.log`.
- `cargo check --workspace --all-targets`: passed; `/tmp/waf-s6-check.log`.
- `cargo clippy --workspace --all-targets`: passed; `/tmp/waf-s6-clippy.log`.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Agent-doc, SSA force, metrics inventory, Pingora source policy, Gateway API
  transform contract and unit-test layout guards: passed. The transform guard
  used `/tmp/edgion-p090-005-tools/bin/python3` for its existing YAML dependency.

Both independent reviewers approved two rounds. No dependencies, Gateway wiring,
classifier promotion or response handling changed. No full workspace tests or
new throughput benchmarks were run. S6 removes two finish-time entry scans by
inspection; no quantitative performance gain is claimed. Tasks 01/03 remain open
for the transport adapter and eligible-rule replay contracts.

S6 committed locally as `e28145d0`; no push. The next batch starts from the
transport-adapter boundary described above; preserve unrelated working files.

## S7 adapter design checkpoint (2026-09-30)

Canonical body failures are finalized only at EOS: final byte length, source
work, optional raw/length/processor prelude, syntax, surviving traversal,
combined-size and retained-byte arithmetic. The adapter owns the total byte
counter because the tokenizer stops inside the first malformed chunk. Syntax
failure drops document storage and enters bounded length-only drain; no later
callback runs. Zero bytes at EOS is empty HTTP, while whitespace is malformed.

An independent source admission bound remains active during drain. Streaming
resource, allocation, internal and subscriber errors are immediate distinct
failures, never malformed fallback. Source admission applies to an entire
supplied chunk before parser dispatch; independent-budget error ordering is
not promised to match the complete-body parser. Ordinary BodyError precedence
is promised only within those separate streaming budgets.

Detection-only fallback is considered only after the original prelude succeeds
and syntax is malformed. It replays fresh accounting: raw defaults off for
valid JSON and on for fallback, preserving the three-state explicit override.
The summary is logical cost, never retained raw bytes or published variables.

Installed serde_json 1.0.151 starts with remaining_depth=128 and rejects when
container entry decrements it to zero. An adapter-only tokenizer mode rejects
the 128th open container as syntax, including subsequently replaced subtrees.
Normal tokenizer/document constructors retain their independent configured
limits. Tests must pin 127/128 and verify lower configured depth remains a
streaming resource error rather than becoming detection-only fallback.

### S7 round-1 implementation review

Both independent reviewers inspected the full production/test batch and found
no implementation blocker. One contract clarification was applied: standalone
`validate_json_publication` only validates accounting, because ordinary
`JsonDocument` has configurable syntax depth beyond the reference body parser.
Its API docs and task 03 now direct exact body callers to `JsonBodyAdapter`.

Seven adapter tests cover every split across valid/empty/malformed input,
competing prelude limits, fresh fallback, final-length literal error payloads,
independent source/reducer/depth/sink errors, terminal cleanup and Debug safety.
Reference nesting tests include overwritten subtrees. The full WAF library
run is active at `/tmp/waf-s7-tests.log`; final results and round 2 remain pending.

### S7 final review checkpoint

Both independent reviewers approved round 2 after inspecting the complete batch.
The final requested documentation cleanup now names `JsonBodyAdapter` instead
of a future adapter. No behavioral changes followed review. Reviewed production
and test identities (SHA-256; body comment includes the requested cleanup):

- `edgion-waf/src/seclang/body.rs`: `c095ff1ac9d29fbbdd8c34e1ea498ab6abf7fce686ac48a9410ca4d1122ca154`
- `edgion-waf/src/seclang/json_body.rs`: `1ab271028bfc48eec3adf157459ac114f8956ad61afb2305149d133678a0434d`
- `edgion-waf/src/seclang/json_document.rs`: `89aa7e8e82340b0c5dae40265494b26dd2db87a2021ffeca2b69aaf7a3d00e4b`
- `edgion-waf/src/seclang/json_stream.rs`: `aecf562157e9bb81e0c0e3f897de63abb62a71756f7a871be0330496bc7be7ce`
- `edgion-waf/src/seclang/mod.rs`: `a2d14e150e64da4ce6a6ae91e95b676a64c5d997da4afa9c36dc58a500f420bd`
- `edgion-waf/tests/unit/seclang/json_body_test.rs`: `204fa8e453604cb07b5e0988fbe78d6b4b1696b1f69bf941ccb23bf426faa568`

`cargo test -p edgion-waf --lib --quiet` passed 460 tests with 2 existing ignored
cases (exit 0, `/tmp/waf-s7-tests.log`). Workspace check and Clippy are still
running (`/tmp/waf-s7-check.log`, `/tmp/waf-s7-clippy.log`). Agent-doc, SSA,
metrics, Pingora source and Gateway API transform guards passed. Final unit-test
layout and formatting result collection remains pending before commit.

### Next-step audit after S7

The compiler plan and runtime bridge must preserve the three-pass contract:
`select_values` completes selection before `evaluate_member_matches` collects
operator results; `apply_member_match` then applies ordered matches before
`commit_unit_match`. Selection overflow can precede an earlier provisional
predicate failure, and unreachable rules must never publish deferred errors.
A boolean per-member hit cannot encode multiplicity or virtual TX retention.

A fixed Copy subscriber result cannot cover arbitrary runtime rule counts by
itself. A sidecar range is possible only with its own admitted capacity/work
bounds. JsonDocument duplicate removal does not reclaim external records;
a first append-only implementation must charge every provisional fact, even
facts from overwritten subtrees, and treat exhaustion as an independent state
budget. Reclamation needs an explicit ownership seam rather than an assumption.

The next candidate batch is a bounded immutable subscriber plan and multi-rule
fact storage with standalone differential execution tests. Reuse actual
transformation/phrase scratch and preserve `member_match_bytes` accounting.
Audit `tx.target_exclusions` and phase-2 control actions before freezing the
plan: controls outside an otherwise eligible rule may invalidate its selection.
These are pending implementation constraints, not delivered execution support.

### S7 final verification (2026-09-30)

All required gates passed with exit 0 on the final reviewed implementation:

- `cargo test -p edgion-waf --lib --quiet`: 460 passed, 2 existing ignored;
  `/tmp/waf-s7-tests.log`. Later changes were documentation only.
- `cargo check --workspace --all-targets`: `/tmp/waf-s7-check.log`.
- `cargo clippy --workspace --all-targets`: `/tmp/waf-s7-clippy.log`.
- `cargo fmt --all -- --check`, `git diff --check` and staged whitespace check.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract and unit-test layout guards. The transform
  used `/tmp/edgion-p090-005-tools/bin/python3` for its installed YAML dependency.

Two independent reviewers approved both rounds; all requested documentation
corrections are applied. No new dependencies, body cache, Gateway wiring or
execution-class promotion. Full workspace tests and throughput benchmarks were
not run. Eight scoped files are staged; unrelated working changes are excluded.

S7 committed locally as `5eb3afdb`; no push. Only the post-commit checkpoint
remains dirty within this batch; preserve unrelated working changes.

## S8 design and implementation checkpoint (2026-09-30)

The standalone internal bridge compiles bounded subscriber plans from resolved
records, not raw action history. Eligible effective transformations are empty
or repeated lowercase after reset. Candidate operators are static noncapture
phrase rules on one unmodified ARGS/ARGS_NAMES all/literal target. Controls,
chains, dynamic operands and payload-observing actions remain excluded.

Per-request sessions borrow existing matcher scratch and own the body adapter
plus an independently bounded append-only fact arena. Every provisional fact,
including overwritten subtrees, remains charged. No scalar payload enters the
arena. Query facts precede surviving JSON facts. Replay first completes selection
counting, then resolves ordered transform/work/matcher results, then applies
logical TX match charges. A deferred predicate failure can disappear with its
subtree; an independent fact admission failure cannot. Static metadata must
retain its runtime validation limit. Deferred runtime predicate errors are surfaced only when the rule is replayed;
independent speculative resource and invariant failures remain immediate.

The first API is internal to transaction.rs through its json_rules child. It
does not expose unstable interfaces or pretend to execute the phase schedule.
The following integration still needs virtual committed match accounting and
whole-request observer/control admission. Tests use the actual evaluator as an
oracle and retain separate literal limit cases where both paths share helpers.

### S8 first review checkpoint

Both independent reviewers found no correctness blocker in the first complete
implementation. Caller-contract clarifications were applied to source and task03:
matching generation scratch, consistent canonical body/head configuration, TX
base after clearing inherited matches, and the deliberately omitted transaction
schedule/commit/global-retention checks. Eight initial tests passed before later
coverage additions. Additional tests pin ordinary TX base and nonmatch no-commit,
exact fact capacity, and sparse per-scalar subscriber ranges.

Root review identified an avoidable O(values * subscribers squared) replay scan:
each subscriber filtered every fact three times. Before final review, replace
that traversal with sorted per-scalar subscriber lookup and bounded query range
headers, avoiding a separate index allocation. The speculative-work bound is
separate from canonical replayed operator work; neither claims exact CPU counts.
No throughput improvement is claimed without measurement. Final implementation
verification and round 2 are pending.

### S8 final review and test checkpoint

The replay scan correction is implemented: first-fact query range headers and
per-scalar binary search yield at most one fact per subscriber without a separate
index allocation. Both independent reviewers approved the complete final diff
in round 2. They verified range ownership/order, sparse selections, canonical
error precedence, metadata and TX charges, and capacity/lifecycle behavior.
The requested documentation clarification distinguishes deferred runtime errors
from immediate speculative-resource failures.

Reviewed source/test SHA-256:

- `edgion-waf/src/seclang/transaction.rs`: `bbe43ad1cb213b6d8d1f35efcdc7774fbc5823a9953feb82ecda87b90ca90ce5`
- `edgion-waf/src/seclang/json_rules.rs`: `1f0d693f77cb8bb0d24fb029aff795b0e38d785108117e7dde48fb8194739efb`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `b13e307bf03bd446d77762dc8bc5d9f307ccf2828cace3a990cc6380df978708`

`cargo test -p edgion-waf --lib --quiet` passed 471 tests with 2 existing ignored
cases (exit 0, `/tmp/waf-s8-tests.log`). Formatting, diff whitespace, agent-doc,
SSA, metrics, Pingora source and Gateway API transform guards passed. Workspace
check and Clippy are active at `/tmp/waf-s8-check.log` and
`/tmp/waf-s8-clippy.log`; unit-test layout result collection remains pending.

### S9 ownership and transaction integration audit

Read-only audit identifies a required ownership handoff: JsonSubscriberSession
borrows transaction group scratch mutably, so schedule execution needs a consuming
completion operation that detaches owned facts and releases that borrow. Do not
allocate duplicate scratch. Bind completed facts to the same immutable plan and
generation.

The next proposed transaction work adds a private virtual matched-state charge,
mutually exclusive with materialized matched fields. Clone copies it; clear_matches
clears it; recount includes it. A successful reached rule installs replayed logical
match bytes and reuses commit_unit_match; a nonmatch never commits the cleared
clone. Metadata publication needs an explicit representation rather than an empty
BodyPublication. Preserve collection-entry admission, logical request retention,
total retention including scratch/TX, then body-error/publication commit order.
Actual fact/document capacity limits remain separate from reference logical bytes.

The existing phase schedule should remain the sole owner of evaluated-unit limits,
findings, intervention and phase advancement. Detach/release completed fact arenas
after Phase 2, including early denial, while retaining required logical publication
and virtual TX charges for response admission. Before internal route activation,
resolve the actual JSON processor and reject later-phase body readers, global
controls/templates and noncandidate Phase-2 units from that route. These are
eligibility fallbacks, not new configuration rejection rules. Gateway wiring and
classifier promotion remain later work after transaction differential proof.

### S8 final verification (2026-09-30)

All final checks passed with exit 0 on the reviewed source hashes above:

- `cargo test -p edgion-waf --lib --quiet`: 471 passed, 2 existing ignored;
  `/tmp/waf-s8-tests.log`.
- `cargo check --workspace --all-targets`: `/tmp/waf-s8-check.log`.
- `cargo clippy --workspace --all-targets`: `/tmp/waf-s8-clippy.log`.
- `cargo fmt --all -- --check`, working/staged diff whitespace checks.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract and unit-test layout. Transform verification
  used `/tmp/edgion-p090-005-tools/bin/python3` for its existing YAML dependency.

Two independent reviewers approved both full-batch rounds. Root's replay-scan
finding was corrected and the final correction independently reviewed. No new
dependencies, runtime publication route, classifier promotion or Gateway wiring.
No full workspace tests or throughput benchmark were run. Five scoped files are
staged; unrelated task edits remain excluded. The overall WAF goal stays active.

S8 committed locally as `ba89f6e5`; no push. Preserve unrelated worktree edits
and resume from the S9 ownership/TX/schedule audit above.

## S9 transaction integration checkpoint (2026-09-30)

The internal execute_json_body_inputs route derives body policy, query facts,
head size, runtime limits and detection mode from its transaction. Caller inputs
are only the actual processor, borrowed input sequence and separate streaming
resource limits. It requires valid phase order and rejects conflicting published,
prefix/raw-stream or unsupported-generation state before consuming bytes.
This synchronous scoped iterator is a transaction proof seam, not an async
Gateway interface. Future Gateway wiring must persist bounded state across
callbacks without buffering chunk sequences for this helper.

Consuming completion transfers owned facts/subscriber indices and releases the
existing scratch borrow; no duplicate scratch is allocated. Metadata publication
is explicit and follows the original collection-count and logical-retention
admission/commit order. Virtual matches carry count and logical bytes through
clone/clear/recount and reuse ordinary commit checks. Nonmatch leaves previously
committed match charge intact.

Independent oracle review identified retry semantics: existing runtime errors
leave Phase 2 current, preserve earlier unit commits, and allow scheduling to
restart on a later execute_request_body call. Completed facts must remain owned
until success (including deny) or transaction destruction; do not release them
on a retryable phase error. Inputs/admission/publication failures install no
partial body state. Full schedule outcome and response admission are tested,
not only predicate results. Later body-payload/metadata observers must be rejected
from this route before consumption; ordinary configuration remains accepted.

### S9 round-1 implementation review

Both independent reviewers found no correctness blocker in the first full diff.
The oracle reviewer corrected an earlier overbroad observer warning: later
independent MATCHED_VARS readers clear inherited matches before selection, and
chains see only their own new materialized matches. The implementation therefore
rejects unavailable body collections, not those safe matched-state consumers.

Ten transaction differential tests now pass, including full schedule outcomes,
query/duplicate/malformed/detection behavior, publication and runtime caps, retry
state, response admission, later match replacement, AUTO processor resolution,
raw-lane conflict, abandonment/missing/trailing completion and independent source
or fact failure rollback. A fixture initially used forbidden full RESPONSE_BODY;
it was corrected to a supported Phase-4 REQUEST_METHOD condition while retaining
response-window accounting coverage. Final full-library verification is pending.

Root review is also checking completed-state placement: keep ordinary transaction
footprint small rather than embedding a large inactive parser/arena owner inline.
Any cold allocation must be fallible and occur before publication commit. Final
round-2 review follows that decision and its affected tests.

### S9 final implementation checkpoint

Ten targeted transaction tests passed after correcting the invalid response-body
fixture. Additional final assertions verify successful completed-state capacity
release, retry retention, and publication caps including prior committed Phase-1
matches. Root footprint review moved the completed owner from inline transaction
state into a fallibly allocated private 0/1 cold vector. Allocation completes
before publication; attachment itself cannot fail. The ordinary path keeps an
empty vector header. Type-size inspection used the current arm64 metadata; an
older artifact was not treated as a verified baseline or throughput evidence.

Source and tests are ready for full-library/workspace checks and final independent
review. Logs: `/tmp/waf-s9-tests.log`, `/tmp/waf-s9-check.log`,
`/tmp/waf-s9-clippy.log`. These final results are not yet confirmed.

### S9 final independent review checkpoint

Both independent reviewers approved round 2 of the complete final batch. They
verified cold-state ownership, allocation-before-publication atomicity, retry
retention versus success release, prior Phase-1 charge admission, response
accounting and safe later matched-variable readers/chains. No unresolved finding
remains. Reviewed SHA-256:

- `edgion-waf/src/seclang/transaction.rs`: `9f18cbc209ce3cad8ef0ed277382ccc6673db83f996bdb8d1a7ece282ef22e01`
- `edgion-waf/src/seclang/json_rules.rs`: `d0a9a831a784a200c69ae2e8b4b5f44f1a727ea0d9f9202a8eb3ee768003fa03`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `68af57851300eecc7b383acea24a327918751abc6252d186168dd4e7dc8abd97`

Final test/check/Clippy sessions remain active; verify terminal success before commit.

### S10 preparation audit (pending implementation)

The independent ownership audit recommends a persistent internal start/feed API.
Extract owned adapter/facts/query-boundary/runtime state from the borrowed
subscriber session; each feed receives temporary generation/plan/scratch borrows.
Keep the existing transaction scratch and a cold 0/1 collecting/completed owner.
Do not build an accumulated chunk sequence or duplicate scratch. Cover explicit
abandonment, missing/repeated EOS, sticky feed errors, drop cleanup, competing
raw/prefix APIs, and scheduled-error retries before exposing this interface.

Plan descriptors currently scan and allocate per request. A later focused batch
should compile immutable eligibility/descriptors into CompiledGeneration, account
for their compiler-owned memory, and check per-request caps without treating an
unsupported capability as invalid configuration.

Gateway integration remains separate: RequestBodyStreamHandler runs after
upstream headers may already have been sent. Existing WAF admission completes
inside run_request before dispatch. Preserve the boundary documented in
skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md. Use a
canonical pre-dispatch BodyStorage observation seam; do not promote classification
or use the forwarding callback until that lifecycle has been designed and proven.

### S9 final verification and commit gate

Final reviewed source hashes above remain unchanged. Confirmed exit 0:

- `cargo test -p edgion-waf --lib --quiet`: 483 total, 481 passed, 2 ignored;
  log `/tmp/waf-s9-tests.log`.
- `cargo check --workspace --all-targets`: log `/tmp/waf-s9-check.log`.
- `cargo clippy --workspace --all-targets`: log `/tmp/waf-s9-clippy.log`.
- `cargo fmt --all -- --check` and `git diff --check`.
- `python3 cicd/checks/validate_agent_docs.py`.
- `python3 cicd/checks/check_ssa_force.py`.
- `python3 cicd/checks/check_metrics_inventory.py`.
- `python3 cicd/checks/check_pingora_source_policy.py`.
- `/tmp/edgion-p090-005-tools/bin/python3 cicd/checks/generate_gateway_api_transform_contract.py --check`.
- `python3 cicd/checks/check_unit_test_layout.py`: 958 mounted files across 9 crates.

Both independent full-diff review rounds approved; no unresolved blocker. The
mandatory design-rule red rows were scanned against this scoped diff. There are
no new dependencies, public configuration fields, log/metric sites or Gateway
routing changes. This request-local owner is released through its transaction
lifetime, with explicit success-release and retry-retention tests. No throughput
claim or complete Gateway integration claim is made. Full workspace tests were
not requested and were not run. Only the five S9 files will be committed; unrelated
CRD, mesh and plugin task edits remain excluded. No push.

S9 committed locally as `c4b0ec65`; no push. This post-commit checkpoint is
intentionally retained for the next batch. Resume with the S10 owned collector
and per-feed lifecycle, preserving the S9 retry/publication invariants.

## S10 persistent collection checkpoint (2026-09-30)

Prior turn made progress: S9 committed as c4b0ec65 with terminal verification.
Current scope retains the same five files; unrelated task edits remain excluded.
Implement owned collection with temporary generation/scratch borrows. start admits
and binds canonical query/policy; feed processes borrowed chunks and explicit EOS;
finish publishes atomically and runs the ordinary schedule. Deferring publication
until finish preserves trailing-input rejection in the existing iterator proof
route. Feed errors release provisional arenas and remain sticky until an explicit
new start. Runtime schedule errors keep completed facts for ordinary retries.
No chunk list, duplicate scratch or Gateway forwarding-hook wiring is permitted.

### S10 implementation and first review checkpoint

Production refactor is complete in transaction.rs/json_rules.rs. A shared
JsonCollectionCore serves borrowed test sessions and the owned transaction
collector. The transaction reuses its cold 0/1 owner across collecting/completed
states, retains sticky failure outside released arenas, and guards competing
materialized/prefix/raw entrypoints. Generation cloning is no longer necessary.

Six persistent-API regression tests supplement the existing differential matrix:
interleaved single-byte/empty feeds with source-buffer reuse and scratch identity,
abandonment and explicit restart, collecting/sealed lane conflicts, missing and
repeated EOS cleanup, shared-generation lifetime across cancellation/completion,
and postpublication transport misuse preserving schedule retries. Final results
are pending; targeted JSON tests run in session 69918, log
/tmp/waf-s10-targeted.log. Both independent reviewers have started round 1 on the
full implementation/test diff. No new public API or Gateway capture change.

### S10 first review results and final verification start

Both independent reviewers approved the first full diff with no blocker. The
contract reviewer requested direct raw-chunk guard coverage; the active/sealed
and failed-state tests now call that entrypoint. Targeted JSON tests passed
80/80 before these additional assertions. Final verification is running:
WAF library session 69924 (/tmp/waf-s10-tests.log), workspace check 66200
(/tmp/waf-s10-check.log), workspace Clippy 18468 (/tmp/waf-s10-clippy.log).
No process result is assumed before terminal confirmation.

Round-2 source identity:

- `edgion-waf/src/seclang/transaction.rs`: `5c4495d9cf3cfb1431f83a488338b8183e5473e018149a7be4d0284cae01e646`
- `edgion-waf/src/seclang/json_rules.rs`: `3e15e3bf3eb2015c6132542468840b098a9a5ae8fe3c91874d93bca279a5a391`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `ae65a767bad467ca1d6a767d26b32cf1daf2e0e8ca84ecf7f43182dcfad4e7e5`

### S10 lint correction

Final WAF library tests passed: 487 passed, 2 ignored (489 total). Workspace
check surfaced three unfulfilled dead_code expectations on start/feed/finish:
the proof helper references all three. Removed only those redundant attributes;
retain the expectation on the still-unselected proof entrypoint. Both round-2
reviewers had approved the preceding implementation; this lint-only correction
needs their final confirmation. In-flight build results remain tied to their
original source snapshot; rerun affected compilation/lint after they terminate.

Both final reviewers confirmed approval after the lint-only correction. Updated
transaction.rs SHA-256:
`1fcc61cc7fcc28bb0dbbb2c7753c71b04a0e696042d8faa8369f8dd34ba17ee1`.
Other reviewed source hashes remain unchanged.

### S11 preparation audit (pending implementation)

The compiler audit recommends a generation-owned immutable JSON capability plan.
Move eligibility and subscriber descriptors into a compiler-owned module; publish
Unsupported or an eligible descriptor vector only after canonical actions,
targets and transformations are resolved. Unsupported semantics must not reject
otherwise-valid configuration. Check all eligibility before allocating descriptors.

Charge descriptor bytes/records using MatcherCompileWork and existing retained,
generation and peak caps; allocation/arithmetic failure stays a compiler resource
error, never Unsupported. Use fallible reserve and allocation-failure injection.
At request start, check immutable plan count/bytes against request caps and borrow
its descriptor slice. Remove per-start generation scans, descriptor allocations,
and completed-fact descriptor ownership without adding an Arc clone. No current
CompiledGeneration serde boundary exists; keep the plan private in the canonical
compile/publish pipeline.

Tests must cover capability exclusions, empty plans, exact compiler/request caps,
allocation failures, and shared generations with independent transaction scratch
and facts. Preserve all differential/lifecycle tests. This audit identifies
removable work; it is not measured throughput evidence or Gateway integration.

Final Clippy session 18468 exited 0 using the corrected source (the compiler lock
was acquired after the lint edit); /tmp/waf-s10-clippy.log contains no warning.
Original check 66200 exited 0 with the now-fixed lint warnings. Fresh final-source
check 16488 is still active. Format/diff, docs, SSA, metrics, Pingora source,
transform contract and unit-test layout guards all exited 0; layout reports 958
mounted files across 9 crates. No full workspace test run was requested or run.

### S10 final commit gate

Final-source `cargo check --workspace --all-targets` exited 0 in session 16488,
with no warnings. `cargo clippy --workspace --all-targets` also exited 0 on the
corrected source. `cargo test -p edgion-waf --lib --quiet` passed 487 tests with
2 ignored; subsequent edits removed only non-test lint attributes. Format,
diff, agent-doc, SSA force, metrics inventory, Pingora source, Gateway API
transform contract and unit-test layout guards passed. Both independent reviewers
approved two full-diff rounds and confirmed the final lint-only correction.

The design-rule red rows were scanned against this diff: request-local ownership
remains bounded, failure cleanup and admission are explicit, existing body-cache
and pre-dispatch boundaries stay intact, and no new config/schema/transport,
logging, metrics or dependency surface was added. No measured performance or
Gateway integration claim is made. Stage only transaction.rs, json_rules.rs,
transaction_test.rs and tasks 01/03. Unrelated task changes stay excluded. No push.

S10 committed locally as `b7c99397`; no push. This recovery update is deliberately
left for the next batch rather than creating a circular checkpoint commit.

## S11 immutable plan checkpoint (2026-09-30)

Prior turn made progress: S10 committed as b7c99397 with terminal verification.
Move descriptor allocation and full-generation eligibility scanning to compilation.
Unsupported and eligible-empty remain distinct; compiler resource failures remain
errors. Request lookup checks caps against the shared plan allocation. Unsupported
capability is checked before request caps, unlike the old interleaved eligibility
and allocation loop that could report an early cap before a later unsupported
unit. This internal error-precedence change needs a dedicated regression test.

### S11 implementation checkpoint

Production migration is complete, including new compiler child json_plan.rs.
The two-pass compiler plan validates before allocation, charges actual descriptor
capacity through MatcherCompileWork and updates final generation accounting.
Runtime lookup is named for_generation and checks immutable count/capacity; facts
no longer own descriptor vectors. Root added allocation-failure/unsupported-empty,
descriptor-order, exact shared-cap, stable plan identity and interleaved shared
transaction tests. Existing eligible-plan final compiler-limit tests continue to
exercise exact and one-below owned/generation/peak caps.

The first compile found a missing subscribers initializer in the borrowed test
ReplayView; root restored its plan slice. No test was removed or weakened. Full
library tests restarted after terminal failure (session 16497,
/tmp/waf-s11-tests.log). Both independent reviewers started round 1 on the complete
production/test diff, including the new untracked module. Results remain pending.

### S11 first review findings and final verification start

Both independent first-round reviews found no source defect but agreed that
aggregate memory assertions did not independently prove descriptor charging.
Root added a direct accounting-delta test with nonzero existing owned/matcher/
source/work costs. It checks exact actual-capacity increments and exact success,
then forces allocation failure while testing each first-excess owned/generation/
peak budget, proving the budget error occurs before reservation. The original
end-to-end final-generation threshold test remains unchanged. An additional
interleaved old/new generation test verifies inflight descriptor/matcher isolation.

Initial full suite passed 492 tests with 2 ignored. Final full-library session
8665 (/tmp/waf-s11-tests-final.log), workspace check 87980
(/tmp/waf-s11-check.log), and Clippy 49608 (/tmp/waf-s11-clippy.log) are running.
Final review identity (baseline b7c99397 plus these files, including new module):

- `edgion-waf/src/seclang/compiler.rs`: `d3ac1ae351df8ee00fc52511c1fa82f34a71a96b154f1801b5b088d517aa1dd0`
- `edgion-waf/src/seclang/json_plan.rs`: `04b06ea95a411480cf474d128f584ae85e07ab93e3a227c3162c59b78f67237c`
- `edgion-waf/src/seclang/json_rules.rs`: `80ca7828a134f4d5f0bdfc0f05010264d0bdc4cdc2a12d1404e2ce354442b6a2`
- `edgion-waf/src/seclang/transaction.rs`: `0b51c63da702a430ce565b302df877d24c645f1568d6890334ba716cfc00832e`
- `edgion-waf/tests/unit/seclang/compiler_test.rs`: `b9af13f0afffdf974ea1fcaa28144945830cc1139d42b14385a4ef52198794aa`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `a347c53773714f7060cd5e9b204fcd95a8bcd8a6e060d48a1caa774502030f9f`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `953234d8a3b7b38a3027428c31fa5d2016aa2b1ce146c2d3bbb3c8caeab4af39`

### S11 final review result

Both reviewers approved round 2 against all seven recorded source/test hashes.
They confirmed the new exact-delta/preallocation boundary test closes their shared
R1 verification finding; inflight generations keep separate immutable plans and
transaction scratch/facts. Final library session 8665 exited 0 (494 passed,
2 ignored). Format/diff, documentation, SSA force (1512 files), metrics inventory
(103 metrics), Pingora source, transform contract and unit-test layout (958 mounted
files across 9 crates) all passed. Workspace check/Clippy remain active; no result
is presumed. No performance benchmark or full workspace test run was performed.

### S12 preparation audit (pending implementation)

Next coherent step is mixed Phase-2 scheduling before Gateway activation. Keep
existing JSON subscriber restrictions, but allow units whose every member is
body-independent according to CompiledGeneration::member_requires_request_body.
Leave them on the ordinary standalone/chain evaluator. This includes static
header/URI/query-only/TX predicates, targetless actions, static TX mutation,
captures, wholly body-independent chains, and existing skip/skipAfter behavior.
Keep global controls and dynamic templates excluded; mixed body-dependent chains
still require a separate proof. Eligible-empty remains distinct from unsupported.

The compiler should emit descriptors only for eligible JSON units and prove that
all omitted units are body-independent. Runtime dispatch may then replay a present
descriptor or evaluate an ordinary unit; absence must never silently authorize an
unproven body reader. Preserve the existing scheduler, including skipping deferred
errors, deny/detection ordering, findings and evaluated-unit limits.

Verify virtual TX accounting across ordinary match/nonmatch/action transitions,
static TX/capture effects before later JSON replay, skipped deferred JSON errors,
chains, retries and all relevant budget boundaries against complete publication.
Gateway pre-dispatch capture observation and production budget calibration remain
separate: current forwarding callbacks may already expose upstream headers.

### S11 test lint correction

Workspace check 87980 exited 0. Clippy reported manual_slice_size_calculation in
the new shared-capacity test. Replaced len times element size with size_of_val;
only the equivalent test expression changed. Both reviewers confirmed approval.
Final test-file SHA-256: `0aafcab10d8cc46ff8f997e43da8dffb99b0090b06d0a11e692b6696a7391db5`.
Other reviewed hashes are unchanged. Original Clippy remains active; final-source
workspace check and Clippy are queued in /tmp/waf-s11-check-final.log and
/tmp/waf-s11-clippy-final.log. Verify terminal results before commit.

### S11 final commit gate

Final-source `cargo check --workspace --all-targets` and
`cargo clippy --workspace --all-targets` both exited 0 without warnings. The final
full WAF library run passed 494 tests with 2 ignored (496 total); only the
semantically equivalent test lint expression changed afterward. Format/diff,
agent-doc, SSA force, metrics inventory, Pingora source, Gateway API transform
contract and unit-test layout guards all passed. Two independent full-diff review
rounds and final test-expression confirmation approved the committed source.

Mandatory design-rule red rows were scanned: generation data stays immutable and
private, new capacity is compiler-budgeted, request state remains bounded and
request-local, existing body cache/admission boundaries are preserved, and there
are no new dependencies, schemas, network paths, logs or metrics. Scoped staging
is nine files: four production, three existing tests, and tasks 01/03. Preserve
all unrelated task edits. No push, classifier promotion or throughput claim.

S11 committed locally as `432da908`; no push. This post-commit checkpoint is
intentionally retained for the next batch.

## S12 mixed Phase-2 checkpoint (2026-09-30)

Prior turn made progress: S11 committed as 432da908 with terminal verification.
Permit wholly body-independent Phase-2 units via canonical member dependency
analysis; keep the ordinary evaluator and schedule for these units. JSON units
retain existing restrictions and get immutable descriptors. Absent descriptors
must be proven ordinary rather than silently falling back for a missing body
subscriber. Global controls/macros and later body readers remain excluded.
Root tests must compare full outcomes, ordinary TX entries/captures and logical
retention across matches/nonmatches, targetless actions, captures/chains, skips,
errors/retries and enforcement/detection mode.

### S12 implementation and first review checkpoint

Production changes are complete in json_plan.rs, json_rules.rs and transaction.rs.
Compiler analysis proves every member of ordinary units body-independent; sparse
JSON descriptors preserve member order. Runtime searches a descriptor once and
replays it, or explicitly proves ordinary eligibility before the original evaluator.
The scheduler is unchanged. Global macro/ctl and later-body exclusions remain.

Root differential tests now compare all ordinary TX entries and capture keys,
including after retries. New scenarios cover static actions/predicates, ordinary
match/nonmatch replacement, captures and chains with own MATCHED_VARS, skip and
skipAfter over deferred errors, denial versus detection-only, runtime caps,
zero-subscriber generations, and repeated committed actions before retry errors.
The plan test covers sparse descriptor IDs/caps and unsafe mixed chains/macros/
controls. A private-capability corruption test verifies missing body descriptors
fail closed rather than falling through to unavailable body values.

Initial JSON-targeted run passed 94 tests; targeted WAF Clippy exited 0 without
warnings. The missing-descriptor regression was added afterward and needs final
verification. Both independent reviewers are auditing the full first-round diff.

### S12 first review results and final identity

Both independent reviewers approved round 1 with no source defect. Root added
their suggested defensive missing-descriptor test and exact retained-byte boundary
with long ordinary TX state and regex captures before JSON matching. The unchanged
ordinary schedule remains the oracle for failures and retries. Final source/test
identity for round 2 (baseline 432da908):

- `edgion-waf/src/seclang/json_plan.rs`: `3999f577353c9b06cf759a75e6d330d78b95b84a7a48e80c3e63785b77fb23f9`
- `edgion-waf/src/seclang/json_rules.rs`: `887b0dd67819ef5093c52da05663048673b04426bd45988657350ae243545107`
- `edgion-waf/src/seclang/transaction.rs`: `a8f08a1f66686414d1ad8488bcf9b4bdf61e77648e11c9af14df4189dd6eb751`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `afc77ce9f52e9693f99738f392aa9f7989c27077e22ff87174ab82af6336b8f9`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `63db6852cca488dac2ae60efb77cb777fa105cfa19ae454d02a91012c35486ae`

### S12 final review and library verification

Both reviewers approved round 2 against all five recorded hashes. They verified
ordinary/JSON dispatch, absence-proof failure, retained-byte boundary, TX/capture
state and retry behavior. Final library session 89027 exited 0: 505 total,
503 passed, 2 ignored. Final targeted Clippy session 8832 exited 0 without warnings.
Workspace check/Clippy 75645/14276 are active; do not infer success from targeted
checks. Logs are /tmp/waf-s12-check.log and /tmp/waf-s12-clippy.log.

### S13 preparation audit (pending implementation)

The next bounded eligibility improvement can narrow the global dynamic-template
exclusion without changing the runtime evaluator. Retain global ctl exclusion.
Phase 1 may use ordinary macros before body publication; copied query/header/TX
and capture values stay materialized. Ordinary Phase-2 units still require every
member to pass canonical member_requires_request_body, which includes operator,
metadata and setvar templates. Later-phase eligibility must use that same member
predicate rather than its current target-only scan, or body macros in actions
could evade the metadata-only publication boundary.

JSON subscriber templates must remain static: virtual matched state stores charge,
not payload bytes. Ordinary evaluate_unit clears inherited matches before operator
selection; actions/chain children use their own materialized matches and metadata
expands against staged TX. Nonmatch preserves charges without exposing payload.
JSON subscribers cannot capture or assign TX, so they add no hidden TX provenance.
Revalidate this proof against actual macro expansion and commit timing in S13.

Tests should cover Phase-1 query/header copies; ordinary dynamic operators, keys,
values and metadata; captures and own-chain MATCHED macros; independent matched
macros after JSON and in later phases; rejection of post-Phase-1 body macros in
all template positions; static subscriber metadata and ctl exclusions; expansion
limits, skipped errors, denial/detection and retries. Gateway capture observation
and measured budget selection remain separate prerequisites for activation.

### S12 final commit gate

Confirmed terminal success for `cargo check --workspace --all-targets` and
`cargo clippy --workspace --all-targets`, both without warnings. Final library
run `cargo test -p edgion-waf --lib --quiet` passed 503 tests with 2 ignored.
Format/diff, agent-doc validation, SSA force (1512 files), metrics inventory
(103 metrics), Pingora source policy, Gateway API transform contract and unit-test
layout (958 mounted files across 9 crates) passed. Both independent review rounds
approved final source/test hashes, and all recommended boundary tests are included.

Design-rule red rows were scanned: no extra cache, body aggregation, dynamic
runtime policy parsing, logs, metrics, schema or dependencies were added. The
canonical scheduler/body-dependency predicate remains authoritative; unsupported
body consumers fail closed. Stage exactly seven files: three production, two
existing tests and tasks 01/03. Preserve unrelated changes. No push, Gateway
activation, execution-class promotion or measured throughput claim.

S12 committed locally as `4fdd1270`; no push. This post-commit checkpoint is
intentionally retained for the next batch.

## S13 template eligibility checkpoint (2026-09-30)

Prior turn made progress: S12 committed as 4fdd1270. Replace global macro rejection
with exact per-member restrictions. Later phases use canonical body-dependency
analysis including templates; ordinary Phase-2 units already do. Phase-1 query
macros run before body publication. JSON subscriber operator/action templates
remain static because replay has no matched payload. Global ctl exclusion stays.
Independent design review confirms the only direct body-reading macro source is
Args and that canonical dependency checks cover operators, action templates and
setvar key/value. Root will test actual expansion behavior and limits, not merely
eligibility flags.

### S13 implementation and first verification checkpoint

Only json_plan.rs changes production behavior. It replaces global macro rejection
with canonical later-phase member dependency checks and explicit static-template
validation for JSON subscribers. No runtime evaluator change or new API is added.

Five new tests exercise query-to-TX and dynamic ordinary operators/keys/metadata,
capture/chain matched macros, later phases following virtual matched state,
expansion failure with staged rollback/skip/deny/detection/retry, all post-Phase-1
body-macro positions including chain children, and static subscriber metadata.
The existing transaction fixture now accepts optional frontend limits so expansion
limits are tested through real compilation instead of mutating compiled metadata.
Targeted JSON tests and WAF Clippy are running in /tmp/waf-s13-targeted.log and
/tmp/waf-s13-clippy-targeted.log. Results are not yet confirmed.

### S13 first review and corrected fixture results

Both first-round reviewers found no production blocker. Actual targeted execution
initially reported 99 passed and 2 failed: Msg is not legal on a chain child, and
semantic expansion limits do not set the compiled runtime template limit. Root
changed the child fixture to setvar with an ARGS macro and configured the actual
CompilerLimits.max_template_runtime_bytes at 64. No assertion was relaxed; the
80-byte expansion must fail with ValueByteLimit and leave its staged write absent.
Corrected targeted session 58478 exited 0: 101 passed. Initial targeted Clippy
78316 exited 0. Final full library/check/Clippy logs are /tmp/waf-s13-tests.log,
/tmp/waf-s13-check.log and /tmp/waf-s13-clippy.log; terminal results pending.

Final identity for round 2 against baseline 4fdd1270:

- `edgion-waf/src/seclang/json_plan.rs`: `b7b75eb10bb894db4f9d3280bbfcb5dda69988d2207f173bd49048a3c374e287`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `aaed760711ad1885b38c3b4222efed6de6dccddb9b78ee9112bd15874cce06c7`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `f21d5a71741014651324b0e9f2bd6051f0cbe3872553d041806427245206edac`

Corrected targeted test log is /tmp/waf-s13-targeted-final.log (101 passed);
/tmp/waf-s13-targeted.log deliberately preserves the initial 99-pass/2-failure
fixture result. Final full library session 7649 exited 0: 508 passed, 2 ignored
(510 total), log /tmp/waf-s13-tests.log. Workspace check 19614 exited 0 without
warnings. Clippy 67237 and layout 35621 remain active; other script/format guards
passed. The contract reviewer approved round 2 against all recorded identities;
second reviewer result is still pending.

### S13 final commit gate

Both final reviewers approved all three recorded source/test identities. Final
`cargo test -p edgion-waf --lib --quiet` exited 0 with 508 passed and 2 ignored.
`cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets`
exited 0 without warnings. Format/diff, agent-doc, SSA force (1512 files), metrics
inventory (103 metrics), Pingora source, Gateway API transform contract and
unit-test layout (958 mounted files across 9 crates) all passed.

Design-rule red rows were scanned: canonical dependency/evaluation remains reused,
no body cache/aggregation or new runtime macro implementation is introduced,
static JSON template restrictions fail closed, and no dependency/schema/log/metric
surface changes. Stage only five scoped files. Preserve unrelated task edits;
no push, full-workspace test run, Gateway activation or measured speedup claim.

### S14 readiness audit (pending implementation)

The first real full-mode integration can use existing sealed BodyStorage snapshots
without a new transport observer. In plugins/runtime/session_adapter.rs,
ensure_request_body captures/reuses canonical storage; read_request_body additionally
calls read_all. request_body_snapshot_reader returns the current logical snapshot,
including edits. BodySnapshotReader::next_chunk in request_body/storage.rs emits
at most 64 KiB: memory uses shared slices, disk uses bounded reads under existing
storage timeout/concurrency controls. Keeping collection/finish inside awaited
WafPlugin::run_request before returning preserves pre-dispatch admission.

This would avoid selected requests' contiguous disk materialization and complete
WAF value publication, while retaining the existing full request cache. It does
not eliminate capture or provide transport streaming. Existing ensure_request_body
returns cached length without rechecking its optional capture cap; preserve and
verify canonical physical and WAF logical body-limit handling rather than assuming
that argument is a fresh snapshot admission check.

Before activation, define the engine eligibility/start/feed/finish API and typed
errors, independent document/fact/plan budgets, and failure mapping including
DetectionOnly malformed behavior. Benchmark complete versus structured evaluation
across memory/disk, subscriber count, duplicate-heavy and malformed JSON. Snapshot
readers pin a version but do not freeze future edits: preserve existing full-mode
mutation semantics, not prefix-only freezing. Unsupported admission may choose the
ordinary route before collection; do not silently restart after resource/schedule
failure. Tests must prove no read_all, exact outcomes, edited snapshots, disk error,
cancellation, bodyless handling and denial before backend exposure. Prefix mode
remains unchanged. This is readiness evidence, not implemented Gateway behavior.

S13 committed locally as `aaabc196`; no push. This post-commit checkpoint is
intentionally retained for the next batch.

## S14 API and measurement checkpoint (2026-09-30)

Recovery verified HEAD aaabc196 and scoped post-commit checkpoint; unrelated
CRD/mesh/plugin tasks are preserved. Existing snapshot integration evidence remains
valid. The next bounded batch exposes the already-tested persistent JSON lifecycle
with explicit independent budgets and typed errors, and adds a public-API engine
benchmark. It does not enable Gateway selection, choose production budget defaults,
or claim disk IO measurements. Benchmark fixtures compare canonical outcomes
before timing and cover subscriber counts, duplicate keys, malformed documents,
and chunk sizes. Real BodyStorage memory/disk measurements remain a later gate.

### S14 implementation and first review

The engine now exports three existing transaction lifecycle methods with explicit
limits and canonical JsonExecutionError/JsonExecutionLimit names. No collector or
facts are exposed and no default execution budgets are introduced. Documentation
separates initial Unsupported from sticky failure/restart, sealing from publication,
and DetectionOnly malformed behavior from independent resource failures.

The external consumer target tests success parity, initial unsupported admission,
plan caps, sticky source errors, rejected restart, missing/trailing completion,
and scheduled retry. Its five tests passed before root strengthened the query
fixture to miss, ensuring body matching itself determines the successful deny.
The full library passed 508 tests with 2 ignored; targeted Clippy passed.

Both independent first-round reviewers found no API correctness blocker. Their
benchmark findings were fixed: mutable closure binding, an explicit 1 GiB
benchmark-only speculative work cap for shared-group scaling, and nearest-rank
quantiles instead of a lower order statistic that hid the maximum in small samples.
No production budget is inferred from this benchmark envelope.

The engine benchmark covers 4 documents, 3 subscriber counts, 2 modes and 3 lanes
(72 rows); it asserts canonical outcomes, logical retention, phase and body error
before timing. Initial smoke and 31-sample runs passed, but ran alongside builds
and precede final quantile/fixture assertions. They are preliminary only. Structured
execution has mixed results on these resident-memory fixtures and is slower for overwritten
values and late malformed input; do not infer a speedup, disk benefit or physical
memory measurement from logical retained-byte equality. Final isolated timing and
workspace gates remain pending. Logs: /tmp/waf-s14-{bench,tests,check,clippy}.log.

Final source identity for second-round review against aaabc196:

- `edgion-waf/Cargo.toml`: `2156ca9c34257bc3269d53bd93be062189c5e10ccb17a075cec55ed01c024f80`
- `edgion-waf/src/seclang/compiler.rs`: `677f72141743da06c0986e971ce6c4895e6c23d9b083f7f9176c989a3eb6075b`
- `edgion-waf/src/seclang/json_rules.rs`: `7b0a30e9f3c8af1531187985904b174f2ee5a529b3a52a08ed645db9a6435338`
- `edgion-waf/src/seclang/mod.rs`: `7b42978c7f063f326dc4848d2656cb3456df7c460df93e9568766b13bde87e82`
- `edgion-waf/src/seclang/transaction.rs`: `79aa17029d23e937c5120c0197aaa15947011bdcaa5f25a851ae5eb07498bee4`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `646e6ec4ec555ca67011cbec873f6672b2a822502d6c7aab9a61a04ec0e6d099`
- `edgion-waf/tests/unit/seclang/transaction_test.rs`: `1b51eb52e9c1406b535322b075230f7374851e559de0ce0fe92b698ba57c4af2`
- `edgion-waf/tests/json_execution_api.rs`: `fbf5b66a77a163d86ed6b158634e7f203abc75ad8fad1b3615d048868fb4fd0d`
- `edgion-waf/benches/json_execution.rs`: `adcea950747f69b6df5594a946b09fa66abd85b9e896fd64f1114bbe80e41349`

### S15 readiness audit (not implemented)

Facts::evaluate currently transforms a scalar and scans its entire compiled matcher
group once per subscriber. evaluate_matcher_operand then iterates all matched group
IDs to find the member range. The benchmark intentionally includes equal target/
transform groups, so identical scalar work repeats. Compiler grouping includes
phase, canonical targets and transforms, capture/multimatch and byte view.

A candidate bounded optimization is one scalar-local last-group result: transformed
length or deferred transform failure plus existing scratch match bits. Equal groups
within one scalar imply identical input/transform semantics. Preserve each current
per-subscriber budget charge, reservation and error order, including cache hits;
retain no payload, extra vector or generation-sized table. Consider a checked
pattern-range lookup shared with the ordinary evaluator. Independently verify
alternating groups, ARGS versus ARGS_NAMES, selectors, phrase ranges, negation,
transform caps, exact work boundaries and duplicate replacement. Benchmark both
lanes again because shared result lookup also affects the reference lane.
Overwritten values and late malformed input still require provisional work; this
candidate does not eliminate those costs or justify Gateway activation.

Both second-round independent reviewers approved all nine exact hashes without
blockers. Workspace Clippy 70302 and formatting 12016 exited 0. Workspace check
18158 and final external API test 14776 remain active. Final targeted Clippy was
queued to cover the final quantile/docs/query changes made during the earlier
workspace run. No new implementation changes are planned in this batch.

### S14 terminal verification

Workspace check 18158, workspace Clippy 70302, final API test 14776 (5 passed),
and final targeted Clippy 89343 all exited 0 without warnings. Full library
verification remains 508 passed, 2 ignored. Format, doc consistency, SSA, metrics,
Pingora source, transform contract and unit layout passed. The final 31-sample
release benchmark is running alone in session 61762, log
/tmp/waf-s14-bench-final.log, after all test/build jobs completed.

### S14 isolated measurement and commit gate

Final benchmark session 61762 exited 0: 72 rows, all parity assertions passed,
31 timed operations per row plus 3 warmups. Command:
`cargo bench -p edgion-waf --bench json_execution`; smoke mode is
`EDGION_WAF_BENCH_SMOKE=1 cargo bench -p edgion-waf --bench json_execution`.
Host: aarch64 macOS; release profile; no concurrent agent build/test jobs. This is
one local process/run, not stable tail-latency or production throughput evidence.
Input is already resident; timing includes transaction initialization and drop,
excludes compilation and Gateway capture/IO. Percentiles use empirical nearest
rank. Budgets are benchmark-only envelopes; logical retained bytes are reference
accounting, not allocated/peak memory. Disk and physical-memory measurement remain
pending before production calibration.

Selected final p50 results, RuleEngineMode::On, in microseconds (complete versus
structured with 64 KiB chunks):

| Document | Subscribers | Complete us | Structured us | Structured / complete |
|---|---:|---:|---:|---:|
| wide_hits | 1 | 359.75 | 835.50 | 2.32 |
| duplicates_last_miss | 1 | 77.29 | 1573.92 | 20.36 |
| malformed_tail | 1 | 75.62 | 822.25 | 10.87 |
| large_scalar | 1 | 562.38 | 2899.75 | 5.16 |
| wide_hits | 8 | 2384.92 | 2133.88 | 0.89 |
| duplicates_last_miss | 8 | 96.17 | 3987.33 | 41.46 |
| malformed_tail | 8 | 74.62 | 2029.50 | 27.20 |
| large_scalar | 8 | 4203.00 | 6384.75 | 1.52 |
| wide_hits | 32 | 10443.71 | 7776.25 | 0.74 |
| duplicates_last_miss | 32 | 99.75 | 14644.46 | 146.81 |
| malformed_tail | 32 | 74.33 | 7415.00 | 99.75 |
| large_scalar | 32 | 17069.92 | 19499.62 | 1.14 |

The baseline shows mixed CPU results; overwritten scalars and late malformed input
are especially costly because speculative work is discarded. No general speedup
claim or default activation follows. The next batch should test scalar-local
shared-group reuse against this baseline, preserving all canonical budget/error
semantics. Final raw log remains /tmp/waf-s14-bench-final.log.

All required gates and two independent full-diff review rounds passed. The final
nine code/test identities above still match. Red design rows checked: no new
production cache, dependency, schema, logs/metrics, response aggregation or runtime
policy defaults; explicit bounded request state and existing generation ownership
remain canonical. Stage these nine files and this task only; preserve unrelated
changes. No push or full-workspace test run.

S14 committed locally as `19d9e1dd`; no push. This post-commit checkpoint is
intentionally retained for the next batch. Overall WAF goal remains active.

## S15 scalar-local reuse checkpoint (2026-09-30)

Previous goal turn made concrete progress: S14 committed as 19d9e1dd with public
API and measured parity baseline. Recovery verified HEAD and unrelated dirty task
state. S15 now implements scalar-local reuse of equal-group transformation and
match results, preserving every subscriber budget charge and reservation order.
Production ownership: matcher.rs, transaction.rs, json_rules.rs by implementer;
root owns regression tests and this checkpoint. Independent compiler-contract
audit is active. No Gateway activation or production budgets are changed.

### S15 implementation and first review

Production now retains a single last-group transformed length/error within each
scalar evaluation and reuses that group's existing scratch result. It retains no
payload or new vector. Compiler group keys include canonical target/transform
plans, phase, capture/multimatch and byte view. Matcher and ordinary evaluator use
one checked member-range query. Every original subscriber work charge and fact
reservation remains in order, including reuse hits. A const-generic disabled-reuse
oracle keeps the same implementation available to private tests.

Two tests add coverage: scalar reuse compares complete facts, partial failures,
capacity and work against disabled reuse at every exact work boundary, fact counts
and retention boundaries in both matcher modes; matcher range checks cover 130
patterns, word boundaries, empty/invalid ranges, scratch mismatch and fresh scans.
The scalar fixture also compares complete-body replay at every byte split for
duplicate/nested values, selectors, ARGS/ARGS_NAMES, interleaved groups, negation,
multiple phrase ranges and transformation resets/deferred errors.

Both first-round independent reviewers found no implementation blocker. Root added
the suggested transformation-reset cases. Initial targeted commands failed to
compile because the new test borrowed scratch while a result view was live; root
split the assertions without changing production APIs. Original failure logs are
/tmp/waf-s15-{reuse-test,range-test,targeted-clippy}.log. Corrected full library and
targeted Clippy both exited 0; library has 510 passed and 2 ignored (512 total).
Final logs: /tmp/waf-s15-tests.log and /tmp/waf-s15-targeted-clippy-final.log.
External API tests and isolated benchmark will run before workspace gates.

Final source identity for S15 second review against 19d9e1dd:

- `edgion-waf/src/matcher.rs`: `987141b500b4205c7e05b79783c5d8b9407c0cf6c0e3483a08654943d6b0ff88`
- `edgion-waf/src/seclang/json_rules.rs`: `51f3ab196f54232e60550cd4d60c4b9b3dcb99d610e27894eb920f757523aa21`
- `edgion-waf/src/seclang/transaction.rs`: `8363ee47297ffff55bfb6fad01caa47a315f09057a4e9846c84b597b94080314`
- `edgion-waf/tests/unit/matcher_test.rs`: `0b1ec1891138535429b6afb38318d458b289ac2433cf5e73442c82cf0a1341d8`
- `edgion-waf/tests/unit/seclang/json_rules_test.rs`: `7b85c2f0d9bc49ff2c39698d717a95fe00aa977c42b58e029b2324bd3b675ffd`

### S15 benchmark and final review

Both independent second-round reviewers approved all five exact hashes without
blockers. Final external API tests passed all 5. Isolated release benchmark 78874
exited 0 with 72 rows and all parity checks passing, 31 samples per row; no
concurrent agent build/test jobs. Raw log: /tmp/waf-s15-bench.log. Same source,
fixtures, envelopes and command as S14. Compare CPU p50 only, not stable production
tails or physical memory; these are separate local runs with ambient system noise.

Selected RuleEngineMode::On results, 32 subscribers and 64 KiB chunks:

| Document | S14 structured us | S15 structured us | S14 / S15 | S15 structured / complete |
|---|---:|---:|---:|---:|
| wide_hits | 7776.25 | 1445.92 | 5.38 | 0.14 |
| duplicates_last_miss | 14644.46 | 1955.12 | 7.49 | 20.21 |
| malformed_tail | 7415.00 | 981.92 | 7.55 | 14.15 |
| large_scalar | 19499.62 | 2840.46 | 6.86 | 0.17 |

Repeated-group work is substantially reduced on these fixtures. Duplicate-heavy
and malformed-tail structured inputs remain slower than canonical complete parsing,
because provisional scalar work is unavoidable before duplicate replacement/EOS.
This does not enable Gateway selection or determine production budget defaults.
Workspace check 84464 and Clippy 33345 are active; script/format gates are running.

### S15 external comparison and S16 measurement readiness

Primary-source follow-up checked Coraza's
[transformArg implementation](https://raw.githubusercontent.com/corazawaf/coraza/main/internal/corazawaf/rule.go)
on 2026-09-30: it reuses transformation-chain prefixes and excludes mutable TX
values. This supports examining repeated transformations, not copying its cache.
[Coraza issue 1681](https://github.com/corazawaf/coraza/issues/1681) reports unstable
positional cache identity and excess retained entries; this is an upstream report,
not a reproduced Edgion defect or verified current Coraza verdict. Edgion's reuse
is scoped to one scalar and immutable compiler group identity, retaining no values
across scalars. No external WAF production dependency or code was imported.

The next concrete measurement can use existing public Gateway BodyStorage APIs:
request_body publicly exports BodyStorage/BodyStorageView; create storage, use
Pingora RequestBodyBuffer write/finish, then compare view.read_all plus ordinary
WAF against view.snapshot_reader plus public JSON lifecycle. Use separate fresh
sealed views for each lane to avoid read_all cache contamination; include memory,
spilled disk and edited snapshots before considering actual plugin activation.

Existing allocator instrumentation is in edgion-algo/benches/radix_hostname.rs
(System + allocation-free atomic live/peak counters). Gateway's existing
waf_header_ingestion benchmark measures allocation traffic; its thread-local unit
counter misses disk-worker allocations. Reuse the benchmark approach without new
production interfaces or dependencies. Label figures peak live requested heap
bytes, not physical RSS/resident bytes; realloc transient memory, page cache and
allocator overhead are excluded. A global benchmark counter includes asynchronous
worker allocations, so scope/warmup/cleanup must be deterministic and documented.
Rust stats_alloc's [source](https://docs.rs/stats_alloc/latest/src/stats_alloc/lib.rs.html)
is an additional primary reference for allocation/reallocation accounting, not a
proposed dependency. Keep allocator-instrumented measurements distinct from S14/S15
uninstrumented timing. Gateway limits, failure mapping and activation remain open.

### S15 final gate checkpoint

Workspace check 84464, format 32365, docs 39830/38357, transform guard 7241 and
unit layout 65278 exited 0. SSA, metrics and Pingora guards also exited 0.
Workspace Clippy 33345 is still live and compiling Gateway/integration targets;
its terminal success is required before commit. Both reviewers separately checked
and approved final measurement documentation against all selected raw-log values.

Red design rows scanned against the diff: reuse is scalar-local stack state with
no retained body payload, additional shared cache, unsafe code, schema, dependency,
logging or metric change. Canonical budget charges and error order remain tested;
member ranges reject invalid bounds. No response aggregation or Gateway selection
change. Stage only three production files, two existing mounted test files and
this task after Clippy succeeds. Preserve all unrelated task edits; no push.

Workspace Clippy 33345 has now exited 0 without warnings. All required checks
and reviews are complete; final source hashes are unchanged. S15 is ready for
its scoped local commit.

S15 committed locally as `e41f8e1c`; no push. This post-commit checkpoint is
intentionally retained for the next batch.

## S16 actual snapshot measurement checkpoint (2026-09-30)

The prior goal turn made concrete progress: S15 committed as e41f8e1c and reduced
repeated scalar-group work with verified semantics and measured CPU improvement.
Recovery verified HEAD and preserved unrelated task edits. S16 now implements a
Gateway benchmark using existing BodyStorage/public WAF APIs, with separate fresh
captures for complete and snapshot lanes. No production code/API/dependencies or
Gateway selection changes are planned. Implementer owns new bench/Cargo target;
root owns this checkpoint and independent validation. Two independent review
rounds will cover the final harness and measurement claims.

Compile-time benchmark mode selects timing versus allocation counters from process
startup so allocation/deallocation lifetimes stay balanced and timing excludes
atomic counter updates. Global live counters are never reset. Label metrics as
baseline-relative requested heap across the benchmark process, not RSS or exact
isolated request memory. Storage-worker completion and cleanup are part of the
measurement contract; warm-page-cache disk behavior must be explicit.

### S16 harness and initial review

The benchmark owns its System allocator and uses compile-time timing/allocation
modes. It self-checks known Box/Vec allocation, realloc and release accounting
before creating Tokio. Each sample creates fresh memory/forced-spill BodyStorage,
awaits write/finish, drops fixture and capture owner, proves the source path through
public snapshot introspection, then measures inspection with the Transaction still
alive. It separately reports signed live changes after transaction and view drop.
Snapshot EOF and exact byte counts precede JSON completion. Errors abort execution.
An untimed edited-snapshot assertion uses a partial inline splice surrounded by
original source segments, separately built per lane, including a mixed disk case.

Both first-round independent reviewers approved the complete harness/Cargo diff.
They verified counters, ownership, path probes and comparisons. Async Tokio worker
bookkeeping may straddle stage boundaries; metrics are process-global requested
heap observations with that noise, not exact request ownership or RSS. Warm page
cache, allocator overhead and realloc transient exclusions remain explicit.

Initial release prebuild 58377 failed (exit 101): PATH CMake 3.21.2 is below BoringSSL's
3.22 minimum. Existing /tmp/edgion-p090-005-tools/bin/cmake is 3.31.10. The corrected
smoke command sets CMAKE to that path, with EDGION_WAF_SNAPSHOT_MODE=timing and
EDGION_WAF_SNAPSHOT_SMOKE=1; session 1609 is live in /tmp/waf-s16-timing-smoke.log.
No project tool/dependency change was needed. Source identity for round 2:

- `edgion-gateway/Cargo.toml`: `ff28a49bd46f7a621b713c5692dccf0b56b8f8170d38772cc872a70099ce10c7`
- `edgion-gateway/benches/waf_body_snapshot.rs`: `325c83f2c0cb9a74c6211613499c0c527e141a2471ff43744e85aed389697f45`

### S16 second source review and environment recovery

Both independent second-round reviewers approved the exact Cargo/benchmark hashes.
Format, docs, SSA, metrics, Pingora and transform guards passed; unit layout was
still live at last check. Corrected release smoke 1609 now reports missing protoc
in etcd-client's build script, and remains live while other compile jobs finish.
Do not restart until its process is terminal. Existing verified protoc binaries:
/tmp/edgion-quality-protoc/python/protoc/data/bin/protoc (29.3) and the vendored
ws5-center-tools protoc (31.1). Next retry uses PROTOC pointing to the 29.3 binary,
alongside the already verified CMAKE 3.31.10 override. No project dependency or
machine-wide environment change is required; both failed logs remain preserved.

Session 1609 is confirmed terminal (exit 101). Corrected timing smoke 2120 is
now live with both CMAKE and PROTOC overrides, log
/tmp/waf-s16-timing-smoke-final.log. Unit layout 3442 also exited 0.

Workspace check 72622 and Clippy 47130 were launched while release artifacts
compile; logs /tmp/waf-s16-check.log and /tmp/waf-s16-clippy.log. Smoke runs are
correctness-only under this overlap. Final full timing/allocation runs must occur
after all builds/gates are terminal, so measured results are not build-contended.

Workspace check 72622 and Clippy 47130 both exited 0 without warnings. All
format/script guards also passed. Only release smoke 2120 remains live, compiling
the Gateway library; no benchmark outcomes have yet been observed. Report final
inspection heap deltas as additional requested bytes above that stage baseline,
not full-request peak memory: this harness places transaction setup in inspection.

### S16 timing execution results

Corrected release smoke 2120 exited 0. Parsed 384 valid CSV rows: 96 combinations,
2 samples and 2 stages each; timing allocation columns are empty as specified.
All runtime self-check, path, edited-splice, EOF and canonical parity assertions
passed. With all other builds terminal, full timing session 94056 exited 0 and
produced 2112 rows (11 samples per combination/stage), log /tmp/waf-s16-timing.log.
Allocation smoke 34667 is now live with the same CMAKE/PROTOC overrides and
EDGION_WAF_SNAPSHOT_MODE=allocations, log /tmp/waf-s16-alloc-smoke.log. Both public
modes compile from the same two reviewed source hashes; no source edits occurred.

### S16 full measurements and interpretation

Allocation smoke 34667 exited 0 with 384 valid rows and allocator self-checks.
Full allocation run 55783 exited 0 with 2112 rows. Timing and allocation runs each
contain all 96 combinations, 11 samples and 2 stages; CSV schema, stage fields and
sample counts were validated independently. All body/outcome/error/logical-charge,
source-kind, edited-splice and EOF assertions passed in both modes. Full logs:
/tmp/waf-s16-timing.log and /tmp/waf-s16-alloc.log. No agent builds/tests overlapped
these full runs. The two benchmark/Cargo hashes still match both source approvals.

Reproduction (local tooling overrides are environmental, not project changes):

```bash
CMAKE=/tmp/edgion-p090-005-tools/bin/cmake PROTOC=/tmp/edgion-quality-protoc/python/protoc/data/bin/protoc EDGION_WAF_SNAPSHOT_MODE=timing cargo bench -p edgion-gateway --bench waf_body_snapshot
CMAKE=/tmp/edgion-p090-005-tools/bin/cmake PROTOC=/tmp/edgion-quality-protoc/python/protoc/data/bin/protoc EDGION_WAF_SNAPSHOT_MODE=allocations cargo bench -p edgion-gateway --bench waf_body_snapshot
```

Selected medians on aarch64 macOS, 32 same-group subscribers, engine On. Timing
columns use only the timing executable; heap columns use only allocation mode.
Heap peak is additional live requested bytes above inspection's baseline, with
Transaction still alive at its endpoint. It is not total request memory or RSS.
Disk files were just written and page-cache warm. Each cell is an 11-sample median,
not a cross-host guarantee or statistically stable tail estimate.

| Document | Storage | Complete us | Structured us | Complete extra peak B | Structured extra peak B | Complete post-TX B | Structured post-TX B |
|---|---|---:|---:|---:|---:|---:|---:|
| wide_hits | memory | 10527.04 | 1548.50 | 359826 | 1126952 | 0 | 0 |
| wide_hits | spill_warm_cache | 10441.33 | 1498.96 | 428203 | 1129769 | 68377 | 0 |
| duplicates_last_miss | memory | 103.25 | 2073.29 | 5972 | 2102851 | 0 | 0 |
| duplicates_last_miss | spill_warm_cache | 132.58 | 2169.58 | 142203 | 2107986 | 136231 | 0 |
| malformed_tail | memory | 79.92 | 1044.00 | 97595 | 1126968 | 0 | 0 |
| malformed_tail | spill_warm_cache | 126.38 | 1111.33 | 165971 | 1129768 | 68376 | 0 |
| large_scalar | memory | 18330.50 | 3026.92 | 1317607 | 1057611 | 0 | 0 |
| large_scalar | spill_warm_cache | 18237.04 | 3171.17 | 1579790 | 1057626 | 262183 | 0 |

This changes the next optimization priority: avoiding complete disk materialization
removes its retained cache, but facts for many scalars can raise the inspection
peak substantially. The wide input rises from 428203 to 1129769 extra requested
bytes on spill; duplicate-heavy input rises from 142203 to 2107986. Large scalars
benefit (1579790 versus 1057626 extra peak bytes), and their cumulative requested
allocation traffic falls from 50677523 to 1854161 bytes. Post-TX disk cache bytes
are measured while the view remains alive; the structured lane leaves no such
cache in these samples. Post-view signed deltas subtract captured state already
present at the baseline and must not be interpreted as negative allocations.

Do not activate structured execution globally or infer production budgets from
these synthetic shapes. Next audit fact record/arena overhead and preserve
canonical semantics while reducing bounded physical state, then rerun the same
measurement. Wire-level Gateway behavior, cancellation/failure mapping and budget
calibration remain separate activation gates. No production source changed here.

### S17 fact-layout readiness (not implemented)

Current Facts::evaluate retains only transformation OutputLimit or WorkLimit;
all other RuntimeError variants return immediately. Fact nevertheless stores a
Result<usize, RuntimeError> and a separate positive boolean for every subscriber.
The next smallest change can keep five usize fields (range endpoint, subscriber,
input length, transformed length, name length) plus a compact outcome enum:
Positive/Negative/OutputLimit/WorkLimit. Reconstruct canonical transformation
errors at replay. Keep both lengths instead of relying on current transformations
preserving length. Measure size_of before claiming the anticipated 64-to-48-byte
64-bit reduction; do not change growth strategy or share scalar headers yet.

Canonical runtime charges, outcomes, lazy errors, fact count and retry ordering
must remain unchanged. Independent retained-byte admission and growth-copy work
can improve because both depend on the actual record size; near a byte cap,
reserved capacity may change too. Document and test those intended physical-budget
boundary changes rather than falsely promising byte-for-byte identical independent
failures. Cover both deferred errors, positive/negative/negation, cached/uncached
facts, exact byte/work thresholds, duplicates, selection-before-error and retries.
Rerun the unchanged S16 harness; capacity cliffs and provisional overwritten facts
remain, so this candidate is not an activation justification.


### S16 final commit gate (2026-09-30)

Both independent reviewers approved final documentation against the two full logs:
2112 rows per mode, all eight table rows and allocation-traffic figures agree.
They verified the distinction between timing, extra requested-heap peak, retained
snapshot cache and cleanup deltas, including the adverse many-scalar results.
The source hashes above remain unchanged after both independent review rounds.

All required format, workspace check/Clippy, documentation, SSA, metrics inventory,
Pingora source policy, Gateway API transform and unit-layout checks passed. Final
documentation validation and git diff --check also passed. No full workspace test
run was performed in S16; benchmark assertions provide this batch's executable
verification. Earlier WAF unit-test results remain historical S15 evidence.

Pre-commit red-rule scan: this diff adds only a benchmark binary, its Cargo target
and this checkpoint. It reuses canonical BodyStorage and public engine APIs, adds
no production cache, response aggregation, schema, dependency, logging or metrics
change. The benchmark-local allocator delegates original pointer/layout arguments
to System, with allocation-free atomic bookkeeping. Time uses saturating elapsed
duration; all new on-disk content is English. Stage exactly these three files and
preserve unrelated CRD, mesh and plugin task changes. No push is authorized.

S16 committed as `7b21a473`; verified the commit contains only the three scoped
files. No push performed. The latest recovery summary records the next S17 gate.


## S17 compact fact implementation checkpoint (2026-09-30)

Recovery verified baseline 7b21a473 and preserved unrelated task changes. Previous
turn made concrete progress by committing the completed S16 measurements. Current
Fact retains a general RuntimeError result although only two transformation-limit
errors survive speculative evaluation. Canonical replay counts selected values
before reconstructing deferred errors and retains its three-pass charge ordering.
Implementation is delegated for compact outcome storage and exact boundary tests;
independent contract audit started. Root will review the full diff, obtain two
independent review rounds and rerun the unchanged S16 measurement plus required
gates. No benchmark or workspace build is currently running.


### S17 initial independent audit

Contract audit confirmed four states suffice and every other error must still
return immediately. Selection counting precedes deferred errors; those precede
operator work/negation. Existing duplicate, selection and transaction retry tests
provide relevant coverage but need fresh execution after this change. A transient
compile issue in the in-progress fallible filter predicate was reported directly
to the implementer; no source approval is claimed.

Independent accounting audit derived a literal two-subscriber oracle: identical
ARGS @pm hit members with name json.a and value hit charge 25 units each and one
record-sized growth copy. A 48-byte record therefore requires 98 work and 96
retained bytes for two facts, compared with 114 work for the previous 64-byte
record. These expected numbers still require actual layout and test verification.


### S17 implementation and first-round approval

Compact Fact stores five usize fields and Positive/Negative/OutputLimit/WorkLimit.
Fallible accessors reconstruct canonical errors; the transient group cache and
reserve strategy remain unchanged. Both independent reviewers approved R1 against:

- json_rules.rs: 641a10169451a48766cffa459e0a21b5ee1af6d4d99797aa2e6bec5f0667836d
- json_rules_test.rs: 537ae5015b2ec9e03deead1ee4e29a6b36730b5de4e13d20e2930d0669070b4c

Targeted fact tests passed 20 cases, log /tmp/waf-s17-targeted.log. Measured current
Fact is 48 bytes; a test-local type mirroring the exact previous field types is
64 bytes on this 64-bit host (25 percent record reduction, not a request-memory
claim). Literal 96-byte/98-work boundaries and 95-byte/97-work failures passed.
Existing JSON transaction tests are running in session 7740; no result claimed.
Formatting started in session 78989 before workspace and script gates.


Fresh existing JSON transaction tests passed 31/31 (/tmp/waf-s17-transactions.log).
Formatting 78989 exited 0. Full WAF library/public API test process 81765 exited 0,
log /tmp/waf-s17-waf-tests.log. Workspace check 76044 and Clippy 31418 remain live;
docs 96782, SSA, metrics and Pingora guards passed. Layout 42040 remains live.
Transform guard first exited 1 because system Python lacked PyYAML; rerun 84925
uses the existing /tmp/edgion-p090-005-tools/bin/python3 environment. No dependency
or machine-wide environment change was made. Second independent review rounds
started against unchanged source identities; full benchmark waits for all builds.


S17 R2 approved independently by both reviewers against the same two exact hashes.
Fresh full WAF results: 513 library tests passed, 2 ignored, and 5 public API tests
passed. Corrected transform guard 84925 exited 0. Check 76044, Clippy 31418 and unit
layout 42040 are still live; all other required gates passed. No process has been
restarted because of a polling timeout. Benchmark runs remain pending isolation.


Workspace check 76044 exited 0 without warnings; unit layout 42040 exited 0
(958 mounted files, 9 crates). Clippy 31418 remains live. Release benchmark build
15679 is queued/running with --no-run and the existing CMAKE/PROTOC overrides,
log /tmp/waf-s17-bench-build.log. This performs no measurement; full timing and
allocation runs wait until both build processes are terminal. Source hashes and
the unchanged S16 benchmark will identify the exact measured implementation.


### S18 readiness candidate (not implemented or benchmarked)

Read-only audit found a stronger reuse opportunity than adjusting Vec growth:
JsonDocument::scalar already records path_bytes/value_bytes from the exact callback
inputs in its Scalar event. Replay may derive input length from ARGS/ARGS_NAMES,
matched-name length from the collection prefix and path, and body fact ranges from
the event result. Keeping only subscriber, transformed length and outcome could
halve the current record again. Query scalars would need separately bounded range
and length headers; body scalars need no second metadata arena. Preserve callback
arithmetic checks, independent transformed length, lazy errors, sparse subscriber
lookup and query/body ordering. Charge any new header capacity/work explicitly.

Source-level arithmetic (not measured request peaks): current wide 32-subscriber uses 8224
facts and capacity 16384, or 786432 bytes; duplicate 32-subscriber uses 16448 and capacity 32768,
or 1572864 bytes. Reusing event lengths with approximately 24-byte facts and one
32-byte query header could reduce these arena capacities to 393248/786464 bytes.
Query-heavy one-subscriber traffic is a required counterexample:24-byte facts plus
32-byte query headers can exceed the current 48-byte records. Add that shape to
measurement before choosing this design. Existing S16 fixtures must stay unchanged
for comparability. A 1.5x growth alternative moves capacity cliffs and increases
copy work; segmentation bounds slack but adds allocation/search/ownership changes.
Do not bundle these alternatives or use this unmeasured candidate to justify
Gateway activation. S17 still awaits final gates and measured outcomes.

Clippy 31418 exited 0 without warnings. All required gates now pass. Only release
benchmark build 15679 remains live; measurements and final scoped commit pending.

Recovery verified unchanged reviewed source/benchmark identities and live build
session 15679. Previous turn made implementation/test/review progress; this turn
continues that same build, preserving unrelated task changes.


External design comparison: [Coraza issue 183](https://github.com/corazawaf/coraza/issues/183)
proposed grouping matched data under rules to avoid repeating surrounding metadata.
This is historical design motivation, not a fresh benchmark or a claim about the
current Coraza implementation. The transferable principle is to keep repeated
metadata with its owner. Applying it to Edgion scalar events is our inference;
Coraza's match reporting does not establish Edgion's speculative-budget, duplicate
replacement or lazy-error semantics. Those require the local S18 proof above.

Release build 15679 exited 0 (6m38s). All other jobs were terminal before starting
full timing session 47263, log /tmp/waf-s17-timing.log; measurements run in isolation.


### S17 first measurements and follow-up correction

Timing 47263 and allocation 6007 exited 0, each 2112 rows and all 96 combinations.
Unchanged S16 harness assertions passed. Extra requested-heap peak fell from
1129769 to 867625 bytes for wide 32-subscriber spill and 2107986 to 1583698 for duplicate 32-subscriber spill.
First timing wide 32-subscriber memory/spill was 1801.5/1963.708 us, versus S16 1548.5/1498.958.
A justified isolated repeat 1125 also exited 0 with 2112 rows:1865.125/1825.167 us.
Other shapes varied; no stable CPU improvement is claimed. Original measurements
remain /tmp/waf-s17-timing.log, -timing-repeat.log and -alloc.log.

Independent audit identified redundant fallible polarity checks. The second replay
pass already checks transformed() before polarity, and the third pass visits the
same immutable ordered facts after every transformation passed validation. Root
changed polarity back to a direct boolean lookup and documented that prerequisite.
This preserves deferred-error ordering and budgets; it is a candidate explanation,
not proven causality. New source requires fresh tests, review rounds, gates and
measurements before committing; prior approvals apply only to the previous hashes.


### S17 V2 first-round approvals and fresh execution

Both independent reviewers approved the complete V2 diff against baseline 7b21a473:

- json_rules.rs: 5257f9fd58d23490f9cd1d2cf57e7877d63cb8316474caf2db48b87e639b1c53
- json_rules_test.rs: 0f7f2c09a2b9e77a6cf249d81e62bc3daab5964fc8b2a4a88ecb93e8ad6d5c23

The explicit third-pass prerequisite is valid because the preceding pass visited
and validated the identical immutable ordered set. V2 format 25436 passed. Fresh
check 71742, Clippy 36952, WAF tests 83029 and release build 44035 are running; no new
measurement yet. Prior V1 gate results are historical evidence, not V2 verification.


V2 R2 approved independently by both reviewers against the unchanged V2 hashes.
Workspace check 71742 exited 0 without warnings. WAF tests 83029 exited 0: 513 library
passed, 2 ignored; 5 public API passed. Docs, SSA, metrics, Pingora and transform 15485
passed. Clippy 36952, layout 14832 and release build 44035 remain live. Measurement
is deliberately deferred until these processes finish; no source changed since
both V2 review rounds. V1 timing differences are observed cross-run differences,
not proof of a causal regression; V2 must retain that uncertainty in its report.

V2 Clippy 36952 and layout 14832 exited 0. Only release build 44035 is still live;
all V2 required gates passed, with no compiler/lint warnings.


### S17 final V2 measurement results

Release build 44035 exited 0. Full timing 86272, allocation 70054 and a justified timing
repeat 63076 all exited 0. Every run contains 2112 CSV rows: 96 combinations, 11 samples
and 2 stages, with schema/mode/field/count checks. All canonical outcome, logical
charge, source-kind, edited-splice, EOF and allocator assertions passed. No other
agent build/test process overlapped measurement. Benchmark hash remains 325c83f2
(full S16 identity above); V2 source identities still match both review rounds.
Logs are /tmp/waf-s17-v2-timing.log, -timing-repeat.log and -alloc.log.

Selected 32-subscriber engine-On inspection medians follow. Both V2 timing runs are
reported to avoid selecting the better sample. Timing is microseconds; peaks are
additional live requested bytes above the inspection baseline, not RSS or total
request memory. Spill files are page-cache warm. Source captures and generation
setup follow the unchanged S16 benchmark contract.

| Document | Storage | S16 us | V2 first us | V2 repeat us | S16 extra peak B | V2 extra peak B |
|---|---|---:|---:|---:|---:|---:|
| wide_hits | memory |1548.500|1636.000|1620.875|1126952|864808|
| wide_hits | spill_warm_cache |1498.958|1752.791|1594.334|1129769|867625|
| duplicates_last_miss | memory |2073.291|3268.458|1879.083|2102851|1578563|
| duplicates_last_miss | spill_warm_cache |2169.583|2861.291|1916.959|2107986|1583698|
| malformed_tail | memory |1044.000|1047.500|965.542|1126968|864824|
| malformed_tail | spill_warm_cache |1111.334|1100.666|1036.208|1129768|867624|
| large_scalar | memory |3026.916|3117.583|2844.250|1057611|1056587|
| large_scalar | spill_warm_cache |3171.166|3105.000|3112.792|1057626|1056602|

The observed heap reduction is stable across V1/V2: wide spill saves 262144 bytes
(23.2 percent), duplicate spill saves 524288 (24.9 percent). Large-scalar savings are
only 1024 bytes, as fact storage was not its dominant cost. Complete-path peak
values remain the S16 values in these samples. The many-scalar structured peaks
still exceed complete-path peaks substantially; this does not justify activation.

CPU evidence is mixed: wide-input medians remain above S16, while duplicate-input
medians vary greatly between repeats and other shapes vary in either direction.
These cross-run measurements do not establish a stable speedup or prove redundant
checks caused the earlier difference. S17 is a bounded-state memory improvement,
not a throughput claim. Preserve the unfavorable first-run results; further CPU
characterization and S18 ownership changes remain work before Gateway activation.


S18 ownership refinement: RequestState ARGS collection records retain canonical
query ByteRange lengths, and start_json_request_body seeds from those records.
However selected fact ranges carry no original query ordinal; zero-selected
queries leave no facts. A future header could store ordinal/end and borrow that
request's immutable records instead of copying lengths, but explicit bounded
association storage is still necessary. Standalone add_query(name,value) has no
persistent request owner, so its ownership/API needs deliberate adaptation. Do
not infer query ordinals from sparse ranges or claim the extra store disappears.

Pre-commit red-rule scan: only internal WAF fact representation, private unit tests
and this task record change. No production logging/metrics, dependency, schema,
transport/body cache, failure policy or Gateway activation changes. Physical caps
continue to use actual allocated capacity, canonical runtime charges/errors remain
ordered, and no unsafe code or new shared cache is introduced. Written content is
English; tests retain private mounted access. Stage only the three scoped files.

Both independent final documentation reviewers approved every selected median,
full-run row counts, percentage savings and limitations. Final docs guard and
git diff --check passed. S17 is ready for its scoped local commit; no push.

S17 committed locally as `9aca8545`, containing exactly the three scoped files.
No push performed. Recovery summary records S18 design/proof requirements.


## S18 metadata ownership design checkpoint (2026-09-30)

Recovery verified HEAD 9aca8545 and preserved unrelated changes. Previous turn
completed S17, so this is new progress toward the original goal, not a restart.
Independent implementation/contract design audits are in progress before edits.
Root identified a smaller standalone query header than the earlier 32-byte sketch:
(end,path_bytes,value_bytes), three usize fields. Start is implicit in the previous
header's end because query facts are contiguous before body collection; queries
selecting no subscriber need no header. A 24-byte fact plus a 24-byte selected-query
header can match the old 48-byte query-only one-subscriber requested capacity, while
body facts reuse existing Scalar event lengths and retain no such header.

This candidate avoids introducing RequestState borrows into persistent collectors
or changing standalone add_query ownership. It still needs an aggregate physical
budget for both vectors, explicit header growth-copy charges and cleanup, proof
of sparse boundaries and callback arithmetic/error ordering, and query-heavy CPU/
allocation measurements (two allocations may cost more despite equal requested
bytes). No implementation or benefit is claimed yet. Do not change Vec growth
strategy or Gateway activation in this batch.


S18 design audits independently approved 24-byte headers and 24-byte facts with
aggregate limits and unchanged canonical ordering. Concrete tight-cap hazard:
three one-subscriber queries fit 144 bytes, but greedy fact growth to four records
can consume space needed for the third header. Coordinate reservation across the
two arenas, preserve callback name-length arithmetic, and clear both on failure.

Root added opt-in query fixtures to waf_body_snapshot: one query, 512 selected
queries and 512 sparse queries selecting only the final q0511. Default body fixture
bytes remain unchanged; URI/request-line assembly is outside measured stages.
First benchmark R1 reviewers requested an explicit findings/subscriber assertion
and avoiding sample's new eighth argument. These corrections are pending until
baseline process 16087 is terminal; do not confuse its initial output with the
final corrected-harness baseline. Log /tmp/waf-s18-query-baseline-timing.log.

The current baseline build unexpectedly recompiles Gateway after the new commit;
production WAF source remains exactly 9aca8545. Implementation agent is preparing
scratch copies only under /tmp/waf-s18-candidate/ while this build runs; no repo
production edits or candidate tests yet. After the baseline process finishes, root
will correct/review the benchmark, capture final body/query timing+allocation
baselines, then authorize applying the candidate. Preserve all existing processes;
do not restart due to observation timeout.


Corrected benchmark R2 independently approved the full diff and hash above. Initial
baseline 16087 exited 0; corrected baseline 86151 is still building, not yet measured.
Its production source is unchanged S17. /tmp/waf-s18-bench-corrected.rs was applied;
it is no longer a pending patch. Both final review requests were read-only.


### S18 frozen pre-change baselines

Corrected query timing 86151, query allocation 84079, body allocation 94276 and body
timing 65721 all exited 0. Query runs each contain 1584 rows (72 combinations x11
samples x2 stages); body runs each contain 2112 rows (96 combinations x11 x2).
CSV stage counts/width were checked; all outcome/source/EOF/edited-splice/allocator
and query-finding assertions passed. An initial ad-hoc CSV parser command had a
Python whitespace syntax error; the corrected validator passed all four files.

Logs: /tmp/waf-s18-query-baseline-timing-final.log,
/tmp/waf-s18-query-baseline-alloc.log, /tmp/waf-s18-body-baseline-alloc.log and
/tmp/waf-s18-body-baseline-timing.log. Production identities at measurement remain
S17 source 5257f9fd58d23490f9cd1d2cf57e7877d63cb8316474caf2db48b87e639b1c53 and
test 0f7f2c09a2b9e77a6cf249d81e62bc3daab5964fc8b2a4a88ecb93e8ad6d5c23; frozen
benchmark 88f79815d9b22a07636ee5bd06a8a5183ba7fd2996c813755e62722c28be8a97.
No other build/test job overlapped these final measurements.

Root authorized applying the scratch candidate to the two production/test files
only after those baseline runs and hash checks. Implementer will run targeted WAF
tests, then root will request two independent full-diff review rounds and final
same-harness measurements. No S18 implementation test result is claimed yet.


### S18 implementation and two independent source rounds

Candidate applied only after baseline capture and original-source hash verification.
Facts and QueryHeader each measure 24 bytes on this 64-bit host. Actual allocated
capacities share one cap; coordinated reservation and both growth-copy charges
retain explicit independent budgets. Sparse query headers retain temporary input
lengths, while body replay consumes canonical Scalar event lengths. Callback-time
name arithmetic, deferred-error order and all three replay passes remain intact.

Targeted fact tests passed 25/25 (/tmp/waf-s18-targeted.log); broader JSON tests
passed 110/110 (/tmp/waf-s18-json.log). Root added populated-query/body abandonment
coverage after R1. Both independent R2 reviewers approved the complete final diff:

- json_rules.rs: 78f2a0fc30227fa3739d8030a862c69676ef772630f1044a74447cd056c11be8
- json_rules_test.rs: 5df8fe613d94f687298989881b1caf94db22b6e0a4e2cc889537e09252054f7a
- waf_body_snapshot.rs: 88f79815d9b22a07636ee5bd06a8a5183ba7fd2996c813755e62722c28be8a97

Final full WAF test session 83323 exited 0; formatting 20368, docs 4385, SSA, metrics,
Pingora and transform 35420 passed. Workspace check 76757, Clippy 76804, layout 32664
and release --no-run build 12359 are still live. Logs /tmp/waf-s18-*.log. Do not run
measurements until these jobs are terminal; no measured S18 benefit is claimed yet.
The new owner adds fixed bytes and query arena allocation events, excluded from
arena-capacity equality tests but included in the end-to-end benchmark.


Recovery verified the same scoped worktree and live sessions; prior turn completed
implementation/review/testing, so progress is concrete. Workspace check 76757 exited 0
without warnings; layout 32664 also exited 0. Clippy 76804 and release build 12359 remain
live. Comparison script /tmp/waf-s18-compare.py is ready and syntax-checked; it will
validate all case/stage/sample identities and report peak, requested allocation,
allocation calls and timing for body 32-subscriber and query 1/8/32-subscriber scenarios.

Read-only follow-up audit started on tasks 03-06: task 05 still describes streaming
callbacks with no BodyStorage for JSON, apparently inconsistent with complete JSON
admission before upstream exposure. Verify current authority/call chain before
changing that task or promoting the classifier; do not treat historical queue text
as permission to expose upstream headers before a final body-dependent verdict.


### Next delivery readiness (read-only; not implemented)

Independent audit confirmed task 05's unconditional cache exclusion and relay-body
handler framing are stale against AGENTS body ownership and the original
waf-prefix-before-dispatch regression guard. Current WafPlugin::run_request awaits
capture/publication/Phase 2 before GoodNext; moving the verdict into relay callbacks
would reintroduce upstream exposure. A full-inspection JSON snapshot adapter can
reuse ensure_request_body plus request_body_snapshot_reader, avoiding read_all's
materialization without removing canonical capture or adding a transport observer.

After S18 measurement/commit, reconcile tasks 03-06 and README with actual evidence:
JSON reducer/subscriber/mixed ordinary scheduling are implemented, but simultaneous
raw+JSON+compatibility lanes, Gateway activation, wire/hot-reload evidence and
production budgets are not. Then define an explicit bounded activation policy and
implement the awaited full-JSON snapshot adapter. Initial Unsupported alone can
choose ordinary publication; independent resource failures cannot silently fall
back. Feed Complete only after actual reader EOF and finish before GoodNext.
Preserve prefix admission. Required delivery tests include disk/no-read_all, edits,
unsupported fallback, terminal limits, bodyless input, cancellation, pre-dispatch
deny/malformed behavior, H1/H2 and generation pinning. Snapshot immutability does
not itself freeze subsequent forwarding mutations; retain the existing full-mode
contract or resolve a changed one explicitly. No policy/default is selected here.


### S18 final measurement evidence

All final body/query timing/allocation runs exited 0 after check, Clippy and release
build completed. No concurrent build or test ran during measurement. The frozen
benchmark and production/test hashes still match R2. The CSV validator checks all
19 columns, mode, stage combinations and exactly 11 distinct sample IDs per group:
2112 rows per body run and 1584 per query run, for baseline and final alike.

The following are On/structured/inspection medians. Timing is microseconds; peak
is additional live requested heap above the inspection baseline, not RSS or total
request memory. Capture and URI fixture construction are separate. Spill files
are warm in the page cache. Counters are compiled out of timing runs.

| Case | Rules | Storage | Old us | New us | Old peak B | New peak B | Old calls | New calls | Old requested B | New requested B |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| wide_hits | 32 | memory | 1606.500 | 1701.875 | 864808 | 471632 | 849 | 850 | 1856615 | 1070247 |
| wide_hits | 32 | spill_warm_cache | 1673.125 | 1670.083 | 867625 | 474449 | 853 | 854 | 1925480 | 1139112 |
| duplicates_last_miss | 32 | memory | 1895.709 | 2073.625 | 1578563 | 792171 | 1090 | 1091 | 3415574 | 1842774 |
| duplicates_last_miss | 32 | spill_warm_cache | 1875.708 | 2151.125 | 1583698 | 797306 | 1096 | 1097 | 3552549 | 1979749 |
| malformed_tail | 32 | memory | 943.375 | 1053.167 | 864824 | 471648 | 845 | 846 | 1855175 | 1068807 |
| malformed_tail | 32 | spill_warm_cache | 1018.625 | 1132.583 | 867624 | 474448 | 849 | 850 | 1924039 | 1137671 |
| large_scalar | 32 | memory | 2876.458 | 3079.417 | 1056587 | 1055091 | 74 | 75 | 1588690 | 1585682 |
| large_scalar | 32 | spill_warm_cache | 2865.250 | 3130.083 | 1056602 | 1055106 | 84 | 85 | 1852129 | 1849121 |

| Case | Rules | Storage | Old us | New us | Old peak B | New peak B | Old calls | New calls | Old requested B | New requested B |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| query_single | 1 | memory | 2.458 | 3.542 | 5268 | 5284 | 34 | 35 | 5903 | 5919 |
| query_single | 1 | spill_warm_cache | 22.667 | 50.750 | 5385 | 5401 | 36 | 37 | 6161 | 6177 |
| query_single | 8 | memory | 3.542 | 9.000 | 5604 | 5452 | 38 | 39 | 6767 | 6447 |
| query_single | 8 | spill_warm_cache | 24.125 | 32.375 | 5721 | 5569 | 40 | 41 | 7025 | 6705 |
| query_single | 32 | memory | 8.417 | 10.000 | 6911 | 6183 | 42 | 43 | 10223 | 8751 |
| query_single | 32 | spill_warm_cache | 35.292 | 34.292 | 6911 | 6183 | 44 | 45 | 10481 | 9009 |
| query_many | 1 | memory | 147.458 | 167.792 | 144837 | 144853 | 1093 | 1103 | 256901 | 256917 |
| query_many | 1 | spill_warm_cache | 241.417 | 260.125 | 144954 | 144970 | 1095 | 1105 | 257159 | 257175 |
| query_many | 8 | memory | 323.333 | 333.792 | 316869 | 230869 | 1097 | 1107 | 601157 | 429141 |
| query_many | 8 | spill_warm_cache | 431.458 | 437.084 | 316986 | 230986 | 1099 | 1109 | 601415 | 429399 |
| query_many | 32 | memory | 1460.667 | 1350.958 | 906848 | 525936 | 1101 | 1111 | 1781957 | 1020117 |
| query_many | 32 | spill_warm_cache | 1475.000 | 1504.000 | 906848 | 525936 | 1103 | 1113 | 1782215 | 1020375 |
| query_sparse | 1 | memory | 80.542 | 87.958 | 120309 | 120325 | 62 | 63 | 202224 | 202240 |
| query_sparse | 1 | spill_warm_cache | 147.208 | 130.209 | 120426 | 120442 | 64 | 65 | 202482 | 202498 |
| query_sparse | 8 | memory | 104.542 | 105.208 | 120645 | 120493 | 66 | 67 | 203088 | 202768 |
| query_sparse | 8 | spill_warm_cache | 164.583 | 184.875 | 120762 | 120610 | 68 | 69 | 203346 | 203026 |
| query_sparse | 32 | memory | 163.292 | 151.042 | 121952 | 121224 | 70 | 71 | 206544 | 205072 |
| query_sparse | 32 | spill_warm_cache | 244.875 | 244.500 | 121952 | 121224 | 72 | 73 | 206802 | 205330 |

Wide spill saves 393176 peak bytes (45.3 percent); duplicate spill saves 786392
(49.7 percent). Query-many with 32 rules saves 380912 peak bytes (42.0 percent).
Large-scalar savings are only 1496 bytes. One-subscriber queries add 16 peak and
cumulative requested bytes: arena-capacity parity excludes fixed owner overhead.
Query-many adds 10 allocation events; single/sparse queries and body cases add one.
These measured costs prevent claiming universally allocation-free metadata sharing.

The first timing run has regressions, including duplicate spill 1875.708 to
2151.125 us and query-many/one-rule memory 147.458 to 167.792 us. Smaller cases
also show substantial cross-run variation. This batch establishes reduced retained
metadata, not faster throughput. CPU calibration remains necessary before Gateway
activation; independent physical caps and complete-path outcome parity still apply.

Logs: /tmp/waf-s18-{body,query}-final-{timing,alloc}.log. An attempted direct-binary
timing repeat used the last allocation-mode binary; its CSV mode exposed the
mismatch. Those outputs are retained as *-final-alloc-repeat.log and are excluded
from timing claims. The actual timing repeat explicitly builds with
EDGION_WAF_SNAPSHOT_MODE=timing before running either fixture family.

### S18 isolated timing repeat

Both actual timing-repeat processes exited 0; mode and all sample groups passed
the same validator. Medians below retain the unfavorable first measurements.

| Case | Rules | Storage | Baseline us | Final first us | Final repeat us |
|---|---:|---|---:|---:|---:|
| wide_hits | 32 | memory | 1606.500 | 1701.875 | 1553.334 |
| wide_hits | 32 | spill_warm_cache | 1673.125 | 1670.083 | 1744.375 |
| duplicates_last_miss | 32 | memory | 1895.709 | 2073.625 | 2079.875 |
| duplicates_last_miss | 32 | spill_warm_cache | 1875.708 | 2151.125 | 2075.750 |
| malformed_tail | 32 | memory | 943.375 | 1053.167 | 1045.709 |
| malformed_tail | 32 | spill_warm_cache | 1018.625 | 1132.583 | 1123.458 |
| large_scalar | 32 | memory | 2876.458 | 3079.417 | 2852.167 |
| large_scalar | 32 | spill_warm_cache | 2865.250 | 3130.083 | 2882.417 |
| query_single | 1 | memory | 2.458 | 3.542 | 2.417 |
| query_single | 1 | spill_warm_cache | 22.667 | 50.750 | 34.000 |
| query_single | 8 | memory | 3.542 | 9.000 | 3.917 |
| query_single | 8 | spill_warm_cache | 24.125 | 32.375 | 30.250 |
| query_single | 32 | memory | 8.417 | 10.000 | 9.625 |
| query_single | 32 | spill_warm_cache | 35.292 | 34.292 | 37.625 |
| query_many | 1 | memory | 147.458 | 167.792 | 153.000 |
| query_many | 1 | spill_warm_cache | 241.417 | 260.125 | 227.875 |
| query_many | 8 | memory | 323.333 | 333.792 | 328.333 |
| query_many | 8 | spill_warm_cache | 431.458 | 437.084 | 395.542 |
| query_many | 32 | memory | 1460.667 | 1350.958 | 1324.292 |
| query_many | 32 | spill_warm_cache | 1475.000 | 1504.000 | 1362.333 |
| query_sparse | 1 | memory | 80.542 | 87.958 | 80.333 |
| query_sparse | 1 | spill_warm_cache | 147.208 | 130.209 | 149.667 |
| query_sparse | 8 | memory | 104.542 | 105.208 | 95.875 |
| query_sparse | 8 | spill_warm_cache | 164.583 | 184.875 | 157.917 |
| query_sparse | 32 | memory | 163.292 | 151.042 | 151.750 |
| query_sparse | 32 | spill_warm_cache | 244.875 | 244.500 | 228.459 |

Repeat logs: /tmp/waf-s18-{body,query}-final-timing-repeat.log.

The repeat confirms a material tradeoff in two body shapes: duplicate memory is
9.7 percent slower than baseline and duplicate spill 10.7 percent slower; malformed
memory is 10.8 percent slower and malformed spill 10.3 percent slower. There is no
paired interleaved baseline or profiler attribution, so this is observed regression
evidence, not proof of its cause. Keep CPU diagnosis as a prerequisite to activation,
not merely generic noise. The local engine-only batch is accepted for the measured
45-50 percent many-scalar heap reduction with this unresolved CPU cost explicitly
recorded. It does not enable a production traffic path.

Both independent reviewers are auditing final measurement claims and documentation.
Pre-commit red-rule scan: internal fact/header ownership stays bounded by aggregate
actual capacities; both arenas clear on terminal failure/abandonment. Query headers
are request-owned metadata, not another body cache or global registry. Canonical
replay/error ordering remains covered. No schema, transport, logging/metrics,
dependency, unsafe code or failure-policy changes are introduced. Tests retain
private mounted access and written content is English. Commit only the four scoped
files; unrelated task changes remain untouched.

Both independent final documentation audits approved all 52 table rows and checked
raw modes/counts/medians, fixed overhead, allocation events and regression claims.
Neither found a blocker to this four-file engine-only local commit. The documented
CPU tradeoff remains an activation prerequisite. Final docs guard and diff check
passed; source identities are unchanged from both independent R2 approvals.

S18 committed locally as `da048aef` with exactly the four scoped files. No push.
Post-commit recovery summary updated; unresolved CPU cost remains explicit.


## S19 CPU investigation checkpoint

Previous goal turn made concrete progress: S18 committed da048aef, full evidence
and two-round review retained. Current scoped production equals that commit.
Duplicate and malformed-body regressions motivate checking collection work first:
malformed On mode fails body parsing before rule replay, so replay-only explanations
cannot cover both cases. Two independent reviewers are checking safe fast-path
opportunities without edits or concurrent benchmark work.

Baseline engine-only release build started as session 18613, log
/tmp/waf-s19-baseline-build.log. Existing engine harness includes transaction
setup/drop, resident body input, 31 samples, 1/8/32 rules and complete/1KiB/64KiB
lanes; it excludes Gateway capture and IO. Compare within that scope, retain the
Gateway snapshot harness for final end-to-end verification.

S19 baseline build exited 0 and binaries were preserved before authorized edits.
Independent contract and implementation audits agree: fact capacity already admitted
under immutable aggregate limits can skip repeated budget arithmetic/reservation,
but count enforcement remains unconditional and growth-copy charge precedes count
failure. First-selected-query header processing must preserve existing fact capacity.
Overallocated Vec capacity cannot waive max_facts. This is a candidate, not a proven
explanation for S18 timings. Implementation and a disabled-fast-path boundary oracle
are in progress; no measurements run concurrently with its tests/builds.

Prepared /tmp/waf-s19-paired.py runs frozen engine binaries in ABBA order twice
(eight full 31-sample runs), validates 72 distinct case/rule/mode/lane/chunk rows
per run, and retains every output. Candidate binary is not yet available.

S19 candidate adds FAST_RESERVE to the existing private const-generic evaluator;
production selects true, the boundary oracle selects false with identical matcher
reuse. Three new tests cover partial state/error parity, all four fact/header growth
combinations, overallocated fact capacity vs max_facts, and literal 144/207 bounds.
R1 independent reviews started on the full diff. Initial targeted compilation found
a repeated-array byte-slice coercion error in the new fixture (E0308); implementer
is correcting it before rerunning. No performance measurement has started yet.

Fixture coercion corrected; JSON-targeted rerun exited 0 (114 passed). Candidate
engine --no-run build session 28331 is live, log /tmp/waf-s19-engine-build.log.
Both independent R1 reviewers approved source a57350b5ededc550e2520d18bd1c82d828fc8032684868d43a15c66e48f3e710
and tests d32d2ecf74ed17cb378c0984d1d182da4c736aed4ba050460327276a7544cb99.
Their reviews establish reservation/count/error-order reasoning, not performance.

Candidate engine build 28331 confirmed terminal 0. Copied executable to
/tmp/waf-s19-engine-candidate; baseline binary remains independent. Paired ABBAx2
engine measurements started in session 13630, /tmp/waf-s19-paired.log. No other
build/test jobs are live; reviewers are read-only. Retain complete-lane controls
to distinguish machine drift from structured-only effects.

### S19 first paired engine evidence

ABBAx2 session 13630 exited 0, eight files each with 72 distinct rows and31 samples
per row. All public execution parity assertions passed; identity and logical-charge
columns match across runs. No other build/test overlapped. Both independent R2
reviews approved unchanged a57350/d32d2e source/test hashes, correctness only.

Below are On,32-rule medians in microseconds; each slash-separated list preserves
all four run medians. Delta compares their medians and is descriptive only.

| Case | Lane | Baseline runs us | Candidate runs us | Delta % |
|---|---|---|---|---:|
| duplicates_last_miss | complete | 95.667/101.292/89.041/95.084 | 93.208/94.958/94.083/95.292 | -0.90 |
| duplicates_last_miss | structured | 2301.542/2561.250/2117.875/2141.250 | 2052.917/2108.250/2167.667/2188.291 | -3.76 |
| large_scalar | complete | 17289.292/18119.750/16633.083/16779.458 | 16547.000/16436.042/17244.666/17317.000 | -0.81 |
| large_scalar | structured | 3252.458/3411.292/3187.167/3229.875 | 3139.084/3259.333/3379.541/3294.542 | 1.10 |
| malformed_tail | complete | 76.417/85.833/81.792/76.292 | 68.709/77.084/76.542/101.083 | -2.90 |
| malformed_tail | structured | 1164.500/1438.708/1085.167/1121.292 | 1057.458/1093.875/1132.833/1141.583 | -2.58 |
| wide_hits | complete | 11017.750/10456.958/9890.084/9851.041 | 9518.208/9596.750/9885.167/10105.916 | -4.25 |
| wide_hits | structured | 1760.917/1694.042/1640.917/1648.000 | 1568.000/1547.750/1635.084/1661.875 | -4.16 |

Logs /tmp/waf-s19-engine-{0..7}-{baseline,candidate}.csv and paired.log. Complete
controls also move, including wide_hits -4.25 percent against structured -4.16, so
this evidence does not establish a causal speedup or resolve S18 regression. The
candidate removes redundant admitted-capacity arithmetic without new allocation
or policy changes; final BodyStorage comparison and all required gates remain.

S19 formatting exited0 and approved source hashes stayed unchanged. Full WAF tests
in /tmp/waf-s19-tests.log exited0. Sequential gate runner session18448 now runs
workspace check, then Clippy and release snapshot --no-run; six guard runner
session28205 covers docs/SSA/metrics/Pingora/transform/layout. No measurements during
these jobs. Independent follow-ups check actual codegen and prepare the stale-task
reconciliation read-only; they must not modify source or build.

S19 workspace check and Clippy exited0; six guard runner28205 exited0 for every
check. Full tests are522 library passed,2 ignored and5 public API passed. Release
snapshot build remains live in runner18448. Next measurement script
/tmp/waf-s19-snapshot-paired.py runs ABBA separately for body and query fixtures,
validates timing mode and row identities, preserving controls and all outputs.

Independent next-delivery read-only audit corroborated stale-task corrections:
README/task03 should reflect implemented engine eligibility, canonical ordering
and ordinary-unit integration; task04 must not claim simultaneous three-lane feed.
Task05 should await canonical capture then snapshot reader/actual EOF/Phase2 before
GoodNext, preserving prefix. Task06 needs an explicit activation decision with
CPU/memory/wire/hot-reload gates. Additional integration caveat: cached
ensure_request_body returns current length without rechecking its optional cap;
physical and WAF limits require independent verification. No defaults or Gateway
implementation selected, and none of those task files changed in S19.

Independent arm64 codegen audit with installed LLVM objdump confirmed the fast
path survives optimization. Baseline evaluate_with_reuse performs checked header
capacity times24 and budget subtraction at0x1000740e4-0x100074104 before its
fact-length/capacity branch at0x10007411c. Candidate branches at0x1000740e8/0x1000740ec
directly to0x1000741c0 with spare capacity, leaving multiplication/subtraction at
0x1000740f0-0x100074114 only on growth. Unconditional count admission remains at
0x100073fa8-0x100073fb0. This proves fewer operations on that path, not causal
throughput attribution or resolution of S18 regressions. Binary identities:

- baseline: 8d028c3697ff8e012aff59c85fdde483f09c333a42cea012eb678d4569682d9c

- candidate: bec1bec7b72262fc28344d0ae41a65dabb5c71348a9b77b455db124407bcab76

All S19 gate runner stages exited0, including release snapshot build (5m11s).
Candidate executable copied to /tmp/waf-s19-snapshot-candidate. Eight isolated
BodyStorage runs (body ABBA, then query ABBA) started as session97223; no concurrent
build/test jobs. Both snapshot executables use the frozen S18 harness and timing
mode with allocation counters compiled out.

First snapshot ABBA session97223 exited0; all eight files passed timing-mode,
row-count, stage and distinct sample checks (body2112/query1584 rows each). Results
are noisy: wide memory structured +8.48 percent while its complete control +9.23;
query-many/8 spill structured +30.06 while complete +21.59. Some later query32 cases
are near parity. These results cannot establish a causal gain and must not be
suppressed. A reversed BAAB sequence for each family is now live as session84930,
/tmp/waf-s19-snapshot-reverse.log, with *snapshot-reverse-*.csv outputs. Both sets
will be retained. Root will not label S18's regression resolved from these results.

### S19 final snapshot comparison and candidate decision

Reverse session84930 also exited0. All16 snapshot timing files pass row/sample
validation; the table preserves four run medians per binary (ABBA then BAAB).
Each run has11 samples per combination. Values are On/inspection microseconds.
Complete controls are included alongside every selected structured result.

| Case | Rules | Storage | Lane | Baseline runs us | Candidate runs us | Delta % |
|---|---:|---|---|---|---|---:|
| duplicates_last_miss | 32 | memory | complete | 90.792/90.875/91.500/83.583 | 100.625/90.625/99.750/91.291 | 5.16 |
| duplicates_last_miss | 32 | memory | structured | 2001.291/1892.750/1877.833/1791.666 | 2071.084/2043.292/2019.375/1865.334 | 7.75 |
| duplicates_last_miss | 32 | spill_warm_cache | complete | 123.500/114.500/129.208/156.125 | 139.583/132.167/130.875/126.042 | 4.09 |
| duplicates_last_miss | 32 | spill_warm_cache | structured | 2080.625/1913.250/1913.666/2481.083 | 2032.000/2018.542/2070.375/1885.917 | 1.41 |
| large_scalar | 32 | memory | complete | 17993.334/17264.625/17339.333/16614.083 | 19171.500/18306.959/19207.750/16625.000 | 8.31 |
| large_scalar | 32 | memory | structured | 3018.166/2896.792/2865.958/2757.333 | 3117.958/2968.917/3104.666/2837.167 | 5.39 |
| large_scalar | 32 | spill_warm_cache | complete | 17887.333/17128.875/17133.417/16780.792 | 18729.000/18037.250/18837.083/16435.209 | 7.31 |
| large_scalar | 32 | spill_warm_cache | structured | 2965.000/2873.875/2880.625/2847.583 | 3144.333/3226.583/3111.000/2858.291 | 8.70 |
| malformed_tail | 32 | memory | complete | 73.167/73.500/79.792/73.291 | 82.250/79.834/80.917/66.958 | 9.51 |
| malformed_tail | 32 | memory | structured | 1039.542/1037.958/1031.750/945.542 | 1031.666/1031.500/1025.375/940.833 | -0.62 |
| malformed_tail | 32 | spill_warm_cache | complete | 116.792/135.792/113.416/99.917 | 134.333/128.291/125.417/121.625 | 10.21 |
| malformed_tail | 32 | spill_warm_cache | structured | 1058.417/1033.042/1098.375/1013.834 | 1094.875/1027.709/1085.584/1002.167 | 1.04 |
| query_many | 1 | memory | complete | 262.542/286.875/255.416/263.083 | 261.583/343.917/264.333/263.084 | 0.34 |
| query_many | 1 | memory | structured | 152.917/167.667/153.083/153.125 | 152.750/224.416/152.708/152.917 | -0.18 |
| query_many | 1 | spill_warm_cache | complete | 334.292/371.625/327.250/341.125 | 346.458/482.958/351.750/329.916 | 3.37 |
| query_many | 1 | spill_warm_cache | structured | 233.083/216.625/222.334/223.500 | 218.291/327.917/227.750/231.000 | 2.90 |
| query_many | 32 | memory | complete | 10425.833/10377.583/10408.500/10179.959 | 10357.583/10343.125/10214.334/10245.417 | -0.95 |
| query_many | 32 | memory | structured | 1325.708/1325.250/1335.250/1301.208 | 1307.416/1339.916/1334.250/1312.417 | -0.16 |
| query_many | 32 | spill_warm_cache | complete | 9994.208/10080.834/10104.709/9921.125 | 10031.792/10028.666/9973.209/9943.416 | -0.36 |
| query_many | 32 | spill_warm_cache | structured | 1361.041/1402.708/1355.167/1354.584 | 1382.875/1351.375/1318.542/1340.625 | -0.89 |
| query_many | 8 | memory | complete | 2006.666/2004.916/1997.167/2009.792 | 2069.333/3115.417/2037.250/1970.250 | 2.37 |
| query_many | 8 | memory | structured | 337.417/407.458/332.333/327.625 | 334.958/538.458/332.375/329.125 | -0.36 |
| query_many | 8 | spill_warm_cache | complete | 2232.959/2204.417/2026.583/2008.292 | 2037.208/3358.334/2031.583/2042.958 | -3.56 |
| query_many | 8 | spill_warm_cache | structured | 398.791/389.875/401.792/389.375 | 394.542/631.208/384.292/393.500 | -0.08 |
| query_single | 1 | memory | complete | 2.417/2.167/2.375/1.792 | 2.417/2.334/2.042/2.083 | -2.75 |
| query_single | 1 | memory | structured | 2.417/2.458/3.125/2.208 | 2.541/2.959/2.333/2.709 | 7.69 |
| query_single | 1 | spill_warm_cache | complete | 39.917/32.834/22.958/23.000 | 31.750/23.542/33.708/33.333 | 16.57 |
| query_single | 1 | spill_warm_cache | structured | 34.083/28.542/30.125/30.125 | 24.750/20.708/18.500/42.791 | -24.55 |
| query_single | 32 | memory | complete | 29.125/29.083/34.541/26.708 | 26.792/29.500/25.167/26.750 | -8.02 |
| query_single | 32 | memory | structured | 10.042/9.333/9.833/9.292 | 9.417/10.125/8.542/9.125 | -3.26 |
| query_single | 32 | spill_warm_cache | complete | 58.333/61.500/69.375/54.500 | 53.417/72.209/54.666/67.959 | 2.33 |
| query_single | 32 | spill_warm_cache | structured | 32.292/42.125/42.667/38.459 | 41.833/32.292/29.500/47.625 | -8.02 |
| query_single | 8 | memory | complete | 10.958/7.625/10.917/6.834 | 8.416/10.625/8.500/7.083 | -8.77 |
| query_single | 8 | memory | structured | 3.833/4.584/4.042/3.875 | 4.125/5.167/4.250/4.500 | 10.52 |
| query_single | 8 | spill_warm_cache | complete | 25.125/30.292/25.584/36.583 | 27.625/43.792/27.833/31.750 | 6.63 |
| query_single | 8 | spill_warm_cache | structured | 20.958/26.750/22.792/41.000 | 33.583/55.083/19.209/27.208 | 22.71 |
| query_sparse | 1 | memory | complete | 82.167/82.208/81.000/75.167 | 87.709/134.250/74.916/85.792 | 6.33 |
| query_sparse | 1 | memory | structured | 80.875/81.917/80.708/74.750 | 81.875/128.792/74.875/81.041 | 0.82 |
| query_sparse | 1 | spill_warm_cache | complete | 132.083/108.667/128.125/121.333 | 144.166/180.208/131.833/128.333 | 10.64 |
| query_sparse | 1 | spill_warm_cache | structured | 131.708/124.916/114.875/136.125 | 134.250/169.666/139.000/128.500 | 6.48 |
| query_sparse | 32 | memory | complete | 307.375/319.958/283.708/308.083 | 307.333/307.459/307.625/327.166 | -0.06 |
| query_sparse | 32 | memory | structured | 151.292/165.667/152.750/152.167 | 152.000/152.125/139.459/166.958 | -0.26 |
| query_sparse | 32 | spill_warm_cache | complete | 383.750/418.209/370.708/380.209 | 389.834/387.666/368.834/430.625 | 1.77 |
| query_sparse | 32 | spill_warm_cache | structured | 225.583/196.750/209.875/220.500 | 217.875/230.083/218.083/230.625 | 4.13 |
| query_sparse | 8 | memory | complete | 130.917/133.208/164.917/131.125 | 131.458/131.416/132.417/131.375 | -0.55 |
| query_sparse | 8 | memory | structured | 96.250/98.291/124.083/96.208 | 96.292/96.125/97.542/96.208 | -1.05 |
| query_sparse | 8 | spill_warm_cache | complete | 217.583/222.458/226.500/217.041 | 172.042/206.416/215.375/200.792 | -7.46 |
| query_sparse | 8 | spill_warm_cache | structured | 151.083/165.500/162.958/157.833 | 152.500/176.416/171.375/152.167 | 0.96 |
| wide_hits | 32 | memory | complete | 10059.667/9816.250/10482.292/9394.417 | 11096.333/10614.000/10700.584/9356.250 | 7.24 |
| wide_hits | 32 | memory | structured | 1514.375/1492.833/1549.583/1528.250 | 1643.250/1619.000/1664.834/1511.708 | 7.22 |
| wide_hits | 32 | spill_warm_cache | complete | 9768.208/9543.916/10094.334/9248.875 | 10573.625/10258.375/10357.750/9386.000 | 6.75 |
| wide_hits | 32 | spill_warm_cache | structured | 1536.917/1540.167/1604.584/1508.958 | 1676.959/1657.875/2492.417/1481.542 | 8.38 |

The candidate does not meet a measured-benefit gate. Combined wide-memory
structured rises7.22 percent while complete rises7.24; duplicate-memory structured
rises7.75 while complete rises5.16; malformed-memory is near parity (-0.62).
Query-many memory is essentially unchanged, while short query timing remains
variable. These controls prevent causal attribution of either gain or regression.
Instruction elimination alone is insufficient to establish an improvement, and
there is no basis to mark S18's CPU issue solved.

Proposed disposition: reject the unproven fast-path candidate and its experiment-only
const generic/tests, restore the two scoped Rust files exactly to da048aef, retain
this evidence, and pursue profiling or more isolated attribution before another
CPU change. Saved binaries and CSV files remain available for inspection. Both
independent reviewers are auditing these results and disposition before restoration.
No Gateway activation or performance improvement is claimed.

Candidate source/test diff saved to /tmp/waf-s19-rejected-candidate.patch before
restoration; SHA256 ad1bf1416a5d75d56edec6cc3e4042a4c887d599ccc56ca47bc3590afc8d23d8.

## Continuation checkpoint (2026-10-05)

Current starting HEAD is `6c89d137`; historical batch commits are not the current
branch tip. Independent read-only review and coordinator revalidation confirmed
the S19 source/test identities and the complete two-file diff against `da048aef`.
Only the unproven `FAST_RESERVE` experiment and its three comparison tests were
restored to the S18 versions. S15 scalar reuse and S18 metadata ownership remain.
The removed tests exercised only the removed experimental branch; the retained
scalar/query reuse, aggregate 144/207 boundary and failure-cleanup tests continue
to cover the restored evaluator. No throughput improvement or resolution of the
S18 CPU regression is claimed. Historical benchmark tables and saved candidate
patch remain evidence; recorded sessions must not be treated as currently live.

Restored SHA256 identities:

- `json_rules.rs`: `78f2a0fc30227fa3739d8030a862c69676ef772630f1044a74447cd056c11be8`
- `json_rules_test.rs`: `5df8fe613d94f687298989881b1caf94db22b6e0a4e2cc889537e09252054f7a`

Fresh engine verification and the independently reviewed Gateway snapshot
delivery are recorded in tasks 00 and 05. The shared WafDynamic run passed;
compiler promotion and performance acceptance remain open. This continuation
does not authorize a commit or push.
