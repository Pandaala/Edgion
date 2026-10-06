# Implement the Incremental JSON Tokenizer

## Authoritative checkpoint (2026-10-06)

Status: Completed native lexical foundation.

The production parser is `JsonStream`, using borrowed contiguous tokens and reusable inline/spilled scratch. Native `json_number` normalization and `zmij` formatting replace the historical `serde_json::Number` dependency. Production `edgion-waf` uses no serde_json parser or DOM. Actual network chunk delivery reuses this parser rather than adding another grammar implementation.

Current implementation, verification and independent review are tracked in
[plan 16](16-json-capture-wiring-and-closure.md) and
[validation task 06](06-json-validation-and-benchmark.md).
These checkpoints supersede the historical interfaces and acceptance statuses
below. Historical measurements retain their original scope and revisions.

## Historical checkpoints (superseded design)

## Meta

| Key | Value |
|-----|-------|
| Status | completed (lexical foundation only) |
| Priority | P1 |
| Depends on | `01-structured-streaming-contract.md` |

## Goal

Implement an in-crate JSON tokenizer that accepts arbitrary byte chunks and
retains only bounded token and parser state. It must not retain the complete
JSON document or depend on an external streaming parser.

## Work

- Add a transport-neutral `Data`/`Complete`/`Abandoned` input contract.
- Incrementally recognize objects, arrays, names, strings, numbers, booleans,
  null, separators, and whitespace.
- Preserve string escape and UTF-8 decoder state across chunk boundaries.
- Validate JSON number grammar without copying the full document.
- Enforce token-byte, decoded-byte, depth, entry, retained-byte, and work
  limits before state growth.
- Reject malformed input, incomplete EOS, invalid UTF-8, invalid escapes, and
  invalid surrogate pairs with typed, payload-free errors.
- Expose exact retained-memory accounting for request metrics and tests.

## Tests

- Split every valid and invalid fixture at every byte boundary.
- Cover escape sequences, UTF-8, surrogate pairs, long numbers, empty values,
  nesting, trailing data, truncated tokens, and abandoned streams.
- Assert retained memory is independent of total input length once configured
  token bounds are fixed.
- Run deterministic chunk-layout property tests against the existing complete
  JSON parser for syntax acceptance.

## Exit gate

The tokenizer produces identical syntax outcomes for whole-input and arbitrary
chunked input, fails within declared bounds, and retains no full document.

## S2 implementation boundary

The tokenizer foundation proceeds independently of task 01's unresolved
publication proof: syntax recognition does not depend on duplicate-key winner
selection or WAF rule scheduling. Task 01 remains the gate for promoting rules
and task 03 publication; no Gateway or transaction consumer is wired in S2.

Public leaf API: `JsonStream`, `JsonStreamInput::{Data, Complete, Abandoned}`,
borrowed `JsonStreamToken` callbacks, `JsonStreamLimits`, `JsonStreamUsage`,
and payload-free errors. All events are provisional until `Complete` succeeds.
Even a syntactically valid early scalar cannot authorize a WAF verdict. The
callback must consume/copy borrowed token bytes synchronously; the tokenizer
retains neither the source document nor an event list.

### Independent lexical budgets

| Limit | Default | Accounting |
|---|---:|---|
| Source bytes | 16 MiB | Every consumed Data byte, independent of chunk layout |
| Raw token bytes | 1 MiB | Source spelling including quotes/escapes; structural punctuation is one byte |
| Decoded bytes | 32 MiB | Decoded keys/strings and raw number/literal spelling, including overwritten duplicates |
| Events | 131,072 | Key/scalar/container-start/container-end callbacks; commas/colons are not events |
| Depth | 64 | Root value is zero; each child value adds one; keys add no depth |
| Retained allocation | 2 MiB | Token and grammar-stack Vec capacities, charged before requested growth and checked after reserve |
| Work | 128 MiB units | Source visits, decoded writes, validation scans, growth-copy lengths, number-delimiter reprocessing and completion |

These limits do not replace `BodyLimits`: existing publication budgets count
surviving flattened entries after duplicate replacement. The lexical parser
must also bound the work of values later discarded. Usage records the live
capacity sum and its peak, excluding fixed `size_of::<JsonStream>()`, allocator
bookkeeping and transient realloc peaks; it is not process RSS. The retained
limit controls variable owned state. No public configuration field changes.

### Syntax and lifecycle

- Container grammar rejects trailing commas, missing separators, extra roots
  and non-JSON whitespace. Empty lexical input is invalid; the existing empty
  HTTP-body adapter exception remains outside the tokenizer.
- Keys/strings decode JSON escapes and validate UTF-8 across arbitrary chunks;
  malformed UTF-8 and unpaired surrogates fail with payload-free typed errors.
- Numbers retain original spelling in events. A number DFA finds token ends;
  the existing bounded `serde_json::Number` parser checks finite-range behavior.
  Future publication must apply the complete processor's Number normalization.
