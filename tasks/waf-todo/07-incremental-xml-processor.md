# Implement the Bounded Incremental XML Processor

## Meta

| Key | Value |
|-----|-------|
| Status | completed: corrected code, quick/live/performance gates and final Astra approval |
| Priority | P1 |
| Depends on | `06-json-validation-and-benchmark.md` |

## Goal

Implement the classified XML request-body subset using the structured stream
interface established for JSON, without external entities, network access, a
DOM, complete extracted value map or additional full-body materialization.
Canonical request BodyStorage still owns forwarding/retry bytes.

The reviewed code-level contract and active evidence live in
[Native XML inspection](../pending/waf-xml-checker/waf-xml-checker.md).
Independent Astra approved the design and corrected code on 2026-10-06. Final
quick/live and complete performance gates passed; independent Astra approved
final evidence and documentation with no unresolved Must Fix.

## Work

- Incrementally tokenize start/end tags, names, attributes, text, CDATA,
  comments, and processing instructions required by the accepted profile.
- Preserve UTF-8 and token state across arbitrary chunk boundaries.
- Maintain bounded element/path and namespace state.
- Reject DTD and external/general entity behavior that could cause expansion,
  file access, or network access.
- Enforce depth, attribute-count, name/value bytes, decoded bytes, entries,
  retained bytes, and parser-work limits.
- Publish XML values through the same transaction structured-value interface.
- Reject unsupported XPath and dynamic XML value operands during strict
  admission; no complete-document fallback is planned. Preserve the current
  text/attribute projection, lexical attribute Literal/Regex selectors, count
  and runtime exclusions through canonical ordered replay.

## Tests

- Every-byte chunk splits around tags, attributes, UTF-8, CDATA, comments, and
  closing delimiters.
- Malformed nesting, duplicate attributes, truncated input, DTD/entity input,
  and limit boundaries.
- Differential value/order/verdict tests on the old parser's reliably handled
  valid subset, with independent tests for specified XML correctness fixes.
- Scoped parser-state plateau tests across sibling-heavy documents. Demanded
  candidate metadata/captures remain independently bounded and may grow; this
  is not a claim of constant memory for the whole transaction or body storage.

## Exit gate

The supported XML inspection projection preserves common valid-reference
semantics across chunks, applies the documented syntax/namespace/normalization
corrections, rejects dangerous expansion behavior and retains no extracted
complete document. Actual Full capture, raw fallback priority, JSON regression,
same-host release evidence and final independent Astra review are required.

## Completed checkpoint (2026-10-06)

Fresh final leaf units: 679 passed, 2 existing ignored; preserved identities
643 to 681, removed 0, added 38. XML API 3, JSON API 9, Gateway capture 10 and
resource XML 2 passed. Workspace check/Clippy, format, static guards and final
canonical WafDynamic passed. All 12 serial release runs exited 0: XML 352 valid
old/final digest matches, each XML/JSON chunk direction 2080/96 rows. Full
regressions, capacities and measurement limits are in the linked results.

This closes the bounded request XML subset, not arbitrary XPath, full CRS/FTW,
general classifier promotion or response Phase 4. Task 08 is the next queued
planning scope; it has not started.
