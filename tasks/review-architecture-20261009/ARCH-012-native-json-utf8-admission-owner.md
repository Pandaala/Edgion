---
id: ARCH-012
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-244
independent_verification: confirmed
---

# Own native JSON UTF-8 admission independently of observation

## Evidence and ownership gap

Native Messages `ai/protocols/anthropic/request.rs::fields` checks the root and
selectively deserializes metadata without validating UTF-8 throughout the body.
Chat and Responses perform full-slice validation before equivalent selective
deserialization. The locked serde_json 1.0.151 ignores unknown strings through
SliceRead::ignore_str, which scans framing and controls without checking UTF-8.

Source-derived input `b"{\"model\":\"m\",\"max_tokens\":1,\"extension\":\"\xff\"}"`
can satisfy Messages metadata selection. Here `\xff` denotes a raw invalid byte.
AiProxy canonicalizes native Messages while retaining the original Bytes.
The later character scan runs only for Provider observation or CostWeighted
selection; unchanged-model dispatch can forward the original bytes directly.
When that optional scan rejects the bytes, it classifies them as BackendUnavailable,
rather than the fixed malformed-request 400.

Admission therefore depends on unrelated observation and selection policy.
The existing UTF-8 correction and regressions cover Chat/Responses but leave
equivalent Messages ingress outside that ownership boundary. Native opaque-field
preservation does not authorize malformed JSON. This is a static call-chain
conclusion, with no executed request or claim about provider exploitation.

## Bounded recommendation and alternatives

Give the existing native request-helper boundary an allocation-free full-slice
UTF-8 admission operation before protocol-specific selective metadata parsing.
Adding that check directly to Messages is the smallest correction; a small
shared helper can prevent the same invariant drifting across all three native
entry points. Returning a borrowed validated string suffices. Keep field types,
duplicate selected-key checks and capability inspection protocol-specific.

The scan is linear over the physically bounded request body and creates no
content copy. Preserve borrowed RawValue spans and original Bytes identity when
no authorized mutation is needed. A conversation AST, parser framework or new
body cache is unnecessary. Observation and model rewriting cannot own ingress
validity because they are optional.

## Invariants and future validation

Invalid UTF-8 anywhere in managed native JSON must produce MalformedBody before
plan publication or provider dispatch, independently of observation, selection
and model mapping. Valid opaque extension bytes, selected-key ambiguity rules,
native capability restrictions, canonical request storage, physical limits,
deadlines and sensitive-data rules remain unchanged.

Future cases should cover ignored top-level and nested strings, metadata and
rewrite entry points, observation off/provider, unchanged/aliased models, fixed
native 400 rendering and zero provider dispatch. Retain large valid extension
and exact byte-preservation cases. Use the prescribed private unit-test layout.

## Original context and limits

The analyst fully read all five Anthropic and seven OpenAI protocol primaries
and ten protocol test files. Original native Messages preservation/accounting,
Responses observation/continuation, provider subset and malformed-body contracts
were read. The Responses decision explicitly records the ignored-value UTF-8
gap; no current decision accepting policy-dependent Messages admission was found.
Locked dependency sources were inspected directly. Evidence hashes are in the
partition report and STATE.json. Independent verification is pending.

This root is distinct from ARCH-010's form-feed JSON grammar and ARCH-011's
replacement-budget composition. Supporting consumer reads do not claim coverage.