- A later duplicate remains a distinct lexical event. Key replacement, sorted
  object publication, array-length pseudo-values and path naming belong to the
  publisher, not this parser.
- Completion validates exactly one complete root. Failure, abandonment and
  successful completion release both owned buffers. Errors are sticky; a
  successfully completed instance rejects all subsequent input. Token consumer
  rejection is terminal and carries no arbitrary diagnostic payload.

### Review and verification status

Implementation and 15 targeted tests passed. Two independent reviewers each
completed two full rounds and approved the final tokenizer/inventory/contracts.
Review follow-ups added 1-4 KiB numeric boundary cases, independent stack-memory
limits and zero-valued budgets. Full WAF library tests passed: 415 passed,
2 ignored. Workspace check/Clippy, formatting, documentation and static guards passed.
The explicit pinned CRS inventory passed; an altered input corpus failed before
report publication as expected. The dependency audit remains an unchanged
failing baseline as detailed below.
Do not treat this checkpoint as proof that structured WAF execution is available.

### S2 dependency audit

Inventory adds only the existing workspace `sha2` as a dev dependency; there
are no new resolved package versions or production dependencies. cargo-audit
was installed into `/tmp/edgion-waf-audit-tools` for verification. Its Git fetch
failed; the official RustSec archive at revision
`f23b768236fe2880e4cfa167da662cad8ca79240` was downloaded and audited with
`--no-fetch --db` instead. Both the baseline and working lockfile exit 1 with
the identical two advisories: `RUSTSEC-2026-0119` (Hickory, already explained in
`deny.toml`) and `RUSTSEC-2026-0316` (Wasmtime, triage tracked in
`../todo/wasmtime-rustsec-2026-0316-triage.md`). No policy exemption was added.
This is a verified unchanged audit baseline, not a clean audit result.

### Prior-art scope

[Coraza v3.7.0](https://github.com/corazawaf/coraza/blob/v3.7.0/internal/bodyprocessors/json.go)
reuses path storage while traversing its complete-body representation.
[KrakenWaf's Rust body decoder](https://github.com/Orangewarrior/KrakenWaf/blob/main/src/body_decode.rs)
checks decoded byte size before appending each block. S2 applies reusable
scratch and pre-growth accounting to an independent tokenizer; it does not
copy their body-aggregation architecture or add either engine as a dependency.
These observations are design evidence, not comparative performance results.

### Final S2 checkpoint

Baseline `582dae37`, branch `feature-08-30`. The complete code/test diff was
reviewed by two independent agents in two rounds. Parent read the tokenizer and
tests directly. No unresolved tokenizer/inventory review finding remains.

Exact commands and results:

- `cargo test -p edgion-waf --lib json_stream`: 15 passed.
- `cargo test -p edgion-waf --lib --quiet`: 415 passed, 2 ignored, exit 0
  (`/tmp/waf-s2-tests.log`).
- The explicit inventory command in task 01: 1 passed, exit 0
  (`/tmp/waf-s2-inventory.log`). Modified-corpus negative probe: source digest
  rejected and no report created (`/tmp/waf-s2-inventory-negative.log`).
- `cargo check --workspace --all-targets`: exit 0 (`/tmp/waf-s2-check.log`).
- `cargo clippy --workspace --all-targets`: exit 0 (`/tmp/waf-s2-clippy.log`).
- `cargo fmt --all -- --check`, `git diff --check`: exit 0.
- Agent docs, SSA-force, metrics inventory, Pingora source, Gateway API transform
  contract and unit-test layout: passed (954 mounted unit-test files).
- Audit working/baseline JSON vulnerability and warning records compare exactly
  equal (`/tmp/waf-s2-audit.json`, `/tmp/waf-s2-baseline-audit.json`); both exit 1.

No full workspace tests, FTW verdict suite, Gateway integration or throughput
benchmark was run for this isolated lexical batch. Next: finish task 01's exact
eligibility/metadata budget proof, then implement task 03's bounded result tree.
Do not skip that gate merely because task 02 is complete.

Reviewed/verified code SHA-256:

- `edgion-waf/src/seclang/json_stream.rs`: `4fe15156ad4b6ae99c69e15a6d3a61753b66e850dcb11db678822c6a86d5fb9f`
- `edgion-waf/src/seclang/mod.rs`: `3c1ec968a5220c7d258929944dc61332c153df0d2b296d473e2898d2f5fd5ea9`
- `edgion-waf/tests/unit/seclang/json_stream_test.rs`: `b969818638970d6f7502f2920d92c86e32bcbbec31612018d3348ea96041296d`
- `edgion-waf/tests/ftw_inventory.rs`: `d6bf1c10f909c013899fedf90d948b5f9d5d62797b22a335cacbca7af6d48d57`
