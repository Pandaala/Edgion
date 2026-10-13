---
name: bounded-xml-primitives
description: Review XML value identity, namespace parsing, producer/work limits and canonical request-body publication.
knowledge_type: regression-guards
---

# XML primitives preserve bounded ownership and publication

## Contract and scope

XML is an immutable opaque Arc node, not a general XML-to-JSON convention.
Expanded names, ordinary attributes, mixed child order and repeated elements
remain semantic. Namespace declarations are scope metadata; deterministic
serialization may change prefixes, omit validated comments/PIs/declarations and
convert CDATA to text. Values are request-side or pure string computation;
there is no response-body aggregation or resolver.

`xmlparser` supplies allocation-free XML 1.0 tokens, not semantic correctness.
The owning leaf validates one complete root, close-name order, NCNames, Unicode
XML characters, namespaces/reserved bindings, late declarations, duplicate
lexical and expanded attributes, whitespace/reference normalization and disabled
DTD/custom entities/XInclude. Do not infer these guarantees from tokenizer success.

## Resource and work boundaries

Source/output/decoded expanded subtree payload are independently bounded at
1 MiB; depth 32; expanded nodes/attributes 4,096; per-element ordinary attributes
and namespace declarations 64 each; names 256 bytes and URI 1,024 bytes. Cached
summaries count each repeated edge, not unique Arc allocations. Private constructors
enforce the summaries; neither bytecode nor Maps can forge Xml handles.
Serialization reuses equal namespace URIs, including the element's prefix,
charges URI lookups and bounded binding/prefix allocation, and enforces 64
generated declarations per element. Standalone subtrees needing more local
declarations fail with the typed namespace limit; never emit output that evades
the declaration ceiling.

The non-refundable ValueWorkBudget charges lexical source before tokenization,
producer payload/capacity, node/frame/binding allocation, identity comparisons,
Map-entry admission, traversal and every serialized expanded visit/output byte.
Output/String capacity uses capped growth, not buffer-then-check. Final Arc copy
is charged separately; Bytes consumes String. The ledger bounds additional XML
producer state per execution, excluding allocator rounding and preexisting storage;
it is not process-wide memory admission. Constants and charges are documented in the maintained
[XML security ledger](../../../01-architecture/02-gateway/dsl-engine-security-model.md#3-compile-time-limits--source-map).
The original task design ledger is no longer present in the tree.

## Decoded token ownership

ARCH-201 keeps unchanged decoded tokens borrowed until the parser creates their
owned Arc nodes or namespace bindings. On the first entity or whitespace
transformation, the private decoder copies the already admitted prefix once and
continues with bounded owned output. No source-backed slice escapes in the tree.
The existing character/reference walk still performs every emitted-byte precharge,
length check and append charge in that order, including unchanged input. Prefix
materialization does not debit those bytes again. Serialization shares the same
append admission and capped reservation owner.

Private decoder tests verify borrowed/owned results, normalization, exact work and
byte admission, competing-error order and remaining work after failure. Existing
leaf and Gateway XML tests cover owned nodes, request memory/spill scanning and
publication. This avoids one intermediate payload for unchanged tokens; final Arc
copies remain, and no measured latency or whole-operation memory claim is made.

## Canonical Body runtime

The lexical boundary must validate PI separator whitespace from the borrowed
token span; xmlparser accepts malformed `<?pi!data?>` without this check. Empty
and whitespace-separated PIs remain valid. The supported declaration subset
requires ASCII space immediately after `<?xml`; tab/CR/LF there safely return
nil and are documented/tested as unsupported rather than adding an input copy.

Body source admission checks the current resolved physical cap before parsing.
DslBodyOps reserves/settles the shared BodyScanBudget and an owned scanner runs
on the canonical blocking lane for memory and disk. Its transient input is bounded
and released. Moving the live work account leaves an exhausted VM placeholder;
restore the returned account before handling any result. Storage/worker failure
terminates fixed fail-close; cancellation retains the worker permit until bounded
finish exits, without claiming immediate parser interruption.

XmlSetBody is both a capture reference and a mutation for source and independently
validated bytecode, including string-only construction. Stage rejection and the
session's replay/handler/transcoding policy remain canonical. Serialize completely
under XML/output/physical bounds before one overwrite; failure cannot publish
partial output. Existing retry, mirror and framing mechanisms use the final generation.

Malformed/unsupported parse is nil; scripts decide whether to deny it. Body
resource/storage/work failures and set_body output/publication failures are fixed
BodyAccess 500 regardless of Ignore. Pure-string limits and construction/type
errors retain the ordinary DSL policy. Implicit Xml coercion is rejected and
equality is O(1) identity rather than uncharged structural comparison.

## Verification and re-evaluation

Owning tests cover tokenizer semantic gaps, namespace/order/reference behavior,
hard bounds, cumulative retained allocation/work, Arc expansion, output rollback,
source/wire arity and stage parity, scanner memory/spill and one-shot publication.
Live evidence and the fresh independent final verdict belong in the XML task
implementation record; this page does not substitute historical tests for a rerun.

Re-evaluate on parser upgrades, new node kinds, serialization/prefix changes,
budget/capacity changes, handle escaping, bytecode dispatch or session/body owner
changes. Preserve the leaf dependency boundary and 24-byte Value layout.
