# DSL XML Primitives

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Queue order | 2 |
| Status | pending |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add safe, bounded XML parsing and construction primitives to EdgionDSL. The
API should expose enough XML structure for scripts to implement SOAP, legacy
integration, and application-specific mappings without imposing a universal
XML-to-JSON convention. Parsing and serialization must remain bounded,
transactional, and compatible with the existing request-body storage and
streaming-response boundaries.

## Requirements

- Define an XML value model that preserves namespace identity, attributes,
  child order, text, and repeated elements.
- Define DSL parse, traversal, construction, and serialization APIs.
- Charge parsing, traversal, and serialization against bounded VM work.
- Reuse the request-scoped `BodyStorage`; do not add another request cache.
- Enforce the resolved physical body cap before parsing.
- Enforce serialized-output bounds before publishing a mutation.
- Define explicit limits for depth, nodes, attributes, names, and text.
- Disable DTDs, external entities, external schemas, XInclude, network access,
  and unbounded entity expansion.
- Define deterministic namespace handling, escaping, and malformed-input
  behavior.
- Preserve transactional mutation: invalid or oversized output is never
  partially published.
- Preserve request replay, retry, mirroring, framing, and stock-Pingora
  behavior.

## Scope

### In scope

- Bounded XML request-body parsing.
- Script-visible XML traversal and value extraction.
- Bounded XML element/attribute/text construction.
- Serialization into a request-body mutation with validation before publish.
- DSL compiler/bytecode validation, Gateway runtime support, tests, and EN/ZH
  user documentation.

### Out of scope

- A universal automatic XML-to-JSON mapping.
- XML Schema or WSDL validation and code generation.
- Network-backed entity or schema resolution.
- Transparent multipart, Protobuf, MessagePack, CSV, or YAML conversion.
- Full response-body buffering or a response-side XML API.

## Expected user scenarios

- Extract fields from SOAP requests for routing or authorization.
- Convert a bounded XML/SOAP request into application-specific JSON.
- Construct XML requests for legacy upstream services from JSON input.
- Normalize namespaces or envelope fields before proxying.

## Design questions

- Which owned XML value representation preserves order and namespace identity
  without retaining an unbounded parser DOM?
- Which operations are supported in the first API, and how are missing,
  repeated, and namespace-qualified values represented?
- Can incremental parsing reduce peak memory while keeping a script-usable
  bounded tree or event projection?
- What are the exact gas, byte, node, depth, and output accounting rules?
- How are namespace declarations, prefixes, escaping, comments, processing
  instructions, and mixed content handled deterministically?
- Which malformed-input and serializer failures map to the existing generic
  fail-closed behavior?

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-dsl` | Parser-facing value model, compiler/bytecode, static validation, and builtin dispatch contract |
| `edgion-gateway` | Bounded request-body scanner, VM runtime, serializer, and transactional publication |
| `edgion-resources` | DSL configuration validation if the public API introduces resource-level limits |
| `edgion-tests/` | XML fixtures, body-cap, spill, replay, mutation, and security coverage |
| `docs/en/` and `docs/zh-CN/` | User-facing DSL API and limit documentation |

## Subtasks

| Subtask | Status | Output |
|------|------|------|
| Existing DSL and request-body capability inventory | pending | Current extension points and reuse boundaries |
| XML value model and API assessment | pending | Compared options with safety and compatibility conclusion |
| Limits, gas, and security design | pending | Hard bounds and disabled XML features |
| Compiler/bytecode and Gateway runtime design | pending | File-level change plan and data flow |
| Transactional serialization and publication | pending | Mutation and framing behavior |
| Unit, integration, security, and build-flavor tests | pending | Malicious-input and regression matrix |
| Documentation and independent review | pending | EN/ZH contract and review closure |

## Acceptance criteria

- The public DSL model and limits are documented before implementation.
- Entity expansion, DTDs, external resources, and network access are disabled
  and covered by safe-failure fixtures.
- Parsing, traversal, and serialization have bounded VM accounting.
- Invalid or oversized output cannot be published partially.
- Memory/spill behavior, body caps, replay, retry, mirroring, framing, and
  stock-Pingora behavior retain their existing invariants.
- Tests cover namespaces, attributes, child order, text/mixed content,
  repeated elements, malformed input, depth/node/text limits, and output caps.

## Related skills

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/dsl-engine.md`
- `skills/03-coding/dsl/SKILL.md`
- `skills/05-testing/01-integration-testing.md`
- `skills/11-doc/SKILL.md`

## Document index

| Document | Status | Notes |
|------|------|------|
| `dsl-xml-primitives.md` | pending | This task record contains the initial requirements and design questions |
| `00-context.md` | — | Create after the existing DSL/runtime inventory is complete |
| `01-design.md` | — | Create after the solution assessment and design review |
| `02-implementation.md` | — | Create at implementation start |
| `03-subtasks.md` | — | Subtasks are recorded in this task file until implementation begins |
| `06-decisions.md` | — | Create when design decisions need a separate log |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-02 | Expose XML structure rather than prescribe XML-to-JSON conversion | Namespace, ordering, mixed-content, and schema choices are application-specific |
| 2026-08-02 | Keep the feature request-side and bounded | The request-body cap and response-streaming invariants must remain authoritative |
