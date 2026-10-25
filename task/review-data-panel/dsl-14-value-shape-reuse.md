# Evaluate reuse of DSL Value shape metadata

Status: deferred for user decision. No runtime change.
Source baseline: d7f0f0eff.

## Evidence and distinct scope

The canonical `edgion-dsl/src/value.rs::Value::inspect_shape` walks Map/List
containers to derive depth, total items, Map-key bytes and string bytes. It
charges the execution's ValueWorkBudget during traversal. Scalar Str contributes
its length without scanning its payload. Fixed limits are depth 64, 32768
container items, 262144 Map-key bytes and 16 Mi work units; a container item
costs 32 units, with additional key/comparison byte accounting.

Gateway `plugins/http/dsl/lang/vm.rs` has repeated consumers:

- GetLocal inspects the source, charges clone work, clones it, then pushes the
  result through stack admission and another container inspection.
- Ordinary Equal/NotEqual inspect both operands and charge comparison work
  before content comparison. Body/string equality takes its separate scan path.
- Map IndexGet inspects and conservatively charges the selected value before
  moving it from the consumed Map. It does not physically clone that value;
  the retained charge is deliberate, and result push may inspect it again.
- MakeMap preflights the supplied stack pairs before allocating/converting keys,
  then builds the Map and pushes it. Duplicate keys can collapse during build,
  so the preflight and final Map are not interchangeable shape inputs.

Repeated traversal is source evidence, not a measured bottleneck. Existing
work-budget tests deliberately reject repeated wide copies and comparisons.
[DSL-08](dsl-08-list-loop-source-ownership.md) concerns repeated physical List
copies in compiled loops; this task concerns metadata traversal across several
operations even when a payload is already shared or moved.
[DSL-01](dsl-01-string-consumer-ownership.md) remains the separate coercion issue.

## Decision requested

Retain traversal-derived metadata, or profile a bounded representation that
reuses validated shape information while preserving admission and accounting.
No representation, cache or implementation is selected. A cached total is not
a substitute for traversal-prefix error semantics, and it must never cache the
remaining per-execution budget. Construction cost and metadata overhead may
outweigh repeated walks for small or short-lived values.

## Acceptance before implementation

- Use IMPL with an exact reviewed source allow-list. Preserve the canonical
  runtime-independent Value owner, the 24-byte Value layout and boxed Map
  contract. A whole-Value copy-on-write migration or Gateway dependency in the
  leaf crate is not authorized. Metadata storage and lifetime need independent
  bounds; no global/request lookup cache is implied.
- Audit every Value producer and mutation, including stack/local transfer,
  container construction, duplicate replacement, selected-value moves, host
  results and JSON/XML boundaries. Specify construction, invalidation and alias
  rules before relying on cached data. Preserve Body/XML opaque/identity
  behavior; do not recursively inspect their host state as ordinary Maps.
- Preserve fixed shape limits and exact ValueWorkBudget deductions, including
  inspection, conservative clone and comparison charges. Removing a physical
  walk or copy does not authorize previously rejected scripts to succeed.
  Preserve maxSteps, maxCallCount, loop and stack limits and error precedence.
- Keep stack-depth checks and inspection at their existing logical stages.
  Aggregating charges can change which limit fails first; prove equivalence of
  traversal prefixes, child-depth checks and Map accumulation failures rather
  than comparing only successful final totals. MakeMap's pre-dedup validation
  and final result admission remain distinct.
- Do not skip accounting because two Arcs are identical or a comparison could
  short-circuit. Existing content equality, iteration order, original aliases,
  non-container cases and mixed-type behavior remain authoritative. Preserve
  selected Map values' conservative copy charge even though extraction moves.
- Measure repeated versus one-shot shallow/deep/wide values, shared strings,
  duplicate keys, nested Lists/Maps and rejected inputs. Include preparation,
  copied keys, traversal work, metadata retention, stack size and total peak
  memory. No request-throughput conclusion follows from fewer source loops.
- Retain leaf shape/depth/key/item/work tests and Gateway sandbox tests; add
  differential exact-budget and competing-error tests for any chosen design.
  No logging, metrics, new body owner, response aggregation or new configuration
  knob is authorized by this deferred proposal.

Original contracts: `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
and canonical `edgion-dsl/src/value.rs`. Existing evidence includes
`edgion-dsl/tests/unit/value_test.rs` and Gateway mirrored
`plugins/http/dsl/mod_test/sandbox.rs`; these tests verify current behavior,
not acceptance of metadata caching or a changed accounting policy.
