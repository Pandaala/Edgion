# gw-ext3-02: DSL `json_get`/`json_select` accept duplicate JSON keys (last wins) while `json.*` rejects them — a parser differential inside one sandbox

- **Category**: design-error
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs:237-260` (`parse_json`), `body_scanners.rs:289-310` (`SelectVisitor::visit_map`: every matching key overwrites `selected`), `body_scanners.rs:515-530,725-760` (`parse_json_select` / `JsonSelectVisitor::visit_map`), `body_scanners.rs:1068-1100` (`BoundedMapAccumulator::insert` replaces earlier entries), `edgion-gateway/src/plugins/http/dsl/lang/vm.rs:869-930` (`OpCode::JsonGet` / `OpCode::JsonSelect`), versus `edgion-dsl/src/json.rs:14,216-220,309-325` (`StrictJsonError::DuplicateKey`, `admit_key`) and `edgion-gateway/src/plugins/http/dsl/lang/json_builtins.rs:44-49,176-200` (`json.read` returns `nil` on duplicates)

## Summary
The DSL has two JSON readers over the same request body with opposite duplicate-key policies.
`edgion-dsl::json` is documented as the "canonical strict JSON policy" and rejects any decoded
duplicate member (`json.read`, `json.get_raw`, `json.get_string`, `json.keys` return `nil`). The
streaming path behind `json_get(...)` and `req.body.json_select({...})` silently accepts duplicates
and returns the *last* occurrence, including inside serialized sub-objects. A script that gates or
routes on `json_get(req.body, "$.role")` therefore evaluates a different value from what a
first-wins or strict upstream parser sees, and a different value from the script's own `json.*`
calls on the same bytes.

## Current behavior (evidence)
- `SelectVisitor::visit_map` loops `while let Some(matches) = map.next_key_seed(...)` and on every
  matching key executes `selected = map.next_value_seed(...)`, so `{"role":"user","role":"admin"}`
  yields `"admin"`.
- `JsonSelectState::reset`/`set` and `BoundedMapAccumulator::insert` (`self.entries.remove(&key)`
  then insert) implement the same replacement for `json_select` captures and for serialized
  object/array values returned by `json_get`.
- `edgion_dsl::json::ParseState::admit_key` fails with `StrictJsonError::DuplicateKey`, and
  `json_builtins.rs::malformed` maps that to `nil` for `json.read` and the `json.get_*` family.
- The user documentation (`docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`,
  `json_select` section) states "Duplicate JSON object keys use the last occurrence, matching
  `json_get`", while the `json.*` section states "Duplicate decoded object keys are rejected
  recursively, including escaped aliases". The two policies are documented but never reconciled,
  and the `json_get` example uses exactly the security-relevant pattern
  (`if role == "admin" { req.set_header("X-Is-Admin", "true") }`).
- Escaped aliases (`"role"`) are decoded by serde before comparison, so they also collapse to
  last-wins on the streaming path.

## Industry comparison
- RFC 8259 §4 says names "SHOULD be unique" and that behavior with duplicates is unpredictable; the
  JSON interoperability research (Bishop Fox, 2021) shows first-wins vs last-wins differences between
  a gateway/WAF and a backend are a practical authorization-bypass class.
- Envoy's JSON-consuming filters and Coraza/ModSecurity JSON body processing in Edgion's own native
  WAF work over one parse policy per component; Kong's `cjson`-based plugins fail on documents the
  backend parses differently only by configuration choice, not by which builtin was called.
- Edgion already chose strict rejection as its canonical policy for the Resources DOM parser and DSL
  `json.*`; the streaming builtins are the outlier.

## Impact
Scripts that use `json_get`/`json_select` for authorization, tenant routing, or header derivation
can be steered by a client that repeats a key: the DSL sees the last value while a first-wins or
strict backend (or a later `json.read`-based check in the same script) sees another. Operators
reading the docs cannot tell which builtin is safe for security decisions.

## Suggested direction
Make duplicate members a parse failure on the streaming path too: track the decoded member names of
each open object (bounded by the existing depth/limit, or only for objects on a selected path plus
serialized captures) and return `Missing` / all-`nil` exactly as for malformed JSON. If a
last-wins mode is wanted for compatibility, make it an explicit opt-in. Update the `json_get` /
`json_select` documentation to state one canonical policy.

## Notes
- Checked `skills/04-review/dsl/regression-guards/json-body-scan-single-blocking-job.md` (lists
  "duplicate-key replacement" as JSON policy kept in `body_scanners.rs`; it governs scan-lane
  structure, not policy correctness) and `bounded-json-reconstruction.md` (strict policy for
  `json.*`). Neither page justifies two policies; this is filed as new evidence that the documented
  divergence is a security-relevant inconsistency.

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P2 -> P3.
- `SelectVisitor::visit_map` (`body_scanners.rs:289-309`) overwrites `selected` on each matching key; `edgion_dsl::json` rejects duplicates for `json.*`.
- The divergence is documented in `api-reference.md:1581-1582` ("Duplicate JSON object keys use the last occurrence, matching `json_get`") vs `:2486`; `json-body-scan-single-blocking-job.md` names "duplicate-key replacement" as the kept policy but gives no security rationale.
- Downgrade reason: last-wins matches the dominant backend parsers (JavaScript `JSON.parse`, Python `json`, Go `encoding/json`), so a practical differential needs a first-wins or strict backend. The internal inconsistency is real and cheap to resolve.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `dsl/lang/body_scanners.rs`, `vm.rs`, `json_builtins.rs` and `edgion-dsl/src/json.rs` are unchanged; `json.get`/`json.select` still keep the last duplicate while `json.read` rejects duplicates.
- DSL review changes (`bounded-json-reconstruction.md`, `host-result-byte-budgets.md`) concern Redis/HTTP result budgets, not duplicate-key semantics.
