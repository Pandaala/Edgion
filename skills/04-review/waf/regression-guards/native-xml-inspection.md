---
name: native-xml-inspection
description: Review bounded native XML parsing, compiled facts, canonical replay and request-body ownership.
knowledge_type: regression-guards
---

# Native XML inspection preserves bounded parsing and ordered effects

## Conclusion and scope

Full WAF inspection uses XmlStream and XmlInspectionSession for effective XML
processors. The parser validates a bounded UTF-8/XML 1.0 inspection subset without
a DOM or complete extracted value map. Reachable compiled static predicates run as values
complete; Phase-2 capture, chain, TX and control effects derived from XML values
run only in canonical rule order after complete source admission. Phase-1 effects
execute before inspection and can select the processor. This is not arbitrary
XPath or schema validation.

## Ownership and projection

`xml_stream.rs` owns lexical validation, namespace scope and duplicate attributes.
It borrows complete contiguous ordinary values during synchronous callbacks.
Escapes, normalization and cross-chunk values use reusable 1024-byte inline token
scratch. Names and the structural arena each have 256 inline bytes. Inline storage
may live inside an async future; it is not necessarily thread-stack memory.
Fallible growth preserves capacity for reuse and never shrinks on the hot path.

Open-element marks reclaim live names and namespace bindings at close. Ordinary
attribute identities are reclaimed after start-tag validation. Lexical and expanded
attribute identities use charged in-place sorting, not an unbounded per-tag map.
After lexical uniqueness, unprefixed ordinary attributes have unique expanded
names because their namespace is empty; they need no expanded sort. Any prefixed
ordinary attribute retains expanded checks. Namespace declarations and element
bindings still validate independently, including late aliases and reserved names.
Namespace declaration order within a tag does not affect resolution.
Prefixed attributes reuse the already charged QName split when resolving bindings;
elements charge their own split once. Binding scans remain charged. Preserve
unbound-prefix Syntax before later work: exact-budget and one-less tests distinguish
malformed DetectionOnly fallback from a physical fail-close.

Attributes, including namespace declarations, are observed in source order under
their lexical QName. Normal text between markup is a separate candidate with outer
literal XML whitespace trimmed before entity decoding; CDATA is separate and
untrimmed, including empty CDATA. Comments, PI and element names are not values.
CR/CRLF normalization and attribute literal whitespace follow the supported XML
profile; character-reference whitespace remains meaningful.

Public selectors are `XML`, `XML:/*` and `XML://@*`. Existing private Literal/Regex
attribute IR is tested without expanding source syntax. Unknown paths and dynamic
XML value operands fail strict Full admission for inline and resolved bundles.
Dynamic count operands use canonical decimal count evaluation. XML values do not
contribute to ARGS; combined-size metadata remains query-only.

The shared inspection domain reuses compiled profiles, selected-result storage,
opaque ownership-checked handles and the transaction scheduler. XML does not use
the JSON-only no-hit summary proof. Required candidate metadata can still grow
with values; parser arena reuse is not a constant-memory whole-transaction claim.
Broad-only XML dispatch reuses precompiled profile order in the existing text and
attribute vectors; mixed private Literal/Regex IR keeps its charged fallback.
XML uses the canonical [demand-driven scratch and static activation](fused-json-inspection.md#demand-driven-scratch-and-static-activation)
proof, all-consumer mask and checked sealed lookup. XML grammar, namespaces,
duplicate attributes and metadata remain mandatory when every static group is
pruned. Non-TX Phase2 target exclusions retain canonical effects and cannot
suppress a TX-only gate. XML does not use the JSON summary shortcut.
Transient XML selection kinds come only from checked candidate metadata. Broad
selectors need no name read; named selectors, capture and matched-variable reads
retain their checked paths. Static replay reuses the profile returned by the same
checked outcome lookup. Charge every remaining read and preserve sticky errors.

## Failure and budget boundaries

Source/raw token, names, definitive decoded bytes, events, depth, attributes,
namespace bindings, active structural bytes, heap capacity and work have independent
limits. Definitive decoded bytes and known name/structure/growth-copy costs are
admitted before copying or allocation. Pending outer literal whitespace is still
raw/work/heap bounded and is charged when it becomes interior. Fixed markup
discriminators consume raw/work/heap budget, not lexical/structural name budget.
Successful growth refreshes actual retained capacity even if a later step fails.
Spare-capacity token append still admits copy work before pushing each byte;
capacity and structural identity do not change, so no aggregate heap refresh is
needed. Growth/overlap admission and all lexical/decoded checks remain intact.

Whitespace-only chunks must have linear scanning work. PI data requires separator
whitespace unless the target immediately closes with `?>`. UTF-8/XML characters
are checked even in ignored markup. DTD and external/general entities are rejected;
there is no entity network IO, recursive expansion or encoding transcoding.

Physical, allocation and arithmetic errors are sticky and fail closed even in
DetectionOnly. Malformed DetectionOnly input discards provisional facts and uses
existing metadata/raw visibility policy. XML selected-result budget errors stay
XmlResultLimit through selector, match, macro/capture reads and subsequent
preflight; parser physical limits use XmlStream limits and logical BodyLimits
retain Body errors. JSON errors keep their original domain. Later multiMatch
transformation failure can override
an earlier provisional hit; failed chains roll back staged TX/capture effects.

Only the adapter accepts empty transport bodies. Nonempty whitespace/comment/PI
documents require a root and fail grammar validation. Completion is single-use;
unfinished Drop poisons the transaction. No XML-derived Phase-2 effects publish before true EOF,
source parity, physical and logical admission and sealing.

## Gateway and evidence

The shared JSON/XML driver uses canonical BodyStorage actual capture chunks or
bounded immutable snapshot chunks. Cached size is admitted before IO. Required
raw reads preserve existing transport/read-error priority and occur only under
effective visibility/fallback policy. Fresh XML-only spill has no complete
materialization; BodyStorage remains the sole forwarding/retry cache. Cancellation
and errors release provisional accounting through RAII.

Source/test owners are the mirrored XML parser/runtime tests, public XML API tests,
Gateway capture tests, resource strict-admission tests and shared WafDynamic XML
wire/generation/mode scenarios. Legacy XML helpers live only in the body unit-test
oracle; their historical test cases are not proof of native parser correctness.
Review and verification were tracked in the historical
`tasks/pending/waf-xml-checker/waf-xml-checker.md` task, which is no longer present
in the working tree.
Historical results do not establish correctness after a relevant source change.

Heap-capacity measurements exclude allocator overhead, BodyStorage, inline storage
and unreported transaction scratch. Metered work includes admitted inline copies
and scans; it does not measure total engine CPU or allocator work.
Independent component peaks cannot be added
into a simultaneous total. Re-evaluate on selector/projection, budget, source
ownership, handle lifetime or scheduling changes; measure strict grammar costs
separately from the frozen old common-valid subset.
