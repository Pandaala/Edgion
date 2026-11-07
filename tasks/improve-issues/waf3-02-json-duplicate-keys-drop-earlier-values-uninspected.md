# waf3-02: JSON duplicate keys discard every earlier value before inspection — payload in the first occurrence bypasses all ARGS rules

- **Status**: implemented in the T14 D-2 decision commit (`feat(waf)!: publish every JSON duplicate-key occurrence as an ARGS candidate`), awaiting user keep/revert. The follow-up `refactor(waf): remove the unused JSON discard hooks` deleted the then-uncalled `JsonCheckerSink::discard`, its collector impl and `ResultStore::discard`; a revert of D-2 must revert that commit too. The evidence below cites the pre-D-2 code.
- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification)
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/json_checker.rs:560-587` (`Reducer::key`: on a duplicate key, `sink.discard(receipt)` and the member value is reset)
  - `edgion-waf/src/seclang/json_inspection.rs` `ResultStore::discard` (removes the discarded candidates together with their match outcomes, captures and profiles)
  - `edgion-waf/src/seclang/json_inspection_collector.rs` `JsonCheckerSink::discard` (forwards to the store)
  - `edgion-waf/src/seclang/compiler.rs:1406-1412` (comment acknowledging duplicate replacement semantics)
  - test `tests/unit/seclang/json_inspection_runtime_test.rs` fixture `duplicate_last_miss` (256 `"same":"hit"` followed by `"same":"miss"` → `@pm hit` sees one live candidate, no hit)

## Summary
The native JSON processor implements "last duplicate wins" by deleting the facts of every earlier
occurrence of a key inside the same object, including predicate matches that were already computed. Only
the final value is published as an `ARGS`/`ARGS_NAMES` candidate. An attacker therefore sends
`{"q":"1' UNION SELECT password FROM users--","q":"x"}`: CRS sees only `json.q = "x"`, while any backend
whose JSON library returns the *first* occurrence (Go `gjson`/`fastjson` `Get`, simdjson on-demand
`find_field`, many hand-rolled or streaming parsers) executes the payload. Duplicate keys are not rejected,
so the request is forwarded unchanged.

## Current behavior (evidence)
- `Reducer::key` (json_checker.rs:571-579): `insert` returns `fresh == false` for an existing decoded key;
  the code then calls `sink.discard(receipt)` on the previous value's receipt and replaces `node.value`
  with an empty subtree. Decoded identity means `"q"` and `"q"` also collide.
- `ResultStore::discard` walks the receipt chain and removes candidates, their outcomes
  (`free_outcome`), profiles/changes and name/deferred leases — the previously recorded match is gone
  before sealing, so no rule in phase 2 can observe it.
- The JSON grammar is otherwise strict (syntax errors fail closed in `On`), so the duplicate path is the
  one structurally valid way to make the WAF and the application disagree on the value of a field.
- `skills/04-review/waf/regression-guards/fused-json-inspection.md` describes the behavior
  ("Duplicate identity is decoded key within one object instance. Replacement discards the preceding
  subtree's summary and provisional results") but contains no security analysis of parser differentials.

## Industry comparison
- ModSecurity v3 (`request_body_processor/json.cc`, yajl callbacks) calls `addArgument` for every scalar,
  so each duplicate becomes its own `ARGS` entry and is inspected.
- Coraza stores JSON args in a map (last wins) — this class of bypass has been reported against it; it is
  not a property to copy.
- I-JSON (RFC 7493) requires unique member names; several API gateways/WAFs offer a "reject duplicate
  keys" switch for exactly this differential.

## Impact
Complete CRS bypass for JSON APIs (the dominant API Gateway / AI Gateway body type) whenever the upstream
JSON library is first-wins or schema-binds the first occurrence. It also defeats operator-written
`ARGS:field` rules on JSON fields. No log trace: the request is clean from the WAF's point of view.

## Suggested direction
Option A (recommended, ModSecurity-compatible): keep the reducer's last-wins *summary* for metadata if
needed, but do not discard candidates/outcomes of earlier duplicates — publish every occurrence as its own
candidate (same qualified name), so rules and anomaly scoring see all values. Counts (`&ARGS`,
`ARGS_COMBINED_SIZE`) should then count every occurrence, matching ModSecurity.

Option B: treat a duplicate key in the same object as malformed JSON (fail closed in `On`, fallback to
raw in `DetectionOnly`), optionally behind a policy knob defaulting to reject.

Either way add regression cases for first-position payloads, decoded-escape duplicates (`"q"`), and
nested-object duplicates.

## Notes
- Not covered by existing findings (`waf-01..06`); `tasks/` has no item for JSON duplicate semantics.
- The regression guard records the mechanism as intentional for memory/ordering reasons; this finding
  adds the missing evidence that the semantics create a parser-differential bypass, which outweighs the
  retention savings (earlier duplicates are bounded by the same candidate/record limits).

## Verification (independent)

- **Verdict**: DOWNGRADED — P1 → P2.
- Mechanism confirmed: `Reducer::key` (`edgion-waf/src/seclang/json_checker.rs:560-580`) calls `sink.discard(receipt)` and resets the subtree when `insert` reports a non-fresh key. `compiler.rs:1406-1410` acknowledges replacement semantics. The user doc (`waf.md:548`) and `fused-json-inspection.md:34` describe last-wins without any parser-differential analysis.
- Reason for the downgrade: the bypass needs a *first-wins* backend parser. Mainstream decoders (Go `encoding/json`, Python `json`, JS `JSON.parse`, Jackson by default, `serde_json` maps) are last-wins and agree with Edgion. The exploitable set (gjson/fastjson `Get`, simdjson on-demand, hand-rolled scanners) is real but a minority. This is a conditional parser-differential bypass, not a general CRS bypass. The ModSecurity comparison (every occurrence becomes its own ARGS entry) is accurate.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-waf/src/seclang/json_checker.rs:560-587` `Reducer::key` still calls `sink.discard(receipt)` on duplicate keys; `compiler.rs:1406-1412` unchanged.
- `json_inspection.rs` changed only the owner-ID counter call (`:12`); `ResultStore::discard` is unchanged.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
